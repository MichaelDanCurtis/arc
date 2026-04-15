//! Per-session watcher presence tracking.
//!
//! Maps each session to the set of (user_id, device_label) pairs currently
//! connected, along with whether they may request control. `MirrorRegistry`
//! (Task 2.7) will consult `snapshot` on presence-change events to build the
//! watcher list broadcast to all subscribers.

use beacon_proto::ipc::WatcherInfo;
use std::collections::HashMap;
use uuid::Uuid;

/// In-memory registry of connected watchers per session.
pub struct PresenceTracker {
    /// session_id → (user_id, device_label) → can_drive
    watchers: HashMap<String, HashMap<(Uuid, String), bool>>,
}

impl PresenceTracker {
    /// Create an empty tracker.
    pub fn new() -> Self {
        Self { watchers: HashMap::new() }
    }

    /// Record a watcher's presence on a session. Idempotent: re-adding the
    /// same (user_id, device_label) pair updates the `can_drive` flag.
    pub fn add(&mut self, session_id: &str, user_id: Uuid, device_label: String, can_drive: bool) {
        self.watchers
            .entry(session_id.to_string())
            .or_default()
            .insert((user_id, device_label), can_drive);
    }

    /// Remove a watcher. When the last watcher for a session leaves, the
    /// session entry is dropped so `snapshot` returns an empty vec.
    pub fn remove(&mut self, session_id: &str, user_id: Uuid, device_label: &str) {
        if let Some(map) = self.watchers.get_mut(session_id) {
            map.remove(&(user_id, device_label.to_string()));
            if map.is_empty() {
                self.watchers.remove(session_id);
            }
        }
    }

    /// Project the current watcher set for a session into the wire format.
    /// Order is unspecified (HashMap iteration).
    pub fn snapshot(&self, session_id: &str) -> Vec<WatcherInfo> {
        self.watchers
            .get(session_id)
            .map(|map| {
                map.iter()
                    .map(|((uid, dev), can_drive)| WatcherInfo {
                        user_id: uid.to_string(),
                        device_label: dev.clone(),
                        can_drive: *can_drive,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Default for PresenceTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_remove_snapshot() {
        let mut p = PresenceTracker::new();
        let alice = Uuid::new_v4();
        p.add("s1", alice, "Mac".into(), true);
        p.add("s1", alice, "iPad".into(), true);
        assert_eq!(p.snapshot("s1").len(), 2);
        p.remove("s1", alice, "iPad");
        assert_eq!(p.snapshot("s1").len(), 1);
    }

    #[test]
    fn remove_last_watcher_drops_session_entry() {
        let mut p = PresenceTracker::new();
        let alice = Uuid::new_v4();
        p.add("s1", alice, "Mac".into(), true);
        p.remove("s1", alice, "Mac");
        assert!(p.snapshot("s1").is_empty());
        // Internal cleanup: session entry should be gone, not just empty map.
        assert!(!p.watchers.contains_key("s1"));
    }

    #[test]
    fn readd_updates_can_drive() {
        let mut p = PresenceTracker::new();
        let alice = Uuid::new_v4();
        p.add("s1", alice, "Mac".into(), false);
        p.add("s1", alice, "Mac".into(), true);
        let snap = p.snapshot("s1");
        assert_eq!(snap.len(), 1);
        assert!(snap[0].can_drive);
    }
}
