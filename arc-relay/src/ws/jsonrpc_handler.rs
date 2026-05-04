//! JSON-RPC 2.0 over a WebSocket text-frame channel.
//!
//! Auth is validated by the upgrade handler before this module runs.
//! Each connection gets a fresh [`RpcDispatcher`] populated with:
//!   - `arc.session.*` handlers (always, when `JsonRpcSpine` is on)
//!   - `arc.ai.*` handlers (only when `state.arc_ai_backend` is `Some`)
//!
//! Inbound text frames are dispatched via [`RpcDispatcher::dispatch_raw`] and
//! the JSON string response is sent back as a text frame. Ping → Pong.
//! Binary frames are ignored with a warning. Close → clean exit.

use std::collections::HashSet;

use axum::extract::ws::{Message, WebSocket};
use beacon_jsonrpc::RpcDispatcher;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::api::AppState;

pub async fn handle_jsonrpc_connection(
    socket: WebSocket,
    user_id: Uuid,
    _token: String,
    state: AppState,
) {
    let mut dispatcher = RpcDispatcher::new();
    crate::rpc::register_if_enabled(&mut dispatcher, state.arc_session_store.clone());
    if let Some(backend) = state.arc_ai_backend.clone() {
        crate::ai_rpc::register_if_enabled(
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

    loop {
        tokio::select! {
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
