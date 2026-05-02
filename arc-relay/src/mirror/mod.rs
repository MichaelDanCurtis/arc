pub mod fanout;
pub mod handlers;
pub mod invitation;
pub mod lease;
pub mod presence;
pub mod scrollback;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use beacon_proto::envelope::ArcToClient;

use crate::mirror::{
    fanout::{BroadcastEmitter, Fanout},
    lease::LeaseManager,
    presence::PresenceTracker,
    scrollback::{ScrollbackRing, DEFAULT_MAX_BYTES, DEFAULT_MAX_AGE},
};

/// Per-session aggregate: lease + scrollback + presence + fanout, plus the host
/// input channel. Always held behind `Arc<Mutex<_>>` inside `MirrorRegistry`.
pub struct SessionMirror {
    pub lease: LeaseManager,
    pub scrollback: ScrollbackRing,
    pub presence: PresenceTracker,
    pub fanout: Fanout,
    /// Set by the host WebSocket handler once the host connects; `None` until then.
    pub host_input_tx: Option<mpsc::Sender<ArcToClient>>,
}

impl SessionMirror {
    pub fn new(owner: Uuid, eligible: Vec<Uuid>) -> Self {
        Self {
            lease: LeaseManager::new(owner, eligible),
            scrollback: ScrollbackRing::new(DEFAULT_MAX_BYTES, DEFAULT_MAX_AGE),
            presence: PresenceTracker::new(),
            fanout: Fanout::new(),
            host_input_tx: None,
        }
    }
}

/// Process-wide map from session_id to per-session mirror state. Cheaply
/// `Clone`able (`Arc`-backed) so it can live inside `AppState`.
#[derive(Clone)]
pub struct MirrorRegistry {
    inner: Arc<Mutex<HashMap<String, Arc<Mutex<SessionMirror>>>>>,
    /// Pluggable observers called on every [`MirrorRegistry::broadcast`]. Uses
    /// `std::sync::RwLock` so `add_emitter` is sync (called once at startup).
    emitters: Arc<RwLock<Vec<Arc<dyn BroadcastEmitter>>>>,
}

impl Default for MirrorRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            emitters: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl MirrorRegistry {
    /// Return the mirror for `session_id`, creating a fresh `SessionMirror`
    /// with the given owner + eligible set if no entry exists yet. Subsequent
    /// calls for the same session return the same `Arc` regardless of the
    /// `owner`/`eligible` args — they're only consulted on first init.
    ///
    /// Callers holding an `Arc<Mutex<SessionMirror>>` from a prior call retain
    /// that reference across `remove` + re-init — they do NOT see the
    /// re-initialized session. Release the Arc before re-initing if you need
    /// that.
    pub async fn get_or_init(
        &self,
        session_id: &str,
        owner: Uuid,
        eligible: Vec<Uuid>,
    ) -> Arc<Mutex<SessionMirror>> {
        let mut map = self.inner.lock().await;
        map.entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(SessionMirror::new(owner, eligible))))
            .clone()
    }

    /// Lookup without mutation; returns `None` if no session is registered.
    pub async fn get(&self, session_id: &str) -> Option<Arc<Mutex<SessionMirror>>> {
        self.inner.lock().await.get(session_id).cloned()
    }

    /// Drop the session entry. Any `Arc<Mutex<SessionMirror>>` still held by
    /// callers remains valid.
    pub async fn remove(&self, session_id: &str) {
        self.inner.lock().await.remove(session_id);
    }

    /// Register a [`BroadcastEmitter`] that will be called for every subsequent
    /// [`broadcast`] invocation. Call once at startup; sync so no async needed.
    pub fn add_emitter(&self, emitter: Arc<dyn BroadcastEmitter>) {
        self.emitters
            .write()
            .expect("MirrorRegistry emitters RwLock poisoned")
            .push(emitter);
    }

    /// Fan a frame out via the msgpack path (existing `Fanout::broadcast`) AND
    /// every registered jsonrpc [`BroadcastEmitter`].
    ///
    /// The msgpack path only fires when a session entry exists in the registry.
    /// Emitters always fire regardless, allowing the jsonrpc layer to receive
    /// frames for sessions it tracks independently.
    pub async fn broadcast(&self, session_id: &str, frame: ArcToClient) {
        // Msgpack path — requires session entry.
        if let Some(mirror) = self.get(session_id).await {
            let mut m = mirror.lock().await;
            m.fanout.broadcast(session_id, frame.clone());
        }
        // Jsonrpc emitter path — always fires.
        let emitters = self
            .emitters
            .read()
            .expect("MirrorRegistry emitters RwLock poisoned")
            .clone();
        for e in emitters {
            e.on_broadcast(session_id, &frame);
        }
    }
}
