use axum::extract::ws::{Message, WebSocket};
use beacon_model::{Block, BlockFooter, BlockHeader, BlockId, HostInfo, SessionId, TabId};
use beacon_proto::{
    codec::{decode, encode},
    envelope::{ArcToClient, ClientToArc},
};
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::time::{interval, Duration};
use uuid::Uuid;

use crate::ws::handler::{close_with_expired, token_is_expired};
use crate::{models::block::get_blocks_for_session, relay::router::RelayRouter};

/// Handle a client (viewer) WebSocket connection.
///
/// Called after the upgrade handler has decoded the opening `AttachSession`
/// message and passed it in as `attach_msg`.
///
/// Lifecycle:
/// 1. Validate that the requested session exists in the router.
/// 2. Subscribe to the session's broadcast channel.
/// 3. Load historical blocks from the DB and replay them.
/// 4. Enter the main loop: forward live broadcast frames to the client while
///    reading client input (SessionInput, DetachSession) from the socket.
/// 5. On disconnect: clean up silently (no router state to remove for clients).
pub async fn handle_client(
    mut socket: WebSocket,
    user_id: Uuid,
    attach_msg: ClientToArc,
    router: Arc<RelayRouter>,
    pool: PgPool,
    token: String,
    jwt_secret: String,
) {
    // ── Unpack AttachSession ──────────────────────────────────────────────────
    let (session_id, since_block) = match attach_msg {
        ClientToArc::AttachSession {
            session_id,
            since_block,
        } => (session_id, since_block),
        _ => {
            tracing::error!("handle_client called with non-AttachSession message");
            let _ = socket.close().await;
            return;
        }
    };

    let sid = session_id.to_string();
    tracing::info!(user_id = %user_id, session_id = %sid, "client attaching");

    // ── 1. Check session exists ───────────────────────────────────────────────
    let session_info = match router.get_session_info(&sid) {
        Some(info) => info,
        None => {
            tracing::warn!(
                user_id = %user_id,
                session_id = %sid,
                "client attach denied: session not in router"
            );
            let deny = ArcToClient::AttachDenied {
                session_id,
                reason: "session not found".to_string(),
            };
            send_msg(&mut socket, &deny).await;
            let _ = socket.close().await;
            return;
        }
    };

    // ── 2. Subscribe to live broadcast before we load history, so we don't
    //       miss any frames that arrive between the DB read and the subscribe.
    let mut broadcast_rx = match router.subscribe(&sid) {
        Some(rx) => rx,
        None => {
            // Session disappeared between the get_session_info and subscribe calls.
            let deny = ArcToClient::AttachDenied {
                session_id,
                reason: "session disappeared".to_string(),
            };
            send_msg(&mut socket, &deny).await;
            let _ = socket.close().await;
            return;
        }
    };

    // ── 3. Load historical blocks from DB ─────────────────────────────────────
    let since_str = since_block.as_ref().map(|b| b.to_string());
    let history = match get_blocks_for_session(&pool, &sid, since_str.as_deref(), 1000).await {
        Ok(blocks) => blocks,
        Err(e) => {
            tracing::warn!(session_id = %sid, "get_blocks_for_session error: {e}; continuing without history");
            vec![]
        }
    };

    // ── 4. Send AttachAck ─────────────────────────────────────────────────────
    let ack = ArcToClient::AttachAck {
        session_id,
        current_cols: session_info.cols,
        current_rows: session_info.rows,
        resume_from: since_block,
    };
    if !send_msg(&mut socket, &ack).await {
        return; // socket already closed
    }

    // ── 5. Send ReplayBegin ───────────────────────────────────────────────────
    let replay_sid = session_id_from_str(&sid);
    let total_blocks = history.len() as u32;

    let replay_begin = ArcToClient::ReplayBegin {
        session_id: replay_sid,
        total_blocks,
    };
    if !send_msg(&mut socket, &replay_begin).await {
        return;
    }

    // ── 6. Send ReplayBlock for each historical block ─────────────────────────
    for db_block in &history {
        let block = db_block_to_block(db_block);
        let replay_block = ArcToClient::ReplayBlock {
            session_id: replay_sid,
            block,
        };
        if !send_msg(&mut socket, &replay_block).await {
            return;
        }
    }

    // ── 7. Send ReplayEnd ─────────────────────────────────────────────────────
    let replay_end = ArcToClient::ReplayEnd {
        session_id: replay_sid,
    };
    if !send_msg(&mut socket, &replay_end).await {
        return;
    }

    // ── 8. Main loop: live relay + client input ───────────────────────────────
    // Periodic token re-validation: check every second. JWT decode is cheap
    // (HMAC-SHA256 + JSON parse) so a 1-second interval imposes no measurable load.
    let mut expiry_tick = interval(Duration::from_secs(1));
    expiry_tick.tick().await; // consume the immediate first tick

    loop {
        tokio::select! {
            // Token expiry check.
            _ = expiry_tick.tick() => {
                if token_is_expired(&token, &jwt_secret) {
                    tracing::warn!(
                        user_id = %user_id,
                        session_id = %sid,
                        "client token expired; closing 4001"
                    );
                    close_with_expired(&mut socket).await;
                    break;
                }
            }

            // Live broadcast frames → client socket.
            result = broadcast_rx.recv() => {
                match result {
                    Ok(data) => {
                        if socket.send(Message::Binary(data)).await.is_err() {
                            tracing::info!(
                                user_id = %user_id,
                                session_id = %sid,
                                "client socket send error; disconnecting"
                            );
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            user_id = %user_id,
                            session_id = %sid,
                            "client broadcast lagged by {n} messages"
                        );
                        // Continue — don't disconnect the client for lag.
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::info!(
                            user_id = %user_id,
                            session_id = %sid,
                            "session broadcast channel closed (host disconnected)"
                        );
                        // Notify client that the session is gone.
                        let gone = ArcToClient::SessionGone {
                            session_id: replay_sid,
                            reason: "host disconnected".to_string(),
                        };
                        send_msg(&mut socket, &gone).await;
                        break;
                    }
                }
            }

            // Client messages → host (or control).
            msg = socket.recv() => {
                match msg {
                    None => {
                        tracing::info!(
                            user_id = %user_id,
                            session_id = %sid,
                            "client socket closed"
                        );
                        break;
                    }
                    Some(Err(e)) => {
                        tracing::warn!(
                            user_id = %user_id,
                            session_id = %sid,
                            "client socket recv error: {e}"
                        );
                        break;
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        match decode::<ClientToArc>(&bytes) {
                            Err(e) => {
                                tracing::warn!(
                                    user_id = %user_id,
                                    session_id = %sid,
                                    "client: decode error: {e}"
                                );
                            }
                            Ok(ClientToArc::SessionInput {
                                session_id: input_sid,
                                bytes: input_bytes,
                            }) => {
                                // Re-encode as a ClientToArc::SessionInput and
                                // forward to the host via the router.
                                let fwd = ClientToArc::SessionInput {
                                    session_id: input_sid,
                                    bytes: input_bytes,
                                };
                                match encode(&fwd) {
                                    Ok(encoded) => {
                                        if !router
                                            .send_to_host(&sid, encoded)
                                            .await
                                        {
                                            tracing::warn!(
                                                user_id = %user_id,
                                                session_id = %sid,
                                                "send_to_host failed (session gone?)"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            user_id = %user_id,
                                            session_id = %sid,
                                            "failed to encode SessionInput for forwarding: {e}"
                                        );
                                    }
                                }
                            }
                            Ok(ClientToArc::DetachSession { .. }) => {
                                tracing::info!(
                                    user_id = %user_id,
                                    session_id = %sid,
                                    "client requested detach"
                                );
                                break;
                            }
                            Ok(ClientToArc::Pong) => {
                                tracing::trace!(user_id = %user_id, "client pong");
                            }
                            Ok(ClientToArc::SessionResizeRequest {
                                session_id: rs_sid,
                                cols,
                                rows,
                            }) => {
                                // Forward resize request to the host.
                                let fwd = ClientToArc::SessionResizeRequest {
                                    session_id: rs_sid,
                                    cols,
                                    rows,
                                };
                                if let Ok(encoded) = encode(&fwd) {
                                    router.send_to_host(&sid, encoded).await;
                                }
                            }
                            Ok(other) => {
                                tracing::warn!(
                                    user_id = %user_id,
                                    session_id = %sid,
                                    "client: unexpected message: {other:?}"
                                );
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        tracing::info!(
                            user_id = %user_id,
                            session_id = %sid,
                            "client sent Close frame"
                        );
                        break;
                    }
                    Some(Ok(_)) => {
                        // Text / ping / pong frames — ignore.
                    }
                }
            }
        }
    }

    tracing::info!(user_id = %user_id, session_id = %sid, "client handler exiting");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Encode `msg` and send it as a binary WebSocket frame.
/// Returns `true` on success, `false` if the send failed (socket closed).
async fn send_msg(socket: &mut WebSocket, msg: &ArcToClient) -> bool {
    match encode(msg) {
        Ok(bytes) => {
            if socket.send(Message::Binary(bytes)).await.is_err() {
                tracing::warn!("client socket send failed");
                return false;
            }
            true
        }
        Err(e) => {
            tracing::warn!("encode error sending to client: {e}");
            false
        }
    }
}

/// Convert a `DbBlock` to a `beacon_model::Block` for replay.
///
/// - Output is decompressed from zstd if present; falls back to empty vec.
/// - `host_info` is deserialized from the stored JSONB; falls back to `HostInfo::current()`.
/// - `tab_id`, `session_id`, and timestamps are reconstructed from the DB columns.
fn db_block_to_block(db: &crate::models::block::DbBlock) -> Block {
    // Decompress output.
    let output = match &db.output {
        Some(compressed) => zstd::decode_all(compressed.as_slice()).unwrap_or_else(|e| {
            tracing::warn!(block_id = %db.id, "zstd decompress failed: {e}");
            vec![]
        }),
        None => vec![],
    };

    // Parse host_info from JSONB.
    let host_info: HostInfo = db
        .host_info
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_else(HostInfo::current);

    // Parse IDs (best-effort; fall back to fresh IDs so the block is still usable).
    let block_id: BlockId = db.id.parse().unwrap_or_else(|_| BlockId::new());
    let session_id: SessionId = db.session_id.parse().unwrap_or_else(|_| SessionId::new());
    let tab_id: TabId = db.tab_id.parse().unwrap_or_else(|_| TabId::new());

    // Timestamps: convert from chrono DateTime<Utc> → SystemTime.
    let started_at: SystemTime = db.started_at.into();

    let footer = db.ended_at.map(|ended| {
        let ended_at: SystemTime = ended.into();
        BlockFooter {
            id: block_id,
            exit_code: db.exit_code,
            ended_at,
            shadow_git_ref: db
                .shadow_git_ref
                .as_ref()
                .map(|r| beacon_model::ShadowCommitId(r.clone())),
        }
    });

    // Reconstruct cwd (may be None in DB).
    let cwd = db
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));

    Block {
        header: BlockHeader {
            id: block_id,
            session_id,
            tab_id,
            command: db.command.clone(),
            cwd,
            started_at,
            env_snapshot: HashMap::new(), // not stored in DB
            host_info,
        },
        output,
        footer,
    }
}

/// Parse a `SessionId` from a string, returning a fresh ID on failure.
fn session_id_from_str(s: &str) -> SessionId {
    s.parse().unwrap_or_else(|_| SessionId::new())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::block::DbBlock;
    use chrono::Utc;

    fn sample_db_block() -> DbBlock {
        DbBlock {
            id: BlockId::new().to_string(),
            session_id: SessionId::new().to_string(),
            tab_id: TabId::new().to_string(),
            command: "echo hello".to_string(),
            output: None,
            exit_code: Some(0),
            cwd: Some("/tmp".to_string()),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            shadow_git_ref: None,
            pinned: false,
            host_info: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn db_block_to_block_no_output() {
        let db = sample_db_block();
        let block = db_block_to_block(&db);
        assert_eq!(block.header.command, "echo hello");
        assert!(block.output.is_empty());
        assert!(block.footer.is_some());
    }

    #[test]
    fn db_block_to_block_with_output() {
        let mut db = sample_db_block();
        let raw = b"hello world\n".to_vec();
        let compressed = zstd::encode_all(raw.as_slice(), 3).expect("compress");
        db.output = Some(compressed);

        let block = db_block_to_block(&db);
        assert_eq!(block.output, b"hello world\n");
    }

    #[test]
    fn db_block_to_block_with_host_info() {
        let mut db = sample_db_block();
        let hi = HostInfo {
            hostname: "relay-host".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            beacon_version: "0.1.0".to_string(),
        };
        db.host_info = Some(serde_json::to_value(&hi).unwrap());

        let block = db_block_to_block(&db);
        assert_eq!(block.header.host_info.hostname, "relay-host");
    }

    #[test]
    fn db_block_to_block_no_footer_when_ended_at_none() {
        let mut db = sample_db_block();
        db.ended_at = None;

        let block = db_block_to_block(&db);
        assert!(block.footer.is_none());
    }

    /// Relay integration: register a session, broadcast, verify subscriber receives.
    #[tokio::test]
    async fn relay_broadcast_integration() {
        use tokio::sync::mpsc;

        let router = RelayRouter::new();
        let (tx, _rx) = mpsc::channel::<Vec<u8>>(16);
        let owner = Uuid::new_v4();

        let mut sub = router.register_session("sess".into(), tx, owner, "my session".into());

        // Broadcast an encoded ArcToClient message.
        let msg = ArcToClient::Ping;
        let encoded = encode(&msg).expect("encode");
        assert!(router.broadcast("sess", encoded));

        // Subscriber should receive it.
        let data = sub.recv().await.expect("recv");
        let decoded: ArcToClient = decode(&data).expect("decode");
        assert!(matches!(decoded, ArcToClient::Ping));
    }

    /// Relay integration: send_to_host forwards bytes through the channel.
    #[tokio::test]
    async fn relay_send_to_host_integration() {
        use tokio::sync::mpsc;

        let router = RelayRouter::new();
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);

        router.register_session("s2".into(), tx, Uuid::new_v4(), "test".into());

        let payload = b"keyboard input".to_vec();
        assert!(router.send_to_host("s2", payload.clone()).await);

        let received = rx.recv().await.expect("recv");
        assert_eq!(received, payload);
    }
}
