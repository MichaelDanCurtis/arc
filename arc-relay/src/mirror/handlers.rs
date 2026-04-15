//! Subscribe handler for session mirroring.
//!
//! [`handle_subscribe`] is called when a watcher WebSocket sends a
//! `ClientToArc::SubscribeSession` frame.  It performs permission checks,
//! replays scrollback, registers the watcher in `Fanout` + `PresenceTracker`,
//! and broadcasts a `PresenceChanged` event to all current watchers.
//!
//! Control-flow handlers ([`handle_request_control`], [`handle_approve_control`],
//! [`handle_deny_control`], [`handle_revoke_control`], [`handle_session_input`])
//! are called from the watcher inbound arm in `ws/handler.rs` (Task 2.10).

use tokio::sync::mpsc;
use uuid::Uuid;

use beacon_proto::envelope::ArcToClient;
use beacon_model::SessionId;

use crate::api::AppState;
use crate::mirror::fanout::WatcherSink;
use crate::mirror::invitation::SubscribePermission;
use crate::mirror::lease::{ApproveOutcome, GrabOutcome};

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

// ---------------------------------------------------------------------------
// Control-flow error type
// ---------------------------------------------------------------------------

/// Errors that can arise from control-flow handler calls.
///
/// A separate error type from [`SubscribeError`] keeps the two call-sites
/// cohesive: `handle_subscribe` always has DB calls and permission checks that
/// map cleanly to the subscribe variants; control handlers have no DB I/O and
/// their only silent-failure mode is "session not found in the mirror registry".
#[derive(thiserror::Error, Debug)]
pub enum ControlError {
    /// The session is not registered in the mirror registry — either the host
    /// has never connected or the session has already been cleaned up.  The
    /// caller should silently ignore the frame (no send back to the watcher).
    #[error("session not found in mirror registry")]
    SessionNotFound,
}

// ---------------------------------------------------------------------------
// Control-flow handlers
// ---------------------------------------------------------------------------

/// Handle a `ClientToArc::RequestControl` frame from a watcher.
///
/// - Owner: immediately granted; all watchers receive `LeaseChanged`.
/// - Eligible non-owner: creates a pending request; only owner-class sinks
///   receive `ControlRequested` (so non-owners don't see pending requests they
///   can't act on).
/// - Ineligible caller: silently ignored (`GrabOutcome::Rejected`).
///
/// **Lock discipline:** entire body executes under one mirror lock acquisition.
/// No `.await` inside the critical section — `fanout.broadcast` and
/// `lease.try_grab` are both sync.
pub async fn handle_request_control(
    state: &AppState,
    user_id: Uuid,
    device_label: &str,
    session_id: &str,
) -> Result<(), ControlError> {
    let mirror = state
        .mirror_registry
        .get(session_id)
        .await
        .ok_or(ControlError::SessionNotFound)?;

    let mut m = mirror.lock().await;

    match m.lease.try_grab(user_id, device_label.to_string()) {
        GrabOutcome::Granted { driver } => {
            let sid: SessionId = session_id.parse().unwrap_or_else(|_| SessionId::new());
            m.fanout.broadcast(
                session_id,
                ArcToClient::LeaseChanged {
                    session_id: sid,
                    driver: Some(driver),
                },
            );
            tracing::info!(
                user_id = %user_id,
                session_id = %session_id,
                "control granted (immediate)"
            );
        }
        GrabOutcome::Pending { request_id } => {
            let owner = m.lease.owner_user_id();
            let sid: SessionId = session_id.parse().unwrap_or_else(|_| SessionId::new());
            m.fanout.broadcast_to_owner(
                session_id,
                owner,
                ArcToClient::ControlRequested {
                    session_id: sid,
                    request_id: request_id.clone(),
                    from_user: user_id.to_string(),
                    from_device: device_label.to_string(),
                },
            );
            tracing::info!(
                user_id = %user_id,
                session_id = %session_id,
                request_id = %request_id,
                "control request queued (pending owner approval)"
            );
        }
        GrabOutcome::Rejected { reason } => {
            tracing::debug!(
                user_id = %user_id,
                session_id = %session_id,
                reason = %reason,
                "control request rejected"
            );
        }
    }

    Ok(())
}

/// Handle a `ClientToArc::ApproveControl` frame from the session owner.
///
/// Approves the pending request identified by `request_id`.  On success all
/// watchers receive `LeaseChanged` with the new driver info.
///
/// Outcome variants:
/// - `Granted` → broadcast `LeaseChanged` to all watchers.
/// - `AlreadyResolved` → log at debug (benign race: request timed out or was
///   already resolved by another path).
/// - `UnknownRequest` → log at debug (client may have sent a stale request_id).
/// - `Unauthorized` → log at warn (a non-owner tried to approve; treat as a
///   client bug rather than closing the connection, since the watcher loop is
///   shared with legitimate control frames).
pub async fn handle_approve_control(
    state: &AppState,
    approver_user_id: Uuid,
    request_id: &str,
    session_id: &str,
) -> Result<(), ControlError> {
    let mirror = state
        .mirror_registry
        .get(session_id)
        .await
        .ok_or(ControlError::SessionNotFound)?;

    let mut m = mirror.lock().await;

    match m.lease.approve(request_id, approver_user_id) {
        ApproveOutcome::Granted { driver, .. } => {
            let sid: SessionId = session_id.parse().unwrap_or_else(|_| SessionId::new());
            m.fanout.broadcast(
                session_id,
                ArcToClient::LeaseChanged {
                    session_id: sid,
                    driver: Some(driver),
                },
            );
            tracing::info!(
                approver = %approver_user_id,
                session_id = %session_id,
                request_id = %request_id,
                "control request approved"
            );
        }
        ApproveOutcome::AlreadyResolved => {
            tracing::debug!(
                approver = %approver_user_id,
                session_id = %session_id,
                request_id = %request_id,
                "approve: request already resolved (benign race)"
            );
        }
        ApproveOutcome::UnknownRequest => {
            tracing::debug!(
                approver = %approver_user_id,
                session_id = %session_id,
                request_id = %request_id,
                "approve: unknown request_id (stale client frame?)"
            );
        }
        ApproveOutcome::Unauthorized => {
            tracing::warn!(
                approver = %approver_user_id,
                session_id = %session_id,
                request_id = %request_id,
                "approve: non-owner attempted to approve control request"
            );
        }
    }

    Ok(())
}

/// Handle a `ClientToArc::DenyControl` frame from the session owner.
///
/// Denies the pending request identified by `request_id`. No broadcast is
/// emitted — the requesting watcher will time out client-side.
///
/// A non-owner caller is a no-op (`lease.deny` returns `false`) and logged
/// at debug level (same as `UnknownRequest` — the frame may be stale).
pub async fn handle_deny_control(
    state: &AppState,
    denier_user_id: Uuid,
    request_id: &str,
    session_id: &str,
) -> Result<(), ControlError> {
    let mirror = state
        .mirror_registry
        .get(session_id)
        .await
        .ok_or(ControlError::SessionNotFound)?;

    let mut m = mirror.lock().await;
    let removed = m.lease.deny(request_id, denier_user_id);

    if removed {
        tracing::info!(
            denier = %denier_user_id,
            session_id = %session_id,
            request_id = %request_id,
            "control request denied"
        );
    } else {
        tracing::debug!(
            denier = %denier_user_id,
            session_id = %session_id,
            request_id = %request_id,
            "deny: request not found or denier is not owner (no-op)"
        );
    }

    Ok(())
}

/// Handle a `ClientToArc::RevokeControl` frame from the session owner.
///
/// Clears the current driver (if any).  All watchers receive
/// `LeaseChanged { driver: None }`.
///
/// If the revoker is not the session owner the call is a no-op and a warning
/// is logged — non-owners cannot revoke.
pub async fn handle_revoke_control(
    state: &AppState,
    revoker_user_id: Uuid,
    session_id: &str,
) -> Result<(), ControlError> {
    let mirror = state
        .mirror_registry
        .get(session_id)
        .await
        .ok_or(ControlError::SessionNotFound)?;

    let mut m = mirror.lock().await;

    // Authorisation: only the owner may revoke.
    if revoker_user_id != m.lease.owner_user_id() {
        tracing::warn!(
            revoker = %revoker_user_id,
            session_id = %session_id,
            "revoke: non-owner attempted to revoke control"
        );
        return Ok(());
    }

    let had_driver = m.lease.revoke(revoker_user_id);
    if had_driver {
        let sid: SessionId = session_id.parse().unwrap_or_else(|_| SessionId::new());
        m.fanout.broadcast(
            session_id,
            ArcToClient::LeaseChanged {
                session_id: sid,
                driver: None,
            },
        );
        tracing::info!(
            revoker = %revoker_user_id,
            session_id = %session_id,
            "control revoked; LeaseChanged(driver=None) broadcast"
        );
    } else {
        tracing::debug!(
            revoker = %revoker_user_id,
            session_id = %session_id,
            "revoke: no active driver to revoke"
        );
    }

    Ok(())
}

/// Handle a `ClientToArc::SessionInput` frame from a watcher.
///
/// Routing logic:
/// 1. If `user_id` is already the current driver → pass through immediately.
/// 2. If not the driver, attempt an implicit reclaim via
///    [`LeaseManager::try_implicit_reclaim`] — this succeeds only for the
///    session owner.  On success, broadcast `LeaseChanged` so all watchers see
///    the ownership transfer before the first keystroke is forwarded.
/// 3. On successful driver status (either path), forward bytes to the host via
///    `mirror.host_input_tx`.
///
/// **Design decision — `host_input_tx` over `router.send_to_host`:**
/// We use `host_input_tx` (the mpsc channel plumbed in `ws/host.rs` Task 2.10)
/// rather than `router.send_to_host`.  The router path was added in Task 2.5 for
/// the attached-client path and encodes the bytes as a full `ClientToArc`
/// message before forwarding; `host_input_tx` sends an `ArcToClient::HostInput`
/// frame that the host pump writes directly.  This is the correct direction for
/// mirrored input (relay → host) and keeps the control path consistent with how
/// the host_input_tx pump is wired.
///
/// If `host_input_tx` is `None` (host not yet connected) the input is dropped
/// silently — the caller already holds the driver seat and a subsequent
/// keystroke will retry once the host connects.
///
/// **Lock discipline:** same as other control handlers — one acquisition, no
/// `.await` inside the critical section.
pub async fn handle_session_input(
    state: &AppState,
    user_id: Uuid,
    device_label: &str,
    session_id: &str,
    bytes: Vec<u8>,
) -> Result<(), ControlError> {
    let mirror = state
        .mirror_registry
        .get(session_id)
        .await
        .ok_or(ControlError::SessionNotFound)?;

    let mut m = mirror.lock().await;

    // Check if caller is already the driver.
    let already_driver = m
        .lease
        .driver()
        .is_some_and(|d| d.user_id == user_id.to_string());

    if !already_driver {
        // Attempt implicit reclaim (owner-class only).
        match m.lease.try_implicit_reclaim(user_id, device_label.to_string()) {
            GrabOutcome::Granted { driver } => {
                let sid: SessionId = session_id.parse().unwrap_or_else(|_| SessionId::new());
                m.fanout.broadcast(
                    session_id,
                    ArcToClient::LeaseChanged {
                        session_id: sid,
                        driver: Some(driver),
                    },
                );
                tracing::info!(
                    user_id = %user_id,
                    session_id = %session_id,
                    "implicit reclaim granted on SessionInput"
                );
            }
            GrabOutcome::Rejected { reason } => {
                tracing::debug!(
                    user_id = %user_id,
                    session_id = %session_id,
                    reason = %reason,
                    "SessionInput dropped: not driver and implicit reclaim rejected"
                );
                return Ok(());
            }
            GrabOutcome::Pending { .. } => {
                // try_implicit_reclaim never returns Pending (it only calls
                // try_grab for the owner, which always grants immediately).
                // Guard arm included for exhaustiveness.
                return Ok(());
            }
        }
    }

    // Forward to host via host_input_tx.
    let sid: SessionId = session_id.parse().unwrap_or_else(|_| SessionId::new());
    if let Some(host_tx) = &m.host_input_tx {
        let _ = host_tx.try_send(ArcToClient::HostInput {
            session_id: sid,
            bytes,
            from_user: user_id.to_string(),
        });
    } else {
        tracing::debug!(
            user_id = %user_id,
            session_id = %session_id,
            "SessionInput: host_input_tx not set (host not connected); input dropped"
        );
    }

    Ok(())
}
