use dashmap::DashMap;
use tokio::sync::{broadcast, mpsc, oneshot};
use uuid::Uuid;

/// State for one relay session.
pub struct SessionRelay {
    /// Send encoded messages to the host connection.
    pub host_tx: mpsc::Sender<Vec<u8>>,
    /// Broadcast encoded ArcToClient messages to all attached clients.
    pub broadcast: broadcast::Sender<Vec<u8>>,
    /// User who owns/registered this session.
    pub owner_id: Uuid,
    /// Human-readable session name.
    pub name: String,
    /// Current session state.
    pub state: SessionState,
    /// Terminal dimensions.
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Active,
    Suspended,
}

/// Central router managing all active relay sessions.
///
/// Thread-safe — uses DashMap for lock-free concurrent access.
pub struct RelayRouter {
    sessions: DashMap<String, SessionRelay>,
    /// Pending shadow diff response waiters, keyed by request_id.
    shadow_diff_waiters: DashMap<String, oneshot::Sender<String>>,
}

impl RelayRouter {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            shadow_diff_waiters: DashMap::new(),
        }
    }

    /// Register a new session. Returns a broadcast receiver the caller (host handler)
    /// can optionally use. The `host_tx` allows sending messages back to the host.
    pub fn register_session(
        &self,
        session_id: String,
        host_tx: mpsc::Sender<Vec<u8>>,
        owner_id: Uuid,
        name: String,
    ) -> broadcast::Receiver<Vec<u8>> {
        let (broadcast_tx, broadcast_rx) = broadcast::channel(1024);
        self.sessions.insert(
            session_id,
            SessionRelay {
                host_tx,
                broadcast: broadcast_tx,
                owner_id,
                name,
                state: SessionState::Active,
                cols: 80,
                rows: 24,
            },
        );
        broadcast_rx
    }

    /// Remove a session from the router.
    pub fn unregister_session(&self, session_id: &str) -> bool {
        self.sessions.remove(session_id).is_some()
    }

    /// Broadcast encoded bytes to all clients attached to this session.
    /// Returns false if the session doesn't exist.
    pub fn broadcast(&self, session_id: &str, data: Vec<u8>) -> bool {
        if let Some(session) = self.sessions.get(session_id) {
            // broadcast::send returns Err only if there are no receivers, which is fine
            let _ = session.broadcast.send(data);
            true
        } else {
            false
        }
    }

    /// Subscribe to a session's broadcast channel (for client attach).
    /// Returns None if the session doesn't exist.
    pub fn subscribe(&self, session_id: &str) -> Option<broadcast::Receiver<Vec<u8>>> {
        self.sessions
            .get(session_id)
            .map(|s| s.broadcast.subscribe())
    }

    /// Send bytes to the host's channel (for forwarding client input).
    /// Returns false if session doesn't exist or host channel is full/closed.
    pub async fn send_to_host(&self, session_id: &str, data: Vec<u8>) -> bool {
        if let Some(session) = self.sessions.get(session_id) {
            session.host_tx.send(data).await.is_ok()
        } else {
            false
        }
    }

    /// Update session terminal dimensions.
    pub fn update_size(&self, session_id: &str, cols: u16, rows: u16) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.cols = cols;
            session.rows = rows;
        }
    }

    /// Mark a session as suspended (host disconnected).
    pub fn suspend_session(&self, session_id: &str) {
        if let Some(mut session) = self.sessions.get_mut(session_id) {
            session.state = SessionState::Suspended;
        }
    }

    /// Get session info (for REST API).
    pub fn get_session_info(&self, session_id: &str) -> Option<SessionInfo> {
        self.sessions.get(session_id).map(|s| SessionInfo {
            id: session_id.to_string(),
            name: s.name.clone(),
            owner_id: s.owner_id,
            state: s.state,
            cols: s.cols,
            rows: s.rows,
        })
    }

    /// Register a oneshot sender to await a shadow diff response.
    pub fn register_shadow_diff_waiter(&self, request_id: &str, tx: oneshot::Sender<String>) {
        self.shadow_diff_waiters.insert(request_id.to_string(), tx);
    }

    /// Remove a pending shadow diff waiter (e.g. on timeout).
    pub fn remove_shadow_diff_waiter(&self, request_id: &str) {
        self.shadow_diff_waiters.remove(request_id);
    }

    /// Resolve a pending shadow diff request with the given diff text.
    pub fn resolve_shadow_diff(&self, request_id: &str, diff: String) -> bool {
        if let Some((_, tx)) = self.shadow_diff_waiters.remove(request_id) {
            tx.send(diff).is_ok()
        } else {
            false
        }
    }

    /// List all active sessions.
    #[allow(dead_code)]
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .iter()
            .map(|entry| SessionInfo {
                id: entry.key().clone(),
                name: entry.value().name.clone(),
                owner_id: entry.value().owner_id,
                state: entry.value().state,
                cols: entry.value().cols,
                rows: entry.value().rows,
            })
            .collect()
    }
}

impl Default for RelayRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub owner_id: Uuid,
    pub state: SessionState,
    pub cols: u16,
    pub rows: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_and_list() {
        let router = RelayRouter::new();
        let (host_tx, _host_rx) = mpsc::channel(16);
        let owner = Uuid::new_v4();
        router.register_session("s1".into(), host_tx, owner, "test".into());

        let sessions = router.list_sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "s1");
        assert_eq!(sessions[0].owner_id, owner);
    }

    #[tokio::test]
    async fn broadcast_to_subscriber() {
        let router = RelayRouter::new();
        let (host_tx, _) = mpsc::channel(16);
        router.register_session("s1".into(), host_tx, Uuid::new_v4(), "test".into());

        let mut rx = router.subscribe("s1").unwrap();
        router.broadcast("s1", b"hello".to_vec());

        let data = rx.recv().await.unwrap();
        assert_eq!(data, b"hello");
    }

    #[tokio::test]
    async fn send_to_host() {
        let router = RelayRouter::new();
        let (host_tx, mut host_rx) = mpsc::channel(16);
        router.register_session("s1".into(), host_tx, Uuid::new_v4(), "test".into());

        assert!(router.send_to_host("s1", b"input".to_vec()).await);
        let data = host_rx.recv().await.unwrap();
        assert_eq!(data, b"input");
    }

    #[tokio::test]
    async fn unregister_removes_session() {
        let router = RelayRouter::new();
        let (host_tx, _) = mpsc::channel(16);
        router.register_session("s1".into(), host_tx, Uuid::new_v4(), "test".into());

        assert!(router.unregister_session("s1"));
        assert!(router.subscribe("s1").is_none());
    }

    #[tokio::test]
    async fn subscribe_nonexistent_returns_none() {
        let router = RelayRouter::new();
        assert!(router.subscribe("nope").is_none());
    }

    #[tokio::test]
    async fn suspend_session_updates_state() {
        let router = RelayRouter::new();
        let (host_tx, _) = mpsc::channel(16);
        router.register_session("s1".into(), host_tx, Uuid::new_v4(), "test".into());

        router.suspend_session("s1");
        let info = router.get_session_info("s1").unwrap();
        assert_eq!(info.state, SessionState::Suspended);
    }

    #[tokio::test]
    async fn update_size() {
        let router = RelayRouter::new();
        let (host_tx, _) = mpsc::channel(16);
        router.register_session("s1".into(), host_tx, Uuid::new_v4(), "test".into());

        router.update_size("s1", 120, 40);
        let info = router.get_session_info("s1").unwrap();
        assert_eq!(info.cols, 120);
        assert_eq!(info.rows, 40);
    }

    #[tokio::test]
    async fn broadcast_nonexistent_returns_false() {
        let router = RelayRouter::new();
        assert!(!router.broadcast("nope", b"data".to_vec()));
    }
}
