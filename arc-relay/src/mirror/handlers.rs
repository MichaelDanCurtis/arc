//! Subscribe handler for session mirroring.
//!
//! [`handle_subscribe`] is called when a watcher WebSocket sends a
//! `ClientToArc::SubscribeSession` frame.  It performs permission checks,
//! replays scrollback, registers the watcher in `Fanout` + `PresenceTracker`,
//! and broadcasts a `PresenceChanged` event to all current watchers.

use tokio::sync::mpsc;
use uuid::Uuid;

use beacon_proto::envelope::ArcToClient;
use beacon_model::SessionId;

use crate::api::AppState;
use crate::mirror::fanout::WatcherSink;
use crate::mirror::invitation::SubscribePermission;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during a `SubscribeSession` handshake.
///
/// Uses `thiserror` to derive `Display`, `Error`, and `From` impls rather than
/// hand-rolling them, keeping the variant list as the single source of truth.
#[derive(thiserror::Error, Debug)]
pub enum SubscribeError {
    /// The user does not have permission to subscribe to this session.
    #[error("permission denied")]
    Denied,

    /// A database query failed.
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    /// The session does not exist (or the client-supplied session_id string
    /// failed to parse as a valid `SessionId` — an unparseable ID semantically
    /// equates to a non-existent session from the server's perspective).
    #[error("session not found")]
    SessionNotFound,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Process a `SubscribeSession` frame from a watcher WebSocket.
///
/// Steps:
/// 1. Permission check via `InvitationStore`.
/// 2. Load owner + eligible users from DB.
/// 3. `get_or_init` the `SessionMirror`.
/// 4. Replay scrollback (with `SessionDiscontinuity` marker if out-of-range).
/// 5. Register watcher in `Fanout` + `PresenceTracker`.
/// 6. Broadcast `PresenceChanged` to all watchers.
///
/// **Sanctioned deviation:** `device_label` is hardcoded to `"Watcher"` for
/// Task 2.8.  Task 2.10+ will add it to the `SubscribeSession` wire variant.
pub async fn handle_subscribe(
    state: &AppState,
    user_id: Uuid,
    user_email: &str,
    device_label: &str,
    session_id: &str,
    since_seq: Option<u64>,
    sink_tx: mpsc::Sender<ArcToClient>,
) -> Result<(), SubscribeError> {
    // ------------------------------------------------------------------
    // 1. Permission check
    // ------------------------------------------------------------------
    let perm = state
        .invitations
        .check_subscribe_permission(session_id, user_id, user_email)
        .await?;

    let can_drive = match perm {
        SubscribePermission::Denied => return Err(SubscribeError::Denied),
        SubscribePermission::Owner => true,
        SubscribePermission::Granted { can_drive } => can_drive,
    };

    // ------------------------------------------------------------------
    // 2. Load owner + eligible from DB
    // ------------------------------------------------------------------
    let owner: Uuid = sqlx::query_scalar::<_, Uuid>(
        "SELECT owner_id FROM sessions WHERE id = $1",
    )
    .bind(session_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(SubscribeError::SessionNotFound)?;

    let eligible: Vec<Uuid> = sqlx::query_scalar(
        "SELECT grantee_user_id FROM session_grants \
         WHERE session_id = $1 AND grantee_user_id IS NOT NULL \
           AND can_request_control AND revoked_at IS NULL",
    )
    .bind(session_id)
    .fetch_all(&state.pool)
    .await?;

    // ------------------------------------------------------------------
    // 3. Get or init the mirror
    // ------------------------------------------------------------------
    let mirror = state
        .mirror_registry
        .get_or_init(session_id, owner, eligible)
        .await;

    // ------------------------------------------------------------------
    // 4. Replay scrollback — extract all data then drop the lock before
    //    any .send().await calls to avoid holding a contended per-session
    //    mutex across unbounded await points.
    // ------------------------------------------------------------------

    // Parse the session_id string into the SessionId newtype used in wire frames.
    // An unparseable string is treated as SessionNotFound rather than fabricating
    // a phantom UUID — the client sent a malformed ID which semantically means
    // "the session doesn't exist".
    let sid: SessionId = session_id
        .parse()
        .map_err(|_| SubscribeError::SessionNotFound)?;

    let start = since_seq.unwrap_or(0);

    let (out_of_range_flag, last_seq, replay_chunks) = {
        let m = mirror.lock().await;

        let out_of_range = since_seq.is_some() && m.scrollback.out_of_range(start);
        let last_seq = m.scrollback.next_seq().saturating_sub(1);

        // Collect (seq, bytes) pairs while holding the lock.
        // ScrollbackEntry has `at: Instant` which is !Clone, so we extract fields.
        // When out-of-range, skip collection — we will send SessionDiscontinuity only.
        let chunks: Vec<(u64, Vec<u8>)> = if out_of_range {
            Vec::new()
        } else {
            m.scrollback
                .replay_since(start)
                .map(|e| (e.seq, e.bytes.clone()))
                .collect()
        };

        (out_of_range, last_seq, chunks)
        // lock guard `m` is dropped here
    };

    // All sink_tx.send calls happen WITHOUT holding the mirror lock.
    if out_of_range_flag {
        // When the requested sequence is out of scrollback range, send a
        // discontinuity marker and skip replay — do NOT send both.
        let _ = sink_tx
            .send(ArcToClient::SessionDiscontinuity {
                session_id: sid,
                last_seq,
            })
            .await;
    } else {
        for (seq, bytes) in replay_chunks {
            let _ = sink_tx
                .send(ArcToClient::SessionOutput {
                    session_id: sid,
                    bytes,
                    seq,
                    replay: true,
                })
                .await;
        }
    }

    // ------------------------------------------------------------------
    // 5. Register sink + presence — re-acquire lock for fanout/presence
    //    registration, then release before the final broadcast.
    // ------------------------------------------------------------------
    let snapshot = {
        let mut m = mirror.lock().await;
        m.fanout.add(
            session_id,
            WatcherSink {
                user_id,
                device_label: device_label.to_string(),
                tx: sink_tx,
            },
        );
        m.presence
            .add(session_id, user_id, device_label.to_string(), can_drive);

        m.presence.snapshot(session_id)
        // lock guard `m` is dropped here
    };

    // ------------------------------------------------------------------
    // 6. Broadcast PresenceChanged — lock NOT held during broadcast
    // ------------------------------------------------------------------
    if let Some(mirror) = state.mirror_registry.get(session_id).await {
        let mut m = mirror.lock().await;
        m.fanout.broadcast(
            session_id,
            ArcToClient::PresenceChanged {
                session_id: sid,
                watchers: snapshot,
            },
        );
    }

    Ok(())
}
