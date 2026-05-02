//! JSON-RPC handlers for `arc.session.*` methods (Arc WebSocket spine).
//!
//! Implements the first sub-slice of the Arc WS jsonrpc cut-over per
//! `beacon-jsonrpc/MIGRATION.md` section 2:
//!
//! - `arc.session.attach`  — join mirrored session
//! - `arc.session.control` — claim write control
//! - `arc.session.release` — release write control
//! - `arc.session.participant` notification (relay → all)
//!
//! These handlers live alongside the existing msgpack `ClientToArc` /
//! `ArcToClient` envelope (see `arc-relay/src/ws/handler.rs`); they do not
//! replace it. Migration strategy: gate the handlers behind
//! [`beacon_features::FeatureFlag::JsonRpcSpine`] and let callers opt in
//! through a separate jsonrpc-aware WS endpoint in a later slice. Until that
//! cut-over lands the handlers operate against an in-rpc-layer
//! [`ArcSessionStore`] that mirrors the per-session lease/presence state the
//! existing `MirrorRegistry` already tracks.
//!
//! # Wire format — methods
//!
//! ```text
//! arc.session.attach   {"session_id":"<ulid>","token":"<jwt>"}      → {"ok":true}
//! arc.session.control  {"session_id":"<ulid>","requester_id":"..."} → {"ok":true}
//! arc.session.release  {"session_id":"<ulid>"}                      → {"ok":true}
//! ```
//!
//! # Wire format — notifications
//!
//! ```text
//! arc.session.participant {"session_id":"<ulid>","user_id":"...","joined":true}
//! ```
//!
//! `participant` fires when a client successfully attaches; the join/leave
//! shape leaves room for a future `arc.session.detach` to emit `joined:false`.

use std::collections::HashMap;
use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use beacon_jsonrpc::{BoxFuture, Notification, RpcDispatcher, RpcError, RpcHandler};
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex};

// ─────────────────────────────────────────────────────────────────────────────
// Session store
// ─────────────────────────────────────────────────────────────────────────────

/// Per-arc-session state held by the rpc layer.
///
/// Tracks the current write-control holder (if any) and a broadcast channel
/// on which `arc.session.participant` notifications are emitted. The
/// underlying mirroring state (scrollback, fanout, etc.) still lives in
/// `crate::mirror::MirrorRegistry`; this struct only mirrors the slice of
/// state the jsonrpc spine cares about.
#[derive(Debug)]
pub struct ArcSessionEntry {
    pub session_id: String,
    /// User id of the current write-control holder, if any.
    pub controller: Mutex<Option<String>>,
    /// Broadcast channel: each `String` is a JSON-encoded
    /// [`beacon_jsonrpc::Notification`].
    pub notif_tx: broadcast::Sender<String>,
}

/// Thread-safe map of `session_id` → [`ArcSessionEntry`].
#[derive(Clone, Default)]
pub struct ArcSessionStore {
    inner: Arc<Mutex<HashMap<String, Arc<ArcSessionEntry>>>>,
    notif_capacity: usize,
}

impl ArcSessionStore {
    pub fn new(notif_capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            notif_capacity,
        }
    }

    /// Look up an existing entry, or insert a fresh one with a new broadcast
    /// channel. Returns the entry.
    pub async fn get_or_create(&self, session_id: &str) -> Arc<ArcSessionEntry> {
        let mut guard = self.inner.lock().await;
        if let Some(e) = guard.get(session_id) {
            return e.clone();
        }
        let (notif_tx, _) = broadcast::channel::<String>(self.notif_capacity);
        let entry = Arc::new(ArcSessionEntry {
            session_id: session_id.to_string(),
            controller: Mutex::new(None),
            notif_tx,
        });
        guard.insert(session_id.to_string(), entry.clone());
        entry
    }

    pub async fn get(&self, session_id: &str) -> Option<Arc<ArcSessionEntry>> {
        self.inner.lock().await.get(session_id).cloned()
    }

    pub async fn session_count(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// Subscribe to a session's notification stream. Returns `None` when the
    /// session does not exist.
    pub async fn subscribe(&self, session_id: &str) -> Option<broadcast::Receiver<String>> {
        self.inner
            .lock()
            .await
            .get(session_id)
            .map(|e| e.notif_tx.subscribe())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Notification helpers
// ─────────────────────────────────────────────────────────────────────────────

fn send_notif(tx: &broadcast::Sender<String>, method: &'static str, params: Value) {
    let n = Notification::new(method, params);
    let _ = tx.send(serde_json::to_string(&n).unwrap_or_default());
}

/// Emit `arc.session.participant`.
pub fn emit_participant(entry: &ArcSessionEntry, user_id: &str, joined: bool) {
    send_notif(
        &entry.notif_tx,
        "arc.session.participant",
        json!({
            "session_id": entry.session_id,
            "user_id": user_id,
            "joined": joined,
        }),
    );
}

/// Emit `arc.mirror.frame` with base64-encoded raw bytes.
///
/// Direction: relay→watcher. Carries a single PTY output frame from the
/// session's host to every subscribed watcher. Pairs with the existing
/// msgpack `ArcToClient::SessionOutput` envelope; future cut-over slices
/// will bridge `Fanout::broadcast` into this emit fn instead.
pub fn emit_mirror_frame(entry: &ArcSessionEntry, data: &[u8]) {
    send_notif(
        &entry.notif_tx,
        "arc.mirror.frame",
        json!({
            "session_id": entry.session_id,
            "data": BASE64.encode(data),
        }),
    );
}

/// Emit `arc.mirror.control_grant`.
///
/// Direction: relay→all. Fires when write control transfers to a new holder.
pub fn emit_mirror_control_grant(entry: &ArcSessionEntry, holder_id: &str) {
    send_notif(
        &entry.notif_tx,
        "arc.mirror.control_grant",
        json!({
            "session_id": entry.session_id,
            "holder_id": holder_id,
        }),
    );
}

/// Emit `arc.mirror.control_revoke`.
///
/// Direction: relay→all. Fires when the current holder releases write
/// control with no successor.
pub fn emit_mirror_control_revoke(entry: &ArcSessionEntry) {
    send_notif(
        &entry.notif_tx,
        "arc.mirror.control_revoke",
        json!({
            "session_id": entry.session_id,
        }),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn require_str<'a>(params: &'a Value, field: &str) -> Result<&'a str, RpcError> {
    match params.get(field).and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => Err(RpcError::invalid_params(format!(
            "missing or empty '{field}' field"
        ))),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.session.attach
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.session.attach`.
///
/// Validates the wire shape, allocates an [`ArcSessionEntry`] for the session
/// if one does not already exist, derives a stable `user_id` from the supplied
/// token, and emits an `arc.session.participant {joined:true}` notification.
/// The actual auth check (verifying the JWT against the relay's secret) is
/// the caller's responsibility — this handler trusts that a higher-level
/// dispatch layer has already validated the token before invoking it. That
/// matches the existing msgpack path where the WS upgrade handler verifies
/// the token before any frame is dispatched.
pub struct ArcSessionAttachHandler {
    store: ArcSessionStore,
    /// Function that maps a token string to a stable user_id. Tests override
    /// this to keep ids deterministic; production wires the real JWT decode.
    user_resolver: Arc<dyn Fn(&str) -> String + Send + Sync>,
}

impl ArcSessionAttachHandler {
    pub fn new(store: ArcSessionStore) -> Self {
        Self {
            store,
            user_resolver: Arc::new(default_user_resolver),
        }
    }

    pub fn with_user_resolver<F>(mut self, resolver: F) -> Self
    where
        F: Fn(&str) -> String + Send + Sync + 'static,
    {
        self.user_resolver = Arc::new(resolver);
        self
    }
}

/// Default token-to-user mapping: hash the token bytes into a short hex id.
///
/// Avoids pulling in jsonwebtoken dep at this layer; production callers should
/// override via [`ArcSessionAttachHandler::with_user_resolver`] to plug in a
/// real JWT decode against the relay's `jwt_secret`.
fn default_user_resolver(token: &str) -> String {
    let mut acc: u64 = 0xcbf29ce484222325;
    for b in token.bytes() {
        acc ^= b as u64;
        acc = acc.wrapping_mul(0x100000001b3);
    }
    format!("u_{acc:x}")
}

impl RpcHandler for ArcSessionAttachHandler {
    fn method_name(&self) -> &'static str {
        "arc.session.attach"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();
            let token = require_str(&params, "token")?;
            let user_id = (self.user_resolver)(token);

            let entry = self.store.get_or_create(&session_id).await;
            emit_participant(&entry, &user_id, true);
            Ok(json!({"ok": true}))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.session.control
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.session.control`.
///
/// Records the requester as the new write-control holder for the session. If
/// no session entry exists this fails with `server_error` — the caller must
/// have attached first.
pub struct ArcSessionControlHandler {
    store: ArcSessionStore,
}

impl ArcSessionControlHandler {
    pub fn new(store: ArcSessionStore) -> Self {
        Self { store }
    }
}

impl RpcHandler for ArcSessionControlHandler {
    fn method_name(&self) -> &'static str {
        "arc.session.control"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();
            let requester_id = require_str(&params, "requester_id")?.to_string();

            let Some(entry) = self.store.get(&session_id).await else {
                return Err(RpcError::server_error(format!(
                    "arc session not found: {session_id}"
                )));
            };
            *entry.controller.lock().await = Some(requester_id.clone());
            emit_mirror_control_grant(&entry, &requester_id);
            Ok(json!({"ok": true}))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.session.release
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.session.release`.
///
/// Clears the write-control holder for the session. No-op when no controller
/// is currently set. Fails with `server_error` when the session entry is
/// missing.
pub struct ArcSessionReleaseHandler {
    store: ArcSessionStore,
}

impl ArcSessionReleaseHandler {
    pub fn new(store: ArcSessionStore) -> Self {
        Self { store }
    }
}

impl RpcHandler for ArcSessionReleaseHandler {
    fn method_name(&self) -> &'static str {
        "arc.session.release"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();

            let Some(entry) = self.store.get(&session_id).await else {
                return Err(RpcError::server_error(format!(
                    "arc session not found: {session_id}"
                )));
            };
            let had_holder = entry.controller.lock().await.take().is_some();
            if had_holder {
                emit_mirror_control_revoke(&entry);
            }
            Ok(json!({"ok": true}))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ArcMirrorEmitter — bridges MirrorRegistry::broadcast to arc.mirror.* notifs
// ─────────────────────────────────────────────────────────────────────────────

/// A [`crate::mirror::fanout::BroadcastEmitter`] that translates `ArcToClient`
/// frames into `arc.mirror.*` JSON-RPC notifications on the matching
/// [`ArcSessionEntry`].
///
/// `on_broadcast` is sync; `ArcSessionStore::get` is async. We spawn a task
/// to bridge the gap.
pub struct ArcMirrorEmitter {
    pub store: ArcSessionStore,
}

impl ArcMirrorEmitter {
    pub fn new(store: ArcSessionStore) -> Self {
        Self { store }
    }
}

impl crate::mirror::fanout::BroadcastEmitter for ArcMirrorEmitter {
    fn on_broadcast(&self, session_id: &str, frame: &beacon_proto::envelope::ArcToClient) {
        let store = self.store.clone();
        let session_id = session_id.to_string();
        let frame = frame.clone();
        tokio::spawn(async move {
            let Some(entry) = store.get(&session_id).await else {
                return;
            };
            match &frame {
                beacon_proto::envelope::ArcToClient::SessionOutput { bytes, .. } => {
                    emit_mirror_frame(&entry, bytes);
                }
                beacon_proto::envelope::ArcToClient::LeaseChanged {
                    driver: Some(d), ..
                } => {
                    emit_mirror_control_grant(&entry, &d.user_id);
                }
                beacon_proto::envelope::ArcToClient::LeaseChanged {
                    driver: None, ..
                } => {
                    emit_mirror_control_revoke(&entry);
                }
                _ => {}
            }
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Registration helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Build a fresh dispatcher with all three arc.session.* handlers registered.
/// Mainly used in tests; production wires through [`register_if_enabled`].
pub fn build_arc_session_dispatcher(store: ArcSessionStore) -> RpcDispatcher {
    let mut d = RpcDispatcher::new();
    d.register(ArcSessionAttachHandler::new(store.clone()));
    d.register(ArcSessionControlHandler::new(store.clone()));
    d.register(ArcSessionReleaseHandler::new(store));
    d
}

/// Register all three arc.session.* handlers with `dispatcher` if
/// `JsonRpcSpine` is on. Returns `true` when handlers were registered.
pub fn register_if_enabled(dispatcher: &mut RpcDispatcher, store: ArcSessionStore) -> bool {
    if beacon_features::is_enabled(beacon_features::FeatureFlag::JsonRpcSpine) {
        dispatcher.try_register(ArcSessionAttachHandler::new(store.clone()));
        dispatcher.try_register(ArcSessionControlHandler::new(store.clone()));
        dispatcher.try_register(ArcSessionReleaseHandler::new(store));
        true
    } else {
        false
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use beacon_jsonrpc::Request;

    fn make_dispatcher() -> (ArcSessionStore, RpcDispatcher) {
        let store = ArcSessionStore::new(64);
        let mut d = RpcDispatcher::new();
        d.register(
            ArcSessionAttachHandler::new(store.clone())
                .with_user_resolver(|_token| "alice".into()),
        );
        d.register(ArcSessionControlHandler::new(store.clone()));
        d.register(ArcSessionReleaseHandler::new(store.clone()));
        (store, d)
    }

    #[tokio::test]
    async fn attach_creates_entry_and_returns_ok() {
        let (store, d) = make_dispatcher();
        let req = Request::new(
            "arc.session.attach",
            json!({"session_id": "s1", "token": "tok"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());
        assert_eq!(resp.result().unwrap()["ok"], true);
        assert_eq!(store.session_count().await, 1);
        assert!(store.get("s1").await.is_some());
    }

    #[tokio::test]
    async fn attach_emits_participant_joined_true() {
        let (store, d) = make_dispatcher();
        // Pre-create + subscribe so we don't race the handler.
        let entry = store.get_or_create("s1").await;
        let mut rx = entry.notif_tx.subscribe();

        let req = Request::new(
            "arc.session.attach",
            json!({"session_id": "s1", "token": "tok"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success());

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.session.participant");
        assert_eq!(v["params"]["session_id"], "s1");
        assert_eq!(v["params"]["user_id"], "alice");
        assert_eq!(v["params"]["joined"], true);
    }

    #[tokio::test]
    async fn attach_missing_session_id_invalid_params() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.session.attach",
            json!({"token": "tok"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    #[tokio::test]
    async fn attach_missing_token_invalid_params() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.session.attach",
            json!({"session_id": "s1"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    #[tokio::test]
    async fn control_records_holder() {
        let (store, d) = make_dispatcher();
        store.get_or_create("s1").await;

        let req = Request::new(
            "arc.session.control",
            json!({"session_id": "s1", "requester_id": "bob"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());

        let entry = store.get("s1").await.unwrap();
        assert_eq!(entry.controller.lock().await.clone(), Some("bob".into()));
    }

    #[tokio::test]
    async fn control_unknown_session_server_error() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.session.control",
            json!({"session_id": "missing", "requester_id": "bob"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        let err = resp.err().unwrap();
        assert!(err.message.contains("not found"), "{err:?}");
    }

    #[tokio::test]
    async fn control_missing_requester_invalid_params() {
        let (store, d) = make_dispatcher();
        store.get_or_create("s1").await;
        let req = Request::new(
            "arc.session.control",
            json!({"session_id": "s1"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    #[tokio::test]
    async fn release_clears_holder() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s1").await;
        *entry.controller.lock().await = Some("bob".into());

        let req = Request::new(
            "arc.session.release",
            json!({"session_id": "s1"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());

        assert!(entry.controller.lock().await.is_none());
    }

    #[tokio::test]
    async fn release_unknown_session_server_error() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.session.release",
            json!({"session_id": "ghost"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.err().unwrap().message.contains("not found"));
    }

    #[tokio::test]
    async fn release_idempotent_when_no_holder() {
        let (store, d) = make_dispatcher();
        store.get_or_create("s1").await;
        let req = Request::new(
            "arc.session.release",
            json!({"session_id": "s1"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success());
    }

    #[tokio::test]
    async fn build_arc_session_dispatcher_registers_three_methods() {
        let store = ArcSessionStore::new(8);
        let d = build_arc_session_dispatcher(store);
        let methods = d.registered_methods();
        assert_eq!(
            methods,
            vec![
                "arc.session.attach",
                "arc.session.control",
                "arc.session.release",
            ]
        );
    }

    #[test]
    fn default_user_resolver_stable() {
        let a = default_user_resolver("token-xyz");
        let b = default_user_resolver("token-xyz");
        let c = default_user_resolver("different");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("u_"));
    }

    #[tokio::test]
    async fn control_emits_mirror_grant() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s1").await;
        let mut rx = entry.notif_tx.subscribe();

        let req = Request::new(
            "arc.session.control",
            json!({"session_id": "s1", "requester_id": "bob"}),
            1i64,
        );
        assert!(d.dispatch(req).await.is_success());

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.control_grant");
        assert_eq!(v["params"]["session_id"], "s1");
        assert_eq!(v["params"]["holder_id"], "bob");
    }

    #[tokio::test]
    async fn release_emits_mirror_revoke_when_holder_was_set() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s1").await;
        *entry.controller.lock().await = Some("bob".into());
        let mut rx = entry.notif_tx.subscribe();

        let req = Request::new(
            "arc.session.release",
            json!({"session_id": "s1"}),
            1i64,
        );
        assert!(d.dispatch(req).await.is_success());

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.control_revoke");
        assert_eq!(v["params"]["session_id"], "s1");
    }

    #[tokio::test]
    async fn release_no_revoke_when_no_holder() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s1").await;
        let mut rx = entry.notif_tx.subscribe();

        let req = Request::new(
            "arc.session.release",
            json!({"session_id": "s1"}),
            1i64,
        );
        assert!(d.dispatch(req).await.is_success());

        // No revoke notif since there was no holder to clear.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn emit_mirror_frame_base64_round_trip() {
        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s1").await;
        let mut rx = entry.notif_tx.subscribe();

        let payload = b"\x1b[1;31mhello\x1b[0m";
        emit_mirror_frame(&entry, payload);

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.frame");
        assert_eq!(v["params"]["session_id"], "s1");
        let b64 = v["params"]["data"].as_str().unwrap();
        let decoded = BASE64.decode(b64).unwrap();
        assert_eq!(decoded, payload);
    }
}
