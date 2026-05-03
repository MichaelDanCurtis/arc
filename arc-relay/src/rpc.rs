//! JSON-RPC handlers for `arc.session.*` methods (Arc WebSocket spine).
//!
//! Implements the first sub-slice of the Arc WS jsonrpc cut-over per
//! `beacon-jsonrpc/MIGRATION.md` section 2:
//!
//! - `arc.session.attach`     — join mirrored session
//! - `arc.session.detach`     — leave mirrored session
//! - `arc.session.subscribe`  — read-only mirror handshake
//! - `arc.session.unsubscribe` — leave mirror handshake
//! - `arc.session.control`    — claim write control
//! - `arc.session.release`    — release write control
//! - `arc.session.input`      — driver-only PTY write
//! - `arc.session.resize`     — driver-only PTY resize
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
//! arc.session.attach       {"session_id":"<ulid>","token":"<jwt>"}                                    → {"ok":true}
//! arc.session.detach       {"session_id":"<ulid>","token":"<jwt>"}                                    → {"ok":true}
//! arc.session.subscribe    {"session_id":"<ulid>","token":"<jwt>"}                                    → {"ok":true}
//! arc.session.unsubscribe  {"session_id":"<ulid>","token":"<jwt>"}                                    → {"ok":true}
//! arc.session.control      {"session_id":"<ulid>","requester_id":"..."}                               → {"ok":true}
//! arc.session.release      {"session_id":"<ulid>"}                                                    → {"ok":true}
//! arc.session.input        {"session_id":"<ulid>","requester_id":"...","data":"<base64>"}             → {"ok":true}
//! arc.session.resize       {"session_id":"<ulid>","requester_id":"...","cols":<u32>,"rows":<u32>}     → {"ok":true}
//! ```
//!
//! # Wire format — notifications
//!
//! ```text
//! arc.session.participant         {"session_id":"<ulid>","user_id":"...","joined":bool}                               relay→all
//! arc.session.control_requested  {"session_id":"<ulid>","request_id":"...","from_user":"...","from_device":"..."}    relay→holder (Phase C)
//! arc.session.gone               {"session_id":"<ulid>","reason":"..."}                                               relay→all (host uplink lost)
//! arc.session.revoked            {"session_id":"<ulid>","reason":"..."}                                               relay→watcher (kicked by owner)
//! arc.host.input                 {"session_id":"<ulid>","data":"<base64>"}                                            relay→host (Phase D consumer)
//! arc.host.resize_request        {"session_id":"<ulid>","cols":<u32>,"rows":<u32>}                                    relay→host (Phase D consumer)
//! arc.mirror.discontinuity       {"session_id":"<ulid>","last_seq":<u64>}                                             relay→watcher (gap signal)
//! arc.mirror.block_start          {"session_id":"<ulid>","header":<BlockHeader-json>}                                  relay→watcher
//! arc.mirror.block_chunk          {"session_id":"<ulid>","block_id":"<ulid>","seq":<u64>,"data":"<base64>"}             relay→watcher
//! arc.mirror.block_end            {"session_id":"<ulid>","footer":<BlockFooter-json>}                                  relay→watcher
//! arc.mirror.replay_begin         {"session_id":"<ulid>","total_blocks":<u32>}                                          relay→watcher
//! arc.mirror.replay_block         {"session_id":"<ulid>","header":<BlockHeader-json>,"output":"<base64>","footer":<BlockFooter-json|null>}  relay→watcher
//! arc.mirror.replay_end           {"session_id":"<ulid>"}                                                                relay→watcher
//! ```
//!
//! `participant` fires on both attach (`joined:true`) and detach (`joined:false`).

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

/// Emit `arc.mirror.discontinuity` — relay→watcher when the broadcast missed
/// frames and the watcher must re-replay or accept the gap.
pub fn emit_mirror_discontinuity(entry: &ArcSessionEntry, last_seq: u64) {
    send_notif(
        &entry.notif_tx,
        "arc.mirror.discontinuity",
        json!({
            "session_id": entry.session_id,
            "last_seq": last_seq,
        }),
    );
}

/// Emit `arc.mirror.block_start` — relay→watcher when a new command block
/// begins. Carries the full `BlockHeader` envelope (id, command, cwd, env,
/// host_info, etc.) per the spec-locked wire shape from Task 1.
///
/// Direction: relay→watcher. Pairs with the existing msgpack
/// `ArcToClient::BlockStart` envelope; future cut-over slices will fan
/// `BlockEvent::Started` from `beacon-blocks::BlockDetector` into this
/// emit fn instead of msgpack.
pub fn emit_mirror_block_start(entry: &ArcSessionEntry, header: &beacon_model::BlockHeader) {
    let header_value = serde_json::to_value(header).unwrap_or(Value::Null);
    send_notif(
        &entry.notif_tx,
        "arc.mirror.block_start",
        json!({
            "session_id": entry.session_id,
            "header": header_value,
        }),
    );
}

/// Emit `arc.mirror.block_chunk` — relay→watcher carrying one base64-encoded
/// chunk of block output bytes, tagged with `seq` for gap-detection. The
/// `block_id` ties it to the parent `arc.mirror.block_start`.
///
/// Direction: relay→watcher. Pairs with msgpack `ArcToClient::BlockOutputChunk`.
pub fn emit_mirror_block_chunk(
    entry: &ArcSessionEntry,
    block_id: &beacon_model::BlockId,
    seq: u64,
    data: &[u8],
) {
    send_notif(
        &entry.notif_tx,
        "arc.mirror.block_chunk",
        json!({
            "session_id": entry.session_id,
            "block_id": block_id.to_string(),
            "seq": seq,
            "data": BASE64.encode(data),
        }),
    );
}

/// Emit `arc.mirror.block_end` — relay→watcher when a command block finishes.
/// Carries the full `BlockFooter` envelope (id, exit_code, ended_at,
/// shadow_git_ref). `exit_code: null` indicates signal termination.
///
/// Direction: relay→watcher. Pairs with msgpack `ArcToClient::BlockEnd`.
pub fn emit_mirror_block_end(entry: &ArcSessionEntry, footer: &beacon_model::BlockFooter) {
    let footer_value = serde_json::to_value(footer).unwrap_or(Value::Null);
    send_notif(
        &entry.notif_tx,
        "arc.mirror.block_end",
        json!({
            "session_id": entry.session_id,
            "footer": footer_value,
        }),
    );
}

/// Emit `arc.mirror.replay_begin` — relay→watcher signalling the start of a
/// scrollback replay. `total_blocks` is the count of historical blocks the
/// watcher should expect via `arc.mirror.replay_block` before
/// `arc.mirror.replay_end`.
///
/// Direction: relay→watcher. Pairs with msgpack `ArcToClient::ReplayBegin`.
pub fn emit_mirror_replay_begin(entry: &ArcSessionEntry, total_blocks: u32) {
    send_notif(
        &entry.notif_tx,
        "arc.mirror.replay_begin",
        json!({
            "session_id": entry.session_id,
            "total_blocks": total_blocks,
        }),
    );
}

/// Emit `arc.mirror.replay_block` — relay→watcher carrying one historical
/// block during scrollback replay. The `Block` is flattened: `header` and
/// `footer` are JSON objects (footer is `null` for a still-running block);
/// `output` is base64-encoded raw bytes per the `arc.mirror.frame` precedent.
///
/// Direction: relay→watcher. Pairs with msgpack `ArcToClient::ReplayBlock`.
pub fn emit_mirror_replay_block(entry: &ArcSessionEntry, block: &beacon_model::Block) {
    let header_value = serde_json::to_value(&block.header).unwrap_or(Value::Null);
    let footer_value = block
        .footer
        .as_ref()
        .map(|f| serde_json::to_value(f).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);
    send_notif(
        &entry.notif_tx,
        "arc.mirror.replay_block",
        json!({
            "session_id": entry.session_id,
            "header": header_value,
            "output": BASE64.encode(&block.output),
            "footer": footer_value,
        }),
    );
}

/// Emit `arc.mirror.replay_end` — relay→watcher signalling that scrollback
/// replay has finished and subsequent `arc.mirror.frame` /
/// `arc.mirror.block_*` notifications are live.
///
/// Direction: relay→watcher. Pairs with msgpack `ArcToClient::ReplayEnd`.
pub fn emit_mirror_replay_end(entry: &ArcSessionEntry) {
    send_notif(
        &entry.notif_tx,
        "arc.mirror.replay_end",
        json!({
            "session_id": entry.session_id,
        }),
    );
}

/// Emit `arc.host.input` — driver→host PTY bytes (forwarded to the host
/// process by the host-side jsonrpc consumer in Phase D).
pub fn emit_host_input(entry: &ArcSessionEntry, data: &[u8]) {
    send_notif(
        &entry.notif_tx,
        "arc.host.input",
        json!({
            "session_id": entry.session_id,
            "data": BASE64.encode(data),
        }),
    );
}

/// Emit `arc.host.resize_request` — driver→host PTY resize (forwarded to the
/// host process by the host-side jsonrpc consumer in Phase D).
pub fn emit_host_resize_request(entry: &ArcSessionEntry, cols: u32, rows: u32) {
    send_notif(
        &entry.notif_tx,
        "arc.host.resize_request",
        json!({
            "session_id": entry.session_id,
            "cols": cols,
            "rows": rows,
        }),
    );
}

/// Emit `arc.session.control_requested` — relay→watcher (sent to the current
/// holder) when a watcher requests write control.
pub fn emit_control_requested(
    entry: &ArcSessionEntry,
    request_id: &str,
    from_user: &str,
    from_device: &str,
) {
    send_notif(
        &entry.notif_tx,
        "arc.session.control_requested",
        json!({
            "session_id": entry.session_id,
            "request_id": request_id,
            "from_user": from_user,
            "from_device": from_device,
        }),
    );
}

/// Emit `arc.session.gone` — relay→all watchers when the host process drops
/// the session (uplink lost, host shutdown, etc.).
pub fn emit_session_gone(entry: &ArcSessionEntry, reason: &str) {
    send_notif(
        &entry.notif_tx,
        "arc.session.gone",
        json!({
            "session_id": entry.session_id,
            "reason": reason,
        }),
    );
}

/// Emit `arc.session.revoked` — relay→watcher when the session owner kicks
/// this watcher out.
///
/// Note: this is distinct from `arc.mirror.control_revoke`, which clears the
/// write-control holder slot (released voluntarily). `arc.session.revoked`
/// means the watcher's access has been terminated by the owner entirely.
pub fn emit_session_revoked(entry: &ArcSessionEntry, reason: &str) {
    send_notif(
        &entry.notif_tx,
        "arc.session.revoked",
        json!({
            "session_id": entry.session_id,
            "reason": reason,
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

fn require_u32(params: &Value, field: &str) -> Result<u32, RpcError> {
    match params.get(field).and_then(|v| v.as_u64()) {
        Some(n) if n > 0 && n <= u32::MAX as u64 => Ok(n as u32),
        _ => Err(RpcError::invalid_params(format!(
            "missing or invalid '{field}' field (need positive u32)"
        ))),
    }
}

/// Application error -32000 returned when a write-gated method is called by
/// someone other than the current write-control holder. Carries
/// `data.reason: "not_controller"` so callers can branch on the cause.
fn not_controller_err() -> RpcError {
    RpcError::server_error("not_controller").with_data(json!({"reason": "not_controller"}))
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
// arc.session.detach
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.session.detach`.
///
/// Symmetric counterpart to `arc.session.attach`: validates the wire shape,
/// looks up the session entry (errors if missing), derives a stable `user_id`
/// from the token, and emits an `arc.session.participant {joined:false}`
/// notification. Does not allocate a fresh entry — detaching a session that
/// was never attached is treated as a server error.
pub struct ArcSessionDetachHandler {
    store: ArcSessionStore,
    user_resolver: Arc<dyn Fn(&str) -> String + Send + Sync>,
}

impl ArcSessionDetachHandler {
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

impl RpcHandler for ArcSessionDetachHandler {
    fn method_name(&self) -> &'static str {
        "arc.session.detach"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();
            let token = require_str(&params, "token")?;
            let user_id = (self.user_resolver)(token);

            let Some(entry) = self.store.get(&session_id).await else {
                return Err(RpcError::server_error(format!(
                    "arc session not found: {session_id}"
                )));
            };
            emit_participant(&entry, &user_id, false);
            Ok(json!({"ok": true}))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.session.subscribe
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.session.subscribe`.
///
/// Read-only mirror-mode handshake. Allocates an [`ArcSessionEntry`] for the
/// session if one does not already exist (mirrors attach), but does not affect
/// the controller field (read-only mirror, no write claim). Emits
/// `arc.session.participant {joined:true}` so existing watchers see the new
/// presence.
pub struct ArcSessionSubscribeHandler {
    store: ArcSessionStore,
    user_resolver: Arc<dyn Fn(&str) -> String + Send + Sync>,
}

impl ArcSessionSubscribeHandler {
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

impl RpcHandler for ArcSessionSubscribeHandler {
    fn method_name(&self) -> &'static str {
        "arc.session.subscribe"
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
// arc.session.unsubscribe
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.session.unsubscribe`.
///
/// Counterpart to `arc.session.subscribe` for the leave path. Unlike
/// subscribe (which allocates via `get_or_create`), this looks up the
/// session entry and errors if missing — a client must subscribe before it
/// can unsubscribe. Emits `arc.session.participant {joined:false}` so
/// existing watchers see the leave. Does not affect the controller field —
/// read-only mirror leave.
pub struct ArcSessionUnsubscribeHandler {
    store: ArcSessionStore,
    user_resolver: Arc<dyn Fn(&str) -> String + Send + Sync>,
}

impl ArcSessionUnsubscribeHandler {
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

impl RpcHandler for ArcSessionUnsubscribeHandler {
    fn method_name(&self) -> &'static str {
        "arc.session.unsubscribe"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();
            let token = require_str(&params, "token")?;
            let user_id = (self.user_resolver)(token);

            let Some(entry) = self.store.get(&session_id).await else {
                return Err(RpcError::server_error(format!(
                    "arc session not found: {session_id}"
                )));
            };
            emit_participant(&entry, &user_id, false);
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
// arc.session.input
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.session.input`.
///
/// Driver-only: gated on `requester_id` matching the current write-control
/// holder. Emits an `arc.host.input` notification carrying the base64-decoded
/// payload to the broadcast channel; the host-side consumer (lands in Phase
/// D) is what actually forwards the bytes to the PTY. Until that consumer
/// exists this handler functions as a wire stub plus an audit fanout.
pub struct ArcSessionInputHandler {
    store: ArcSessionStore,
}

impl ArcSessionInputHandler {
    pub fn new(store: ArcSessionStore) -> Self {
        Self { store }
    }
}

impl RpcHandler for ArcSessionInputHandler {
    fn method_name(&self) -> &'static str {
        "arc.session.input"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();
            let requester_id = require_str(&params, "requester_id")?.to_string();
            let data_b64 = require_str(&params, "data")?;
            let data = BASE64
                .decode(data_b64)
                .map_err(|e| RpcError::invalid_params(format!("data not valid base64: {e}")))?;

            let Some(entry) = self.store.get(&session_id).await else {
                return Err(RpcError::server_error(format!(
                    "arc session not found: {session_id}"
                )));
            };
            let holder = entry.controller.lock().await.clone();
            if holder.as_deref() != Some(requester_id.as_str()) {
                return Err(not_controller_err());
            }
            emit_host_input(&entry, &data);
            Ok(json!({"ok": true}))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.session.resize
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.session.resize`.
///
/// Driver-only: gated on `requester_id` matching the current write-control
/// holder. Emits an `arc.host.resize_request` notification carrying the new
/// `cols`/`rows` dimensions to the broadcast channel; the host-side consumer
/// (lands in Phase D) is what actually resizes the PTY. Until that consumer
/// exists this handler functions as a wire stub plus an audit fanout.
pub struct ArcSessionResizeHandler {
    store: ArcSessionStore,
}

impl ArcSessionResizeHandler {
    pub fn new(store: ArcSessionStore) -> Self {
        Self { store }
    }
}

impl RpcHandler for ArcSessionResizeHandler {
    fn method_name(&self) -> &'static str {
        "arc.session.resize"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();
            let requester_id = require_str(&params, "requester_id")?.to_string();
            let cols = require_u32(&params, "cols")?;
            let rows = require_u32(&params, "rows")?;

            let Some(entry) = self.store.get(&session_id).await else {
                return Err(RpcError::server_error(format!(
                    "arc session not found: {session_id}"
                )));
            };
            let holder = entry.controller.lock().await.clone();
            if holder.as_deref() != Some(requester_id.as_str()) {
                return Err(not_controller_err());
            }
            emit_host_resize_request(&entry, cols, rows);
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

/// Build a fresh dispatcher with all eight arc.session.* handlers registered.
/// Mainly used in tests; production wires through [`register_if_enabled`].
pub fn build_arc_session_dispatcher(store: ArcSessionStore) -> RpcDispatcher {
    let mut d = RpcDispatcher::new();
    d.register(ArcSessionAttachHandler::new(store.clone()));
    d.register(ArcSessionDetachHandler::new(store.clone()));
    d.register(ArcSessionSubscribeHandler::new(store.clone()));
    d.register(ArcSessionUnsubscribeHandler::new(store.clone()));
    d.register(ArcSessionControlHandler::new(store.clone()));
    d.register(ArcSessionReleaseHandler::new(store.clone()));
    d.register(ArcSessionInputHandler::new(store.clone()));
    d.register(ArcSessionResizeHandler::new(store));
    d
}

/// Register all eight arc.session.* handlers with `dispatcher` if
/// `JsonRpcSpine` is on. Returns `true` when handlers were registered.
pub fn register_if_enabled(dispatcher: &mut RpcDispatcher, store: ArcSessionStore) -> bool {
    if beacon_features::is_enabled(beacon_features::FeatureFlag::JsonRpcSpine) {
        dispatcher.try_register(ArcSessionAttachHandler::new(store.clone()));
        dispatcher.try_register(ArcSessionDetachHandler::new(store.clone()));
        dispatcher.try_register(ArcSessionSubscribeHandler::new(store.clone()));
        dispatcher.try_register(ArcSessionUnsubscribeHandler::new(store.clone()));
        dispatcher.try_register(ArcSessionControlHandler::new(store.clone()));
        dispatcher.try_register(ArcSessionReleaseHandler::new(store.clone()));
        dispatcher.try_register(ArcSessionInputHandler::new(store.clone()));
        dispatcher.try_register(ArcSessionResizeHandler::new(store));
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
        d.register(
            ArcSessionDetachHandler::new(store.clone())
                .with_user_resolver(|_token| "alice".into()),
        );
        d.register(
            ArcSessionSubscribeHandler::new(store.clone())
                .with_user_resolver(|_token| "alice".into()),
        );
        d.register(
            ArcSessionUnsubscribeHandler::new(store.clone())
                .with_user_resolver(|_token| "alice".into()),
        );
        d.register(ArcSessionControlHandler::new(store.clone()));
        d.register(ArcSessionReleaseHandler::new(store.clone()));
        d.register(ArcSessionInputHandler::new(store.clone()));
        d.register(ArcSessionResizeHandler::new(store.clone()));
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
    async fn build_arc_session_dispatcher_registers_eight_methods() {
        let store = ArcSessionStore::new(8);
        let d = build_arc_session_dispatcher(store);
        let mut methods = d.registered_methods();
        methods.sort();
        assert_eq!(
            methods,
            vec![
                "arc.session.attach",
                "arc.session.control",
                "arc.session.detach",
                "arc.session.input",
                "arc.session.release",
                "arc.session.resize",
                "arc.session.subscribe",
                "arc.session.unsubscribe",
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
    async fn detach_emits_participant_joined_false() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s1").await;
        let mut rx = entry.notif_tx.subscribe();

        let req = Request::new(
            "arc.session.detach",
            json!({"session_id": "s1", "token": "tok"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());
        assert_eq!(resp.result().unwrap()["ok"], true);

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.session.participant");
        assert_eq!(v["params"]["joined"], false);
    }

    #[tokio::test]
    async fn detach_unknown_session_server_error() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.session.detach",
            json!({"session_id": "ghost", "token": "tok"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.err().unwrap().message.contains("not found"));
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
    async fn subscribe_creates_entry_and_emits_participant() {
        let (store, d) = make_dispatcher();
        let req = Request::new(
            "arc.session.subscribe",
            json!({"session_id": "s2", "token": "tok"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());
        assert_eq!(resp.result().unwrap()["ok"], true);
        assert!(store.get("s2").await.is_some());

        let entry = store.get("s2").await.unwrap();
        let mut rx = entry.notif_tx.subscribe();
        // Re-fire to capture (the emitted notif already happened pre-subscribe).
        let req = Request::new(
            "arc.session.subscribe",
            json!({"session_id": "s2", "token": "tok2"}),
            2i64,
        );
        let _ = d.dispatch(req).await;
        let raw = rx.recv().await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.session.participant");
        assert_eq!(v["params"]["joined"], true);
    }

    #[tokio::test]
    async fn unsubscribe_unknown_session_errors() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.session.unsubscribe",
            json!({"session_id": "ghost", "token": "t"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.err().unwrap().message.contains("not found"));
    }

    #[tokio::test]
    async fn unsubscribe_emits_participant_joined_false() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s3").await;
        let mut rx = entry.notif_tx.subscribe();
        let req = Request::new(
            "arc.session.unsubscribe",
            json!({"session_id": "s3", "token": "t"}),
            1i64,
        );
        assert!(d.dispatch(req).await.is_success());

        let raw = rx.recv().await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.session.participant");
        assert_eq!(v["params"]["joined"], false);
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

    #[tokio::test]
    async fn input_only_when_controller() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s4").await;
        *entry.controller.lock().await = Some("u-bob".into());

        let req = Request::new(
            "arc.session.input",
            json!({
                "session_id": "s4",
                "requester_id": "u-alice",
                "data": BASE64.encode(b"ls\n"),
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        let err = resp.err().unwrap();
        assert_eq!(err.code, -32000, "expected server_error, got {err:?}");
        assert_eq!(err.data.as_ref().unwrap()["reason"], "not_controller");
    }

    #[tokio::test]
    async fn input_rejected_when_no_controller() {
        // Hardens the gating: holder=None must reject, not silently allow.
        let (store, d) = make_dispatcher();
        store.get_or_create("s4-none").await;

        let req = Request::new(
            "arc.session.input",
            json!({
                "session_id": "s4-none",
                "requester_id": "u-alice",
                "data": BASE64.encode(b"x"),
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        let err = resp.err().unwrap();
        assert_eq!(err.code, -32000);
        assert_eq!(err.data.as_ref().unwrap()["reason"], "not_controller");
    }

    #[tokio::test]
    async fn input_emits_host_input_when_controller() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s4b").await;
        *entry.controller.lock().await = Some("u-alice".into());
        let mut rx = entry.notif_tx.subscribe();

        let payload = b"echo hi\n";
        let req = Request::new(
            "arc.session.input",
            json!({
                "session_id": "s4b",
                "requester_id": "u-alice",
                "data": BASE64.encode(payload),
            }),
            1i64,
        );
        assert!(d.dispatch(req).await.is_success());

        let raw = rx.recv().await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.host.input");
        assert_eq!(v["params"]["session_id"], "s4b");
        let b64 = v["params"]["data"].as_str().unwrap();
        assert_eq!(BASE64.decode(b64).unwrap(), payload);
    }

    #[tokio::test]
    async fn input_unknown_session_errors() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.session.input",
            json!({
                "session_id": "ghost",
                "requester_id": "u-alice",
                "data": "AAAA",
            }),
            1i64,
        );
        assert!(d.dispatch(req).await.err().unwrap().message.contains("not found"));
    }

    #[tokio::test]
    async fn resize_emits_host_resize_when_controller() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s5").await;
        *entry.controller.lock().await = Some("u-alice".into());
        let mut rx = entry.notif_tx.subscribe();

        let req = Request::new(
            "arc.session.resize",
            json!({
                "session_id": "s5",
                "requester_id": "u-alice",
                "cols": 120, "rows": 40,
            }),
            1i64,
        );
        assert!(d.dispatch(req).await.is_success());

        let raw = rx.recv().await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.host.resize_request");
        assert_eq!(v["params"]["cols"], 120);
        assert_eq!(v["params"]["rows"], 40);
    }

    #[tokio::test]
    async fn resize_rejects_non_controller() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s5b").await;
        *entry.controller.lock().await = Some("u-bob".into());
        let req = Request::new(
            "arc.session.resize",
            json!({"session_id": "s5b", "requester_id": "u-alice", "cols": 80, "rows": 24}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        let err = resp.err().unwrap();
        assert_eq!(err.code, -32000);
        assert_eq!(err.data.as_ref().unwrap()["reason"], "not_controller");
    }

    #[tokio::test]
    async fn resize_invalid_dimensions_invalid_params() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s5c").await;
        *entry.controller.lock().await = Some("u-alice".into());
        let req = Request::new(
            "arc.session.resize",
            json!({"session_id": "s5c", "requester_id": "u-alice", "cols": 0, "rows": 24}),
            1i64,
        );
        assert_eq!(d.dispatch(req).await.err().unwrap().code, -32602);
    }

    #[tokio::test]
    async fn emit_control_requested_round_trip() {
        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s6").await;
        let mut rx = entry.notif_tx.subscribe();

        emit_control_requested(&entry, "req-1", "u-alice", "MacBook");

        let raw = rx.recv().await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.session.control_requested");
        assert_eq!(v["params"]["session_id"], "s6");
        assert_eq!(v["params"]["request_id"], "req-1");
        assert_eq!(v["params"]["from_user"], "u-alice");
        assert_eq!(v["params"]["from_device"], "MacBook");
    }

    #[tokio::test]
    async fn emit_mirror_discontinuity_round_trip() {
        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s7").await;
        let mut rx = entry.notif_tx.subscribe();

        emit_mirror_discontinuity(&entry, 42);

        let raw = rx.recv().await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.discontinuity");
        assert_eq!(v["params"]["session_id"], "s7");
        assert_eq!(v["params"]["last_seq"], 42);
    }

    #[tokio::test]
    async fn emit_session_gone_round_trip() {
        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s8").await;
        let mut rx = entry.notif_tx.subscribe();

        emit_session_gone(&entry, "host_disconnected");

        let raw = rx.recv().await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.session.gone");
        assert_eq!(v["params"]["session_id"], "s8");
        assert_eq!(v["params"]["reason"], "host_disconnected");
    }

    #[tokio::test]
    async fn emit_session_revoked_round_trip() {
        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s9").await;
        let mut rx = entry.notif_tx.subscribe();

        emit_session_revoked(&entry, "kicked_by_owner");

        let raw = rx.recv().await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.session.revoked");
        assert_eq!(v["params"]["session_id"], "s9");
        assert_eq!(v["params"]["reason"], "kicked_by_owner");
    }

    #[test]
    fn block_header_json_shape_locked() {
        use beacon_model::{BlockHeader, BlockId, HostInfo, SessionId, TabId};
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::time::SystemTime;

        let h = BlockHeader {
            id: BlockId::new(),
            session_id: SessionId::new(),
            tab_id: TabId::new(),
            command: "echo hi".to_string(),
            cwd: PathBuf::from("/tmp"),
            started_at: SystemTime::UNIX_EPOCH,
            env_snapshot: HashMap::new(),
            host_info: HostInfo::current(),
        };
        let v = serde_json::to_value(&h).expect("header serializes");
        let obj = v.as_object().expect("header is JSON object");

        // These keys are part of the locked wire format. Add a key to BlockHeader
        // → this test fails → bump Phase E/F consumers explicitly.
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "command", "cwd", "env_snapshot", "host_info",
                "id", "session_id", "started_at", "tab_id",
            ],
            "BlockHeader wire keys drifted; sync MIGRATION.md Phase B + arc-web/Mac decoders",
        );

        // ULIDs serialize as strings (transparent newtype).
        assert!(obj["id"].is_string(), "BlockId must be transparent ULID string");
        assert!(obj["session_id"].is_string());
        assert!(obj["tab_id"].is_string());

        // Round-trip preserves the value.
        let back: BlockHeader = serde_json::from_value(v).expect("header round-trip");
        assert_eq!(h, back);
    }

    #[test]
    fn block_footer_json_shape_locked() {
        use beacon_model::{BlockFooter, BlockId};
        use std::time::SystemTime;

        let f = BlockFooter {
            id: BlockId::new(),
            exit_code: Some(0),
            ended_at: SystemTime::UNIX_EPOCH,
            shadow_git_ref: None,
        };
        let v = serde_json::to_value(&f).expect("footer serializes");
        let obj = v.as_object().expect("footer is JSON object");

        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["ended_at", "exit_code", "id", "shadow_git_ref"],
            "BlockFooter wire keys drifted; sync MIGRATION.md Phase B + arc-web/Mac decoders",
        );

        let back: BlockFooter = serde_json::from_value(v).expect("footer round-trip");
        assert_eq!(f, back);
    }

    #[test]
    fn block_footer_signal_kill_serializes_null_exit_code() {
        use beacon_model::{BlockFooter, BlockId};
        use std::time::SystemTime;

        let f = BlockFooter {
            id: BlockId::new(),
            exit_code: None,
            ended_at: SystemTime::UNIX_EPOCH,
            shadow_git_ref: None,
        };
        let v = serde_json::to_value(&f).unwrap();
        assert!(v["exit_code"].is_null(), "exit_code:None must serialize as null, got {v}");
    }

    #[tokio::test]
    async fn emit_mirror_block_start_round_trip() {
        use beacon_model::{BlockHeader, BlockId, HostInfo, SessionId, TabId};
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::time::SystemTime;

        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s-blk").await;
        let mut rx = entry.notif_tx.subscribe();

        let header = BlockHeader {
            id: BlockId::new(),
            session_id: SessionId::new(),
            tab_id: TabId::new(),
            command: "ls -la".to_string(),
            cwd: PathBuf::from("/tmp"),
            started_at: SystemTime::UNIX_EPOCH,
            env_snapshot: HashMap::new(),
            host_info: HostInfo::current(),
        };

        emit_mirror_block_start(&entry, &header);

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.block_start");
        assert_eq!(v["params"]["session_id"], "s-blk");
        let header_back: BlockHeader =
            serde_json::from_value(v["params"]["header"].clone()).unwrap();
        assert_eq!(header_back, header);
    }

    #[tokio::test]
    async fn emit_mirror_block_chunk_round_trip() {
        use beacon_model::BlockId;

        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s-blk").await;
        let mut rx = entry.notif_tx.subscribe();

        let block_id = BlockId::new();
        let payload = b"hello\x1b[0mworld";
        emit_mirror_block_chunk(&entry, &block_id, 7, payload);

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.block_chunk");
        assert_eq!(v["params"]["session_id"], "s-blk");
        assert_eq!(v["params"]["block_id"], block_id.to_string());
        assert_eq!(v["params"]["seq"], 7);
        let b64 = v["params"]["data"].as_str().unwrap();
        assert_eq!(BASE64.decode(b64).unwrap(), payload);
    }

    #[tokio::test]
    async fn emit_mirror_block_end_round_trip() {
        use beacon_model::{BlockFooter, BlockId};
        use std::time::SystemTime;

        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s-blk").await;
        let mut rx = entry.notif_tx.subscribe();

        let footer = BlockFooter {
            id: BlockId::new(),
            exit_code: Some(0),
            ended_at: SystemTime::UNIX_EPOCH,
            shadow_git_ref: None,
        };
        emit_mirror_block_end(&entry, &footer);

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.block_end");
        assert_eq!(v["params"]["session_id"], "s-blk");
        let footer_back: BlockFooter =
            serde_json::from_value(v["params"]["footer"].clone()).unwrap();
        assert_eq!(footer_back, footer);
    }

    #[tokio::test]
    async fn emit_mirror_block_end_signal_kill_round_trip() {
        use beacon_model::{BlockFooter, BlockId};
        use std::time::SystemTime;

        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s-blk").await;
        let mut rx = entry.notif_tx.subscribe();

        let footer = BlockFooter {
            id: BlockId::new(),
            exit_code: None, // signal kill
            ended_at: SystemTime::UNIX_EPOCH,
            shadow_git_ref: None,
        };
        emit_mirror_block_end(&entry, &footer);

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert!(v["params"]["footer"]["exit_code"].is_null());
    }

    #[tokio::test]
    async fn emit_mirror_replay_begin_round_trip() {
        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s-rpl").await;
        let mut rx = entry.notif_tx.subscribe();

        emit_mirror_replay_begin(&entry, 42);

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.replay_begin");
        assert_eq!(v["params"]["session_id"], "s-rpl");
        assert_eq!(v["params"]["total_blocks"], 42);
    }

    #[tokio::test]
    async fn emit_mirror_replay_block_round_trip() {
        use beacon_model::{Block, BlockFooter, BlockHeader, BlockId, HostInfo, SessionId, TabId};
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::time::SystemTime;

        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s-rpl").await;
        let mut rx = entry.notif_tx.subscribe();

        let header = BlockHeader {
            id: BlockId::new(),
            session_id: SessionId::new(),
            tab_id: TabId::new(),
            command: "echo replayed".to_string(),
            cwd: PathBuf::from("/tmp"),
            started_at: SystemTime::UNIX_EPOCH,
            env_snapshot: HashMap::new(),
            host_info: HostInfo::current(),
        };
        let footer = BlockFooter {
            id: header.id,
            exit_code: Some(0),
            ended_at: SystemTime::UNIX_EPOCH,
            shadow_git_ref: None,
        };
        let block = Block {
            header: header.clone(),
            output: b"replayed\n".to_vec(),
            footer: Some(footer.clone()),
        };

        emit_mirror_replay_block(&entry, &block);

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.replay_block");
        assert_eq!(v["params"]["session_id"], "s-rpl");

        let header_back: BlockHeader =
            serde_json::from_value(v["params"]["header"].clone()).unwrap();
        assert_eq!(header_back, header);

        let output_b64 = v["params"]["output"].as_str().unwrap();
        assert_eq!(BASE64.decode(output_b64).unwrap(), b"replayed\n");

        let footer_back: BlockFooter =
            serde_json::from_value(v["params"]["footer"].clone()).unwrap();
        assert_eq!(footer_back, footer);
    }

    #[tokio::test]
    async fn emit_mirror_replay_block_running_block_serializes_null_footer() {
        use beacon_model::{Block, BlockHeader, BlockId, HostInfo, SessionId, TabId};
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::time::SystemTime;

        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s-rpl").await;
        let mut rx = entry.notif_tx.subscribe();

        let block = Block {
            header: BlockHeader {
                id: BlockId::new(),
                session_id: SessionId::new(),
                tab_id: TabId::new(),
                command: "tail -f /var/log/x".to_string(),
                cwd: PathBuf::from("/tmp"),
                started_at: SystemTime::UNIX_EPOCH,
                env_snapshot: HashMap::new(),
                host_info: HostInfo::current(),
            },
            output: Vec::new(),
            footer: None, // still running
        };
        emit_mirror_replay_block(&entry, &block);

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert!(v["params"]["footer"].is_null());
        assert_eq!(v["params"]["output"], "");
    }

    #[tokio::test]
    async fn emit_mirror_replay_end_round_trip() {
        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s-rpl").await;
        let mut rx = entry.notif_tx.subscribe();

        emit_mirror_replay_end(&entry);

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.replay_end");
        assert_eq!(v["params"]["session_id"], "s-rpl");
        // No other fields — only session_id.
        assert_eq!(
            v["params"].as_object().unwrap().len(),
            1,
            "replay_end params should be just session_id, got {}",
            v["params"]
        );
    }
}
