//! JSON-RPC 2.0 over a WebSocket text-frame channel.
//!
//! Auth is validated by the upgrade handler before this module runs.
//! Each connection gets a fresh [`RpcDispatcher`] populated with:
//!   - `arc.session.*` handlers (always)
//!   - `arc.ai.*` handlers (only when `state.arc_ai_backend` is `Some`)
//!
//! Inbound text frames are dispatched via [`RpcDispatcher::dispatch_raw`] and
//! the JSON string response is sent back as a text frame. Ping → Pong.
//! Binary frames are ignored with a warning. Close → clean exit.
//!
//! # Token revalidation
//!
//! A periodic `TOKEN_REVALIDATE_INTERVAL` timer re-decodes the bearer token
//! to detect expiry mid-connection and sends a Close frame with the
//! appropriate close code (4001 expired / 4003 invalid) before breaking.
//! This matches the behaviour the deleted msgpack `handle_connection` had.

use std::collections::HashSet;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use beacon_jsonrpc::RpcDispatcher;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

use crate::api::AppState;
use crate::ws::handler::{
    token_decode_status, TokenStatus,
    CLOSE_TOKEN_EXPIRED, CLOSE_TOKEN_INVALID, TOKEN_REVALIDATE_INTERVAL,
};

pub async fn handle_jsonrpc_connection(
    socket: WebSocket,
    user_id: Uuid,
    token: String,
    state: AppState,
) {
    let jwt_secret = state.config.jwt_secret.clone();

    let mut dispatcher = RpcDispatcher::new();
    crate::rpc::register(
        &mut dispatcher,
        state.arc_session_store.clone(),
        state.relay.clone(),
        state.mirror_registry.clone(),
        state.pool.clone(),
        user_id,
    );
    if let Some(backend) = state.arc_ai_backend.clone() {
        crate::ai_rpc::register(
            &mut dispatcher,
            backend,
            state.arc_ai_session_store.clone(),
        );
    }

    tracing::info!(
        user_id = %user_id,
        methods = dispatcher.handler_count(),
        "WebSocket arc.json-rpc connection established",
    );

    let (mut ws_tx, mut ws_rx) = socket.split();
    let (notif_tx, mut notif_rx) = mpsc::channel::<String>(256);

    let mut session_subs: HashSet<String> = HashSet::new();
    let mut ai_subs: HashSet<String> = HashSet::new();
    let mut forwarders: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // Periodic token-expiry check — mirrors the deleted msgpack handle_connection loop.
    let mut expiry_tick = tokio::time::interval(TOKEN_REVALIDATE_INTERVAL);
    expiry_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    expiry_tick.tick().await; // consume the immediate first tick

    loop {
        tokio::select! {
            _ = expiry_tick.tick() => {
                match token_decode_status(&token, &jwt_secret) {
                    TokenStatus::Valid => {}
                    TokenStatus::Expired => {
                        tracing::info!(user_id = %user_id, "json-rpc WS closing: access token expired");
                        let _ = ws_tx.send(Message::Close(Some(CloseFrame {
                            code: CLOSE_TOKEN_EXPIRED,
                            reason: "access token expired".into(),
                        }))).await;
                        break;
                    }
                    TokenStatus::Invalid => {
                        tracing::warn!(user_id = %user_id, "json-rpc WS closing: token decode failed");
                        let _ = ws_tx.send(Message::Close(Some(CloseFrame {
                            code: CLOSE_TOKEN_INVALID,
                            reason: "token rejected".into(),
                        }))).await;
                        break;
                    }
                }
            }
            Some(line) = notif_rx.recv() => {
                if ws_tx.send(Message::Text(line)).await.is_err() {
                    break;
                }
            }
            frame = ws_rx.next() => {
                let Some(Ok(frame)) = frame else { break; };
                match frame {
                    Message::Text(req_str) => {
                        if let Ok(req) = serde_json::from_str::<serde_json::Value>(&req_str) {
                            handle_subscribe_side_effects(
                                &req,
                                &state,
                                &notif_tx,
                                &mut session_subs,
                                &mut ai_subs,
                                &mut forwarders,
                            ).await;
                        }
                        let resp = dispatcher.dispatch_raw(&req_str).await;
                        if ws_tx.send(Message::Text(resp)).await.is_err() {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(p) => {
                        let _ = ws_tx.send(Message::Pong(p)).await;
                    }
                    Message::Pong(_) => {}
                    Message::Binary(_) => {
                        tracing::warn!("binary frame on json-rpc subprotocol; ignoring");
                    }
                }
            }
        }
    }

    for handle in forwarders {
        handle.abort();
    }
}

async fn handle_subscribe_side_effects(
    req: &serde_json::Value,
    state: &AppState,
    notif_tx: &mpsc::Sender<String>,
    session_subs: &mut HashSet<String>,
    ai_subs: &mut HashSet<String>,
    forwarders: &mut Vec<tokio::task::JoinHandle<()>>,
) {
    let Some(method) = req.get("method").and_then(|v| v.as_str()) else { return; };
    let session_id = req
        .get("params")
        .and_then(|p| p.get("session_id"))
        .and_then(|s| s.as_str());
    let Some(session_id) = session_id else { return; };

    let want_session = matches!(
        method,
        "arc.session.attach"
            | "arc.session.detach"
            | "arc.session.subscribe"
            | "arc.session.unsubscribe"
            | "arc.session.control"
            | "arc.session.release"
            | "arc.session.approve_control"
            | "arc.session.deny_control"
            | "arc.session.input"
            | "arc.session.resize"
            | "arc.host.session_register"
            | "arc.host.session_unregister"
            | "arc.host.block_start"
            | "arc.host.block_chunk"
            | "arc.host.block_end"
            | "arc.host.session_resize"
            | "arc.host.push_output"
            | "arc.host.shadow_diff_response"
    );
    if want_session && session_subs.insert(session_id.to_string()) {
        let _ = state.arc_session_store.get_or_create(session_id).await;
        if let Some(rx) = state.arc_session_store.subscribe(session_id).await {
            forwarders.push(spawn_forwarder(rx, notif_tx.clone()));
        }
    }

    if method == "arc.ai.stream_chat" && ai_subs.insert(session_id.to_string()) {
        let _ = state.arc_ai_session_store.get_or_create(session_id).await;
        if let Some(rx) = state.arc_ai_session_store.subscribe(session_id).await {
            forwarders.push(spawn_forwarder(rx, notif_tx.clone()));
        }
    }
}

fn spawn_forwarder(
    mut rx: tokio::sync::broadcast::Receiver<String>,
    notif_tx: mpsc::Sender<String>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(line) => {
                    if notif_tx.send(line).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    })
}
