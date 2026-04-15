use axum::extract::ws::{Message, WebSocket};
use beacon_proto::{
    codec::{decode, encode},
    envelope::{ArcToClient, ClientToArc},
    PROTOCOL_VERSION,
};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::interval;
use uuid::Uuid;

use crate::ws::handler::{close_with_expired, token_is_expired, TOKEN_REVALIDATE_INTERVAL};
use crate::{
    models::session::{create_session, update_session_state},
    relay::router::RelayRouter,
};

/// Handle a host WebSocket connection.
///
/// Called after the upgrade handler has already decoded the opening `HostHello`
/// message and passed it in as `host_hello`.
///
/// Lifecycle:
/// 1. Send `HelloAck`.
/// 2. Open an mpsc channel so that client input can be forwarded to this host.
/// 3. Spawn a task that drains the mpsc receiver and writes frames to the socket.
/// 4. Main receive loop: match on incoming `ClientToArc` variants and drive the
///    relay / DB accordingly.
/// 5. On disconnect: mark all sessions owned by this host as "suspended".
pub async fn handle_host(
    mut socket: WebSocket,
    user_id: Uuid,
    host_hello: ClientToArc,
    router: Arc<RelayRouter>,
    pool: PgPool,
    token: String,
    jwt_secret: String,
) {
    // ── 1. Log the HostHello fields (we don't re-validate auth here; the
    //        upgrade handler already checked the JWT) ────────────────────────
    let host_id_str = match &host_hello {
        ClientToArc::HostHello { host_id, .. } => host_id.clone(),
        _ => {
            tracing::error!("handle_host called with non-HostHello message");
            let _ = socket.close().await;
            return;
        }
    };
    tracing::info!(user_id = %user_id, host_id = %host_id_str, "host connected");

    // ── 2. Send HelloAck ─────────────────────────────────────────────────────
    let ack = ArcToClient::HelloAck {
        protocol_version: PROTOCOL_VERSION,
    };
    match encode(&ack) {
        Ok(bytes) => {
            if socket.send(Message::Binary(bytes)).await.is_err() {
                tracing::warn!(user_id = %user_id, "failed to send HelloAck; closing");
                return;
            }
        }
        Err(e) => {
            tracing::error!(user_id = %user_id, "failed to encode HelloAck: {e}");
            let _ = socket.close().await;
            return;
        }
    }

    // ── 3. Channel: client-input → host socket ───────────────────────────────
    // host_tx is cloned into the router entry for each session registered by
    // this host. The forward task drains host_rx and writes frames to socket.
    let (host_tx, mut host_rx) = mpsc::channel::<Vec<u8>>(256);

    // We need to split sending / receiving on the socket.  axum's WebSocket
    // doesn't implement split directly, so we use a second mpsc channel to
    // funnel outbound frames through the main task.  The forward task just
    // enqueues; the main loop sends.
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(256);

    // Drain host_rx (client input forwarded by the router) into out_tx.
    tokio::spawn(async move {
        while let Some(data) = host_rx.recv().await {
            if out_tx.send(data).await.is_err() {
                break; // socket writer is gone
            }
        }
    });

    // ── 4. Track sessions registered by this host so we can suspend on exit ──
    let mut registered_sessions: Vec<String> = Vec::new();

    // ── 5. Main loop ─────────────────────────────────────────────────────────
    // Periodic token re-validation. Interval is 30 s in production (well under
    // the 15-min access-token lifetime) and 1 s when built with fast-revalidate.
    let mut expiry_tick = interval(TOKEN_REVALIDATE_INTERVAL);
    expiry_tick.tick().await; // consume the immediate first tick

    loop {
        tokio::select! {
            // Outbound: forward client input to the host.
            Some(data) = out_rx.recv() => {
                if socket.send(Message::Binary(data)).await.is_err() {
                    tracing::info!(user_id = %user_id, "host socket send error; disconnecting");
                    break;
                }
            }

            // Token expiry check.
            _ = expiry_tick.tick() => {
                if token_is_expired(&token, &jwt_secret) {
                    tracing::warn!(user_id = %user_id, "host token expired; closing 4001");
                    close_with_expired(&mut socket).await;
                    break;
                }
            }

            // Inbound: read from the host.
            msg = socket.recv() => {
                match msg {
                    None => {
                        tracing::info!(user_id = %user_id, "host socket closed");
                        break;
                    }
                    Some(Err(e)) => {
                        tracing::warn!(user_id = %user_id, "host socket recv error: {e}");
                        break;
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        match decode::<ClientToArc>(&bytes) {
                            Err(e) => {
                                tracing::warn!(
                                    user_id = %user_id,
                                    "host: decode error: {e}"
                                );
                            }
                            Ok(ClientToArc::SessionRegister { session_id, name }) => {
                                let sid = session_id.to_string();
                                tracing::info!(
                                    user_id = %user_id,
                                    session_id = %sid,
                                    "host registering session"
                                );
                                router.register_session(
                                    sid.clone(),
                                    host_tx.clone(),
                                    user_id,
                                    name.clone(),
                                );
                                registered_sessions.push(sid.clone());

                                // Persist session to DB (best-effort).
                                let pool2 = pool.clone();
                                let sid2 = sid.clone();
                                let name2 = name.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = create_session(
                                        &pool2,
                                        &sid2,
                                        user_id, // host_id = connecting user
                                        user_id, // owner_id = connecting user
                                        Some(&name2),
                                    )
                                    .await
                                    {
                                        tracing::warn!(
                                            session_id = %sid2,
                                            "create_session DB error: {e}"
                                        );
                                    }
                                });
                            }

                            Ok(ClientToArc::SessionUnregister { session_id }) => {
                                let sid = session_id.to_string();
                                tracing::info!(
                                    user_id = %user_id,
                                    session_id = %sid,
                                    "host unregistering session"
                                );
                                router.unregister_session(&sid);
                                registered_sessions.retain(|s| s != &sid);

                                let pool2 = pool.clone();
                                tokio::spawn(async move {
                                    if let Err(e) =
                                        update_session_state(&pool2, &sid, "ended").await
                                    {
                                        tracing::warn!(
                                            session_id = %sid,
                                            "update_session_state DB error: {e}"
                                        );
                                    }
                                });
                            }

                            Ok(ClientToArc::BlockStart { session_id, header }) => {
                                let sid = session_id.to_string();
                                let out_msg = ArcToClient::BlockStart {
                                    session_id,
                                    header,
                                };
                                broadcast_or_warn(&router, &sid, &out_msg);
                            }

                            Ok(ClientToArc::BlockOutputChunk {
                                session_id,
                                block_id,
                                seq,
                                bytes,
                            }) => {
                                let sid = session_id.to_string();
                                let out_msg = ArcToClient::BlockOutputChunk {
                                    session_id,
                                    block_id,
                                    seq,
                                    bytes,
                                };
                                broadcast_or_warn(&router, &sid, &out_msg);
                            }

                            Ok(ClientToArc::BlockEnd { session_id, footer }) => {
                                let sid = session_id.to_string();
                                tracing::debug!(
                                    session_id = %sid,
                                    block_id = %footer.id,
                                    "block ended"
                                );
                                let out_msg = ArcToClient::BlockEnd {
                                    session_id,
                                    footer,
                                };
                                broadcast_or_warn(&router, &sid, &out_msg);
                                // TODO(task-later): accumulate output chunks and
                                // call persist_block once we track in-flight blocks.
                            }

                            Ok(ClientToArc::SessionResize {
                                session_id,
                                cols,
                                rows,
                            }) => {
                                let sid = session_id.to_string();
                                router.update_size(&sid, cols, rows);
                            }

                            Ok(ClientToArc::ShadowDiffResponse {
                                session_id: _,
                                request_id,
                                diff,
                            }) => {
                                tracing::debug!(
                                    user_id = %user_id,
                                    request_id = %request_id,
                                    "shadow diff response received"
                                );
                                if !router.resolve_shadow_diff(&request_id, diff) {
                                    tracing::warn!(
                                        request_id = %request_id,
                                        "no waiter for shadow diff response"
                                    );
                                }
                            }

                            Ok(ClientToArc::Pong) => {
                                tracing::trace!(user_id = %user_id, "host pong");
                            }

                            Ok(other) => {
                                tracing::warn!(
                                    user_id = %user_id,
                                    "host: unexpected message variant: {other:?}"
                                );
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        tracing::info!(user_id = %user_id, "host sent Close frame");
                        break;
                    }
                    Some(Ok(_)) => {
                        // Text / ping / pong frames — ignore.
                    }
                }
            }
        }
    }

    // ── 6. Suspend all sessions registered by this host ──────────────────────
    for sid in &registered_sessions {
        tracing::info!(user_id = %user_id, session_id = %sid, "suspending session on host disconnect");
        router.suspend_session(sid);

        let pool2 = pool.clone();
        let sid2 = sid.clone();
        tokio::spawn(async move {
            if let Err(e) = update_session_state(&pool2, &sid2, "suspended").await {
                tracing::warn!(session_id = %sid2, "suspend DB error: {e}");
            }
        });
    }

    tracing::info!(user_id = %user_id, "host handler exiting");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Encode `msg` and broadcast to `session_id`.  Logs a warning on failure.
fn broadcast_or_warn(router: &RelayRouter, session_id: &str, msg: &ArcToClient) {
    match encode(msg) {
        Ok(bytes) => {
            if !router.broadcast(session_id, bytes) {
                tracing::debug!(session_id = %session_id, "broadcast: no subscribers");
            }
        }
        Err(e) => {
            tracing::warn!(session_id = %session_id, "encode error in broadcast: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use beacon_proto::codec::decode;
    use beacon_proto::envelope::ArcToClient;
    use beacon_proto::PROTOCOL_VERSION;

    /// Verify HelloAck encodes the correct protocol version.
    #[test]
    fn hello_ack_has_correct_version() {
        let ack = ArcToClient::HelloAck {
            protocol_version: PROTOCOL_VERSION,
        };
        let bytes = encode(&ack).expect("encode");
        let decoded: ArcToClient = decode(&bytes).expect("decode");
        match decoded {
            ArcToClient::HelloAck { protocol_version } => {
                assert_eq!(protocol_version, PROTOCOL_VERSION);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// Verify that broadcast_or_warn delivers data to a subscriber.
    #[tokio::test]
    async fn broadcast_or_warn_delivers() {
        use tokio::sync::mpsc;
        let router = RelayRouter::new();
        let (tx, _rx) = mpsc::channel(16);
        let mut sub = router.register_session("s1".into(), tx, Uuid::new_v4(), "test".into());

        let msg = ArcToClient::Ping;
        broadcast_or_warn(&router, "s1", &msg);

        let data = sub.recv().await.expect("recv");
        let decoded: ArcToClient = decode(&data).expect("decode broadcast");
        assert!(matches!(decoded, ArcToClient::Ping));
    }

    /// Verify SessionResize updates dimensions in the router.
    #[tokio::test]
    async fn session_resize_updates_router() {
        let router = RelayRouter::new();
        let (tx, _rx) = mpsc::channel(16);
        router.register_session("s1".into(), tx, Uuid::new_v4(), "test".into());

        router.update_size("s1", 120, 40);
        let info = router.get_session_info("s1").expect("session info");
        assert_eq!(info.cols, 120);
        assert_eq!(info.rows, 40);
    }
}
