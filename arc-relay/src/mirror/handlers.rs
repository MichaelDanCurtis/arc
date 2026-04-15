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
#[derive(Debug)]
pub enum SubscribeError {
    /// The user does not have permission to subscribe to this session.
    Denied,
    /// A database query failed.
    Db(sqlx::Error),
    /// The session does not exist.
    SessionNotFound,
}

impl From<sqlx::Error> for SubscribeError {
    fn from(e: sqlx::Error) -> Self {
        SubscribeError::Db(e)
    }
}

impl std::fmt::Display for SubscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubscribeError::Denied => write!(f, "permission denied"),
            SubscribeError::Db(e) => write!(f, "database error: {e}"),
            SubscribeError::SessionNotFound => write!(f, "session not found"),
        }
    }
}

impl std::error::Error for SubscribeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SubscribeError::Db(e) => Some(e),
            _ => None,
        }
    }
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

    let mut m = mirror.lock().await;

    // ------------------------------------------------------------------
    // 4. Replay scrollback
    // ------------------------------------------------------------------
    // Parse the session_id string into the SessionId newtype used in wire frames.
    let sid: SessionId = session_id.parse().unwrap_or_else(|_| SessionId::new());

    let start = since_seq.unwrap_or(0);
    if since_seq.is_some() && m.scrollback.out_of_range(start) {
        let last_seq = m.scrollback.next_seq().saturating_sub(1);
        let _ = sink_tx
            .send(ArcToClient::SessionDiscontinuity {
                session_id: sid,
                last_seq,
            })
            .await;
    }

    // Collect (seq, bytes) pairs while holding the lock, then send outside.
    // ScrollbackEntry has `at: Instant` which is !Clone, so we extract fields.
    let replay_chunks: Vec<(u64, Vec<u8>)> = m
        .scrollback
        .replay_since(start)
        .map(|e| (e.seq, e.bytes.clone()))
        .collect();

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

    // ------------------------------------------------------------------
    // 5. Register sink + presence
    // ------------------------------------------------------------------
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

    let snapshot = m.presence.snapshot(session_id);
    drop(m);

    // ------------------------------------------------------------------
    // 6. Broadcast PresenceChanged
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
