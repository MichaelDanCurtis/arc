//! Per-session broadcaster of [`ArcToClient`] frames to multiple [`WatcherSink`]s.
//!
//! # Slow-watcher policy
//!
//! Each watcher gets a bounded channel of depth [`QUEUE_DEPTH`]. When
//! [`Fanout::broadcast`] is called and a sink's channel is full, the sink is
//! immediately dropped from the session's list. The WebSocket task on the other
//! end of the channel observes `tx` closure (via `rx` returning `None`) and
//! should close the socket with WS close-code 1011 (internal error).
//!
//! # Why `try_send`, not `send`
//!
//! `broadcast` must never `await` — it runs on the host-output hot path where
//! a single blocked watcher must not stall every other subscriber. `try_send`
//! lets us detect a full queue instantly and evict the slow watcher without
//! yielding the executor thread.

use beacon_proto::envelope::ArcToClient;
use std::collections::HashMap;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Maximum number of frames buffered per watcher before they are evicted.
const QUEUE_DEPTH: usize = 256;

/// A single watcher's receive end: metadata plus the sender half of their
/// per-watcher channel.
pub struct WatcherSink {
    /// The authenticated user watching this session.
    pub user_id: Uuid,
    /// Human-readable label for the watcher's device (e.g. "Mac", "iPad").
    pub device_label: String,
    /// Sender half of the bounded channel delivered to the WS task.
    pub tx: mpsc::Sender<ArcToClient>,
}

/// Per-session fan-out table. Owned by `MirrorRegistry` (Task 2.7).
///
/// Keyed by session ID string so the API layer can address sessions without
/// carrying a `SessionId` newtype everywhere. `SessionId::to_string()` is the
/// canonical key.
pub struct Fanout {
    /// session_id (string) → list of active watcher sinks.
    sinks: HashMap<String, Vec<WatcherSink>>,
}

impl Fanout {
    /// Create an empty fanout table.
    pub fn new() -> Self {
        Self { sinks: HashMap::new() }
    }

    /// Register a new watcher sink for `session_id`.
    pub fn add(&mut self, session_id: &str, sink: WatcherSink) {
        self.sinks.entry(session_id.to_string()).or_default().push(sink);
    }

    /// Deregister a watcher by (user_id, device_label) pair for `session_id`.
    ///
    /// If no watchers remain after removal the session entry is dropped so
    /// subsequent `broadcast` calls are no-ops.
    pub fn remove(&mut self, session_id: &str, user_id: Uuid, device_label: &str) {
        if let Some(list) = self.sinks.get_mut(session_id) {
            list.retain(|s| !(s.user_id == user_id && s.device_label == device_label));
        }
        self.prune_if_empty(session_id);
    }

    /// Send `frame` to every watcher subscribed to `session_id`.
    ///
    /// Watchers whose channel is full are evicted immediately (their `tx` is
    /// dropped, causing the WS task's `rx` to return `None`). Already-closed
    /// channels are pruned silently.
    pub fn broadcast(&mut self, session_id: &str, frame: ArcToClient) {
        let Some(list) = self.sinks.get_mut(session_id) else { return };
        list.retain(|sink| {
            match sink.tx.try_send(frame.clone()) {
                Ok(_) => true,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(
                        user_id = %sink.user_id,
                        device  = %sink.device_label,
                        session = %session_id,
                        "slow watcher disconnect (queue full)"
                    );
                    false
                }
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        });
        self.prune_if_empty(session_id);
    }

    /// Remove the HashMap entry if the sink list for `session_id` is empty.
    /// Idempotent; safe to call when the session key is already absent.
    fn prune_if_empty(&mut self, session_id: &str) {
        if self.sinks.get(session_id).is_some_and(Vec::is_empty) {
            self.sinks.remove(session_id);
        }
    }

    /// Create a matched sender/receiver pair sized to [`QUEUE_DEPTH`].
    ///
    /// The sender is stored in a [`WatcherSink`]; the receiver is handed to
    /// the WebSocket task that pumps frames to the client.
    pub fn make_channel() -> (mpsc::Sender<ArcToClient>, mpsc::Receiver<ArcToClient>) {
        mpsc::channel(QUEUE_DEPTH)
    }

    /// Send `frame` to every watcher whose `user_id` matches `owner_user_id`,
    /// using the same slow-watcher eviction policy as [`broadcast`].
    ///
    /// # Design rationale
    ///
    /// The plan pseudocode calls for `owner_sinks` to return `Vec<&mut WatcherSink>`
    /// so callers can call `sink.tx.try_send(...)`.  In practice the borrow-checker
    /// makes mutable-ref iteration over a retained Vec awkward — any shared reference
    /// to the Vec blocks the `retain` call used for eviction.  A closure-based
    /// approach (same as `broadcast`) avoids that friction while keeping the same
    /// evict-on-full semantics.  The caller passes the frame to send, and this
    /// method handles eviction internally.
    pub fn broadcast_to_owner(&mut self, session_id: &str, owner_user_id: Uuid, frame: ArcToClient) {
        let Some(list) = self.sinks.get_mut(session_id) else { return };
        list.retain(|sink| {
            if sink.user_id != owner_user_id {
                return true; // keep non-owner sinks untouched
            }
            match sink.tx.try_send(frame.clone()) {
                Ok(_) => true,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(
                        user_id = %sink.user_id,
                        device  = %sink.device_label,
                        session = %session_id,
                        "slow owner-watcher disconnect (queue full, owner-only send)"
                    );
                    false
                }
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        });
        self.prune_if_empty(session_id);
    }
}

impl Default for Fanout {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beacon_model::SessionId;

    #[tokio::test]
    async fn slow_watcher_dropped_others_unaffected() {
        let mut f = Fanout::new();

        let (tx_slow, _rx_slow) = Fanout::make_channel();
        let (tx_fast, mut rx_fast) = Fanout::make_channel();

        let sid = SessionId::new();
        let sid_key = sid.to_string();

        f.add(&sid_key, WatcherSink {
            user_id: Uuid::new_v4(),
            device_label: "slow".into(),
            tx: tx_slow,
        });
        f.add(&sid_key, WatcherSink {
            user_id: Uuid::new_v4(),
            device_label: "fast".into(),
            tx: tx_fast,
        });

        let mut count = 0usize;
        for i in 0..1000u64 {
            f.broadcast(&sid_key, ArcToClient::SessionOutput {
                session_id: sid,
                bytes: vec![i as u8],
                seq: i,
                replay: false,
            });
            // Drain the fast sink inside the loop so its queue never fills up.
            while rx_fast.try_recv().is_ok() { count += 1; }
        }
        // Drain any straggler frames.
        while rx_fast.try_recv().is_ok() { count += 1; }

        assert!(count > 0, "fast watcher received no frames");
        // Slow watcher was evicted; only the fast sink remains.
        assert_eq!(f.sinks.get(&sid_key).unwrap().len(), 1, "expected only fast sink to remain");
    }

    #[tokio::test]
    async fn closed_channel_sink_silently_pruned() {
        let mut f = Fanout::new();
        let sid = SessionId::new();
        let sid_key = sid.to_string();
        let (tx, rx) = Fanout::make_channel();
        drop(rx); // Close the channel so try_send returns Closed
        f.add(&sid_key, WatcherSink {
            user_id: Uuid::new_v4(),
            device_label: "dead".into(),
            tx,
        });
        f.broadcast(&sid_key, ArcToClient::SessionOutput {
            session_id: sid,
            bytes: vec![1],
            seq: 0,
            replay: false,
        });
        // Closed sink pruned; empty-list cleanup drops the session entry.
        assert!(!f.sinks.contains_key(&sid_key));
    }

    #[tokio::test]
    async fn broadcast_to_owner_only_reaches_owner_sinks() {
        let mut f = Fanout::new();
        let sid = SessionId::new();
        let sid_key = sid.to_string();

        let owner = Uuid::new_v4();
        let other = Uuid::new_v4();

        let (tx_owner, mut rx_owner) = Fanout::make_channel();
        let (tx_other, mut rx_other) = Fanout::make_channel();

        f.add(&sid_key, WatcherSink {
            user_id: owner,
            device_label: "owner-device".into(),
            tx: tx_owner,
        });
        f.add(&sid_key, WatcherSink {
            user_id: other,
            device_label: "other-device".into(),
            tx: tx_other,
        });

        f.broadcast_to_owner(&sid_key, owner, ArcToClient::LeaseChanged {
            session_id: sid,
            driver: None,
        });

        // Owner sink must receive the frame.
        assert!(
            rx_owner.try_recv().is_ok(),
            "owner sink did not receive broadcast_to_owner frame"
        );
        // Non-owner sink must NOT receive it.
        assert!(
            rx_other.try_recv().is_err(),
            "non-owner sink unexpectedly received broadcast_to_owner frame"
        );
        // Both sinks remain registered (no eviction).
        assert_eq!(f.sinks.get(&sid_key).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn remove_drops_session_entry_when_last_watcher_leaves() {
        let mut f = Fanout::new();
        let sid = SessionId::new();
        let sid_key = sid.to_string();
        let (tx, _rx) = Fanout::make_channel();
        let uid = Uuid::new_v4();
        f.add(&sid_key, WatcherSink {
            user_id: uid,
            device_label: "only".into(),
            tx,
        });
        f.remove(&sid_key, uid, "only");
        assert!(!f.sinks.contains_key(&sid_key));
    }
}
