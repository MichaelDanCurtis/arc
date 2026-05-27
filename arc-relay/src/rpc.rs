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
//! - 9 host methods (Phase D): `arc.host.hello`, `arc.host.session_register`,
//!   `arc.host.session_unregister`, `arc.host.block_start`, `arc.host.block_chunk`,
//!   `arc.host.block_end`, `arc.host.session_resize`, `arc.host.push_output`,
//!   `arc.host.shadow_diff_response`
//!
//! These handlers live alongside the existing msgpack `ClientToArc` /
//! `ArcToClient` envelope (see `arc-relay/src/ws/handler.rs`). They operate
//! against an in-rpc-layer [`ArcSessionStore`] that mirrors the per-session
//! lease/presence state the existing `MirrorRegistry` already tracks.
//!
//! # Wire format — methods
//!
//! ```text
//! arc.session.attach       {"session_id":"<ulid>","token":"<jwt>"}                                    → {"ok":true}
//! arc.session.detach       {"session_id":"<ulid>","token":"<jwt>"}                                    → {"ok":true}
//! arc.session.subscribe    {"session_id":"<ulid>","token":"<jwt>"}                                    → {"ok":true}
//! arc.session.unsubscribe  {"session_id":"<ulid>","token":"<jwt>"}                                    → {"ok":true}
//! arc.session.control          {"session_id":"<ulid>","requester_id":"..."}                               → {"ok":true}
//! arc.session.release          {"session_id":"<ulid>"}                                                    → {"ok":true}
//! arc.session.approve_control  {"session_id":"<ulid>","request_id":"...","requester_id":"..."}            → {"ok":true}
//! arc.session.deny_control     {"session_id":"<ulid>","request_id":"...","requester_id":"..."}            → {"ok":true}
//! arc.session.input            {"session_id":"<ulid>","requester_id":"...","data":"<base64>"}             → {"ok":true}
//! arc.session.resize       {"session_id":"<ulid>","requester_id":"...","cols":<u32>,"rows":<u32>}     → {"ok":true}
//! arc.host.hello                  {"host_id":"...","auth_token":"...","protocol_version":<u16>}                          → {"ok":true,"protocol_version":<u16>}
//! arc.host.session_register       {"session_id":"<ulid>","name":"..."}                                                    → {"ok":true}
//! arc.host.session_unregister     {"session_id":"<ulid>"}                                                                 → {"ok":true}
//! arc.host.block_start            {"session_id":"<ulid>","header":<BlockHeader-json>}                                     → {"ok":true}
//! arc.host.block_chunk            {"session_id":"<ulid>","block_id":"<ulid>","seq":<u64>,"data":"<base64>"}               → {"ok":true}
//! arc.host.block_end              {"session_id":"<ulid>","footer":<BlockFooter-json>}                                     → {"ok":true}
//! arc.host.session_resize         {"session_id":"<ulid>","cols":<u32>,"rows":<u32>}                                       → {"ok":true}
//! arc.host.push_output            {"session_id":"<ulid>","data":"<base64>","seq":<u64>}                                   → {"ok":true,"seq":<u64>}
//! arc.host.shadow_diff_response   {"session_id":"<ulid>","request_id":"...","diff":"..."}                                 → {"ok":true}
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
//! arc.mirror.host_input          {"session_id":"<ulid>","data":"<base64>","from_user":"<id>"}                         relay→watcher
//! arc.host.resize_request        {"session_id":"<ulid>","cols":<u32>,"rows":<u32>}                                    relay→host (Phase D consumer)
//! arc.host.shadow_diff_request   {"session_id":"<ulid>","request_id":"..."}                                             relay→host (Phase D consumer)
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
use tokio::sync::{broadcast, mpsc, Mutex};
use uuid::Uuid;

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
    /// JoinHandles for the (translator_task, pump_task) background tasks
    /// spawned by `arc.host.session_register`. Set to `Some(...)` when the
    /// host registers; T2 (`arc.host.session_unregister`) aborts these on
    /// cleanup. Stored here so T2 can find them via `store.get(session_id)`.
    pub host_tasks: Mutex<Option<(tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>)>>,
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
            host_tasks: Mutex::new(None),
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

/// Emit `arc.mirror.resize` — relay→all watchers when the host PTY dimensions
/// change. Fires from `ArcHostSessionResizeHandler` after
/// `RelayRouter::update_size` succeeds, regardless of whether the resize
/// originated from a driver request (`arc.session.resize` →
/// `arc.host.resize_request`) or from a host-side event (window resize,
/// SIGWINCH).
///
/// Watchers consume this to re-lay-out their terminal grid; without it,
/// `arc.mirror.frame` bytes can render at the wrong dimensions and the
/// cursor desyncs. Spec: `arc-relay/docs/rpc-method-inventory.md` §3
/// "Proposed Notification: arc.mirror.resize" (PR #6, A2 from arc-009).
pub fn emit_mirror_resize(entry: &ArcSessionEntry, cols: u16, rows: u16) {
    send_notif(
        &entry.notif_tx,
        "arc.mirror.resize",
        json!({
            "session_id": entry.session_id,
            "cols": cols,
            "rows": rows,
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

/// Emit `arc.mirror.host_input` — relay→all-watchers carrying a base64-encoded
/// echo of the current driver's input bytes, attributed via `from_user`. Fires
/// alongside `arc.host.input` (which forwards the same bytes to the host PTY)
/// so non-driver watchers see real-time typing attribution.
///
/// Direction: relay→watcher. New behavior on the JSON-RPC side — msgpack land
/// does not broadcast `HostInput` to watchers (it routes only to host).
pub fn emit_mirror_host_input(entry: &ArcSessionEntry, data: &[u8], from_user: &str) {
    send_notif(
        &entry.notif_tx,
        "arc.mirror.host_input",
        json!({
            "session_id": entry.session_id,
            "data": BASE64.encode(data),
            "from_user": from_user,
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

/// Emit `arc.host.shadow_diff_request` — relay→host asking for a shadow-diff
/// snapshot. Pairs with the host's `arc.host.shadow_diff_response` reply.
/// Phase D adds the helper; the production trigger (currently
/// `RelayRouter::request_shadow_diff` + msgpack `ArcToClient::ShadowDiffRequest`)
/// is plumbed in Phase F.
pub fn emit_host_shadow_diff_request(entry: &ArcSessionEntry, request_id: &str) {
    send_notif(
        &entry.notif_tx,
        "arc.host.shadow_diff_request",
        json!({
            "session_id": entry.session_id,
            "request_id": request_id,
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
// arc.session.approve_control
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.session.approve_control`.
///
/// Promotes `requester_id` to the new write-control holder and emits
/// `arc.mirror.control_grant`. The `request_id` parameter is accepted for
/// wire compatibility with msgpack `ClientToArc::ApproveControl` and logged,
/// but not validated against any server-side pending-request queue — the
/// full lease-manager flow (queue, owner authorization, race resolution)
/// lives behind msgpack until Phase D wires the host surface.
///
/// Fails with `server_error` when the session entry is missing.
pub struct ArcSessionApproveControlHandler {
    store: ArcSessionStore,
}

impl ArcSessionApproveControlHandler {
    pub fn new(store: ArcSessionStore) -> Self {
        Self { store }
    }
}

impl RpcHandler for ArcSessionApproveControlHandler {
    fn method_name(&self) -> &'static str {
        "arc.session.approve_control"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();
            let request_id = require_str(&params, "request_id")?.to_string();
            let requester_id = require_str(&params, "requester_id")?.to_string();

            let Some(entry) = self.store.get(&session_id).await else {
                return Err(RpcError::server_error(format!(
                    "arc session not found: {session_id}"
                )));
            };

            *entry.controller.lock().await = Some(requester_id.clone());
            emit_mirror_control_grant(&entry, &requester_id);
            tracing::debug!(
                session_id = %session_id,
                request_id = %request_id,
                requester_id = %requester_id,
                "approve_control: promoted requester to controller (in-rpc-layer)"
            );
            Ok(json!({"ok": true}))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.session.deny_control
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.session.deny_control`.
///
/// Silent denial — no notification fired. Matches msgpack
/// `ClientToArc::DenyControl` semantic: the requesting watcher times out
/// client-side rather than receiving an explicit deny event. The
/// `request_id` parameter is accepted for wire compatibility and logged,
/// but not validated against any server-side queue.
///
/// Fails with `server_error` when the session entry is missing — matches
/// the rest of the in-rpc-layer convention (a request against an unknown
/// session is a routing bug, not a client error).
pub struct ArcSessionDenyControlHandler {
    store: ArcSessionStore,
}

impl ArcSessionDenyControlHandler {
    pub fn new(store: ArcSessionStore) -> Self {
        Self { store }
    }
}

impl RpcHandler for ArcSessionDenyControlHandler {
    fn method_name(&self) -> &'static str {
        "arc.session.deny_control"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();
            let request_id = require_str(&params, "request_id")?.to_string();
            let requester_id = require_str(&params, "requester_id")?.to_string();

            let Some(_entry) = self.store.get(&session_id).await else {
                return Err(RpcError::server_error(format!(
                    "arc session not found: {session_id}"
                )));
            };

            tracing::debug!(
                session_id = %session_id,
                request_id = %request_id,
                requester_id = %requester_id,
                "deny_control: silent denial (no notification, no controller change)"
            );
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
            emit_mirror_host_input(&entry, &data, requester_id.as_str());
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
// arc.host.hello
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.host.hello`.
///
/// Connection-level handshake: validates the host's `protocol_version` matches
/// `beacon_proto::PROTOCOL_VERSION` and echoes it back. Does not touch the
/// session store (no session_id at this layer). The `auth_token` is logged but
/// not verified — the WS upgrade handler is the auth boundary, mirroring
/// msgpack `ClientToArc::HostHello` handling.
pub struct ArcHostHelloHandler;

impl ArcHostHelloHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ArcHostHelloHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl RpcHandler for ArcHostHelloHandler {
    fn method_name(&self) -> &'static str {
        "arc.host.hello"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let host_id = require_str(&params, "host_id")?.to_string();
            let _auth_token = require_str(&params, "auth_token")?;
            let protocol_version = match params
                .get("protocol_version")
                .and_then(|v| v.as_u64())
            {
                Some(n) if n <= u16::MAX as u64 => n as u16,
                _ => {
                    return Err(RpcError::invalid_params(
                        "missing or invalid 'protocol_version' field",
                    ));
                }
            };

            if protocol_version != beacon_proto::PROTOCOL_VERSION {
                return Err(RpcError::server_error(format!(
                    "unsupported protocol version: {protocol_version} (relay speaks {})",
                    beacon_proto::PROTOCOL_VERSION
                )));
            }

            tracing::info!(
                host_id = %host_id,
                protocol_version,
                "arc.host.hello accepted"
            );
            Ok(json!({
                "ok": true,
                "protocol_version": beacon_proto::PROTOCOL_VERSION,
            }))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.host.session_register
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.host.session_register`.
///
/// Phase F full wire-up: allocates an `ArcSessionEntry`, registers the session
/// with `RelayRouter`, initialises the `MirrorRegistry` entry, spawns a
/// host-input pump task (watcher→host JSON-RPC notifications), spawns a
/// translator task (RelayRouter msgpack bytes → JSON-RPC notifications), and
/// persists the session to the DB (best-effort). The two task JoinHandles are
/// stored on `entry.host_tasks` so `arc.host.session_unregister` (T2) can abort
/// them on cleanup.
///
/// Pattern mirrors `arc-relay/src/ws/host.rs:155-243` (the msgpack path).
/// `ws/host.rs` is NOT touched by this handler.
pub struct ArcHostSessionRegisterHandler {
    store: ArcSessionStore,
    relay: Arc<crate::relay::router::RelayRouter>,
    mirror_registry: crate::mirror::MirrorRegistry,
    pool: sqlx::PgPool,
    /// Uuid of the authenticated user on this WebSocket connection. Set to
    /// `Uuid::nil()` in unit-test contexts where no real auth is present; the
    /// production path threads the real user_id from the WS upgrade handler via
    /// `register`.
    user_id: Uuid,
}

impl ArcHostSessionRegisterHandler {
    pub fn new(
        store: ArcSessionStore,
        relay: Arc<crate::relay::router::RelayRouter>,
        mirror_registry: crate::mirror::MirrorRegistry,
        pool: sqlx::PgPool,
        user_id: Uuid,
    ) -> Self {
        Self { store, relay, mirror_registry, pool, user_id }
    }
}

impl RpcHandler for ArcHostSessionRegisterHandler {
    fn method_name(&self) -> &'static str {
        "arc.host.session_register"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        let relay = self.relay.clone();
        let mirror_registry = self.mirror_registry.clone();
        let pool = self.pool.clone();
        let store = self.store.clone();
        let user_id = self.user_id;

        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();
            let name = require_str(&params, "name")?.to_string();

            // ── Step 1: Allocate ArcSessionEntry ─────────────────────────────
            let entry = store.get_or_create(&session_id).await;

            // ── Step 2: Register with RelayRouter ────────────────────────────
            // Create the mpsc channel whose Sender we give to RelayRouter as
            // the host_tx. RelayRouter encodes ArcToClient frames as msgpack
            // Vec<u8> and sends them here; the translator task (Step 4) decodes
            // and re-emits them as JSON-RPC notifications on the session entry.
            let (host_tx_bytes, mut host_rx_bytes) = mpsc::channel::<Vec<u8>>(256);
            let _broadcast_rx = relay.register_session(
                session_id.clone(),
                host_tx_bytes,
                user_id,
                name.clone(),
            );
            tracing::info!(
                session_id = %session_id,
                name = %name,
                user_id = %user_id,
                "arc.host.session_register: registered with RelayRouter",
            );

            // ── Step 3: MirrorRegistry init + host_input pump ─────────────────
            // get_or_init is idempotent; if a watcher subscribed first the
            // mirror already exists and we just retrieve it.
            let mirror_arc = mirror_registry
                .get_or_init(&session_id, user_id, vec![])
                .await;

            let (host_input_tx, mut host_input_rx) =
                mpsc::channel::<beacon_proto::envelope::ArcToClient>(256);
            {
                let mut m = mirror_arc.lock().await;
                m.host_input_tx = Some(host_input_tx);
            }

            // Pump task: ArcToClient::HostInput frames arriving from watchers
            // → arc.host.input JSON-RPC notification on this host connection.
            let entry_pump = entry.clone();
            let sid_pump = session_id.clone();
            let pump_handle = tokio::spawn(async move {
                while let Some(frame) = host_input_rx.recv().await {
                    match frame {
                        beacon_proto::envelope::ArcToClient::HostInput { bytes, .. } => {
                            emit_host_input(&entry_pump, &bytes);
                        }
                        _ => {
                            tracing::debug!(
                                session_id = %sid_pump,
                                "host_input pump: unexpected frame variant; skipping",
                            );
                        }
                    }
                }
                tracing::debug!(
                    session_id = %sid_pump,
                    "host_input pump: channel closed; stopping",
                );
            });

            // ── Step 4: Translator task ───────────────────────────────────────
            // Reads raw msgpack Vec<u8> from the RelayRouter host channel,
            // decodes as ArcToClient, and re-emits as the appropriate JSON-RPC
            // notification on the session entry.
            let entry_xlat = entry.clone();
            let sid_xlat = session_id.clone();
            let xlat_handle = tokio::spawn(async move {
                while let Some(bytes) = host_rx_bytes.recv().await {
                    match beacon_proto::codec::decode::<beacon_proto::envelope::ArcToClient>(&bytes) {
                        Ok(frame) => {
                            use beacon_proto::envelope::ArcToClient as F;
                            match &frame {
                                F::HostInput { bytes: data, .. } => {
                                    emit_host_input(&entry_xlat, data);
                                }
                                F::LeaseChanged { driver: Some(d), .. } => {
                                    emit_mirror_control_grant(&entry_xlat, &d.user_id);
                                }
                                F::LeaseChanged { driver: None, .. } => {
                                    emit_mirror_control_revoke(&entry_xlat);
                                }
                                F::ControlRequested {
                                    request_id,
                                    from_user,
                                    from_device,
                                    ..
                                } => {
                                    emit_control_requested(
                                        &entry_xlat,
                                        request_id,
                                        from_user,
                                        from_device,
                                    );
                                }
                                F::PresenceChanged { watchers, .. } => {
                                    // Translate to arc.session.participant for each watcher.
                                    // `joined: true` for all entries (presence snapshot).
                                    for w in watchers {
                                        emit_participant(&entry_xlat, &w.user_id, true);
                                    }
                                }
                                F::SessionRevoked { reason, .. } => {
                                    emit_session_revoked(&entry_xlat, reason);
                                }
                                F::ShadowDiffRequest { request_id, .. } => {
                                    emit_host_shadow_diff_request(&entry_xlat, request_id);
                                }
                                F::BlockStart { header, .. } => {
                                    emit_mirror_block_start(&entry_xlat, header);
                                }
                                F::BlockOutputChunk { block_id, seq, bytes: data, .. } => {
                                    emit_mirror_block_chunk(&entry_xlat, block_id, *seq, data);
                                }
                                F::BlockEnd { footer, .. } => {
                                    emit_mirror_block_end(&entry_xlat, footer);
                                }
                                F::SessionOutput { bytes: data, .. } => {
                                    emit_mirror_frame(&entry_xlat, data);
                                }
                                F::Ping | F::Pong => {
                                    // drop silently
                                }
                                other => {
                                    tracing::debug!(
                                        session_id = %sid_xlat,
                                        variant = ?std::mem::discriminant(other),
                                        "translator: unhandled ArcToClient variant; skipping",
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                session_id = %sid_xlat,
                                "translator: decode error: {e}; skipping frame",
                            );
                        }
                    }
                }
                tracing::debug!(
                    session_id = %sid_xlat,
                    "translator: relay channel closed; stopping",
                );
            });

            // Store handles on the entry so T2 (session_unregister) can abort them.
            *entry.host_tasks.lock().await = Some((xlat_handle, pump_handle));

            // ── Step 5: DB persist (best-effort) ─────────────────────────────
            let pool2 = pool.clone();
            let sid2 = session_id.clone();
            let name2 = name.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::models::session::create_session(
                    &pool2,
                    &sid2,
                    user_id, // host_id = connecting user
                    user_id, // owner_id = connecting user
                    Some(&name2),
                )
                .await
                {
                    tracing::warn!(
                        session_id = %sid2,
                        "arc.host.session_register: create_session DB error: {e}",
                    );
                }
            });

            tracing::info!(
                session_id = %session_id,
                name = %name,
                user_id = %user_id,
                "arc.host.session_register: fully wired (relay + mirror + pump + translator + db)",
            );
            Ok(json!({"ok": true}))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.host.session_unregister
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.host.session_unregister`.
///
/// Full Phase F wire-up: on a known session entry, this handler:
/// 1. Calls `relay.unregister_session` — drops the broadcast channel, stops
///    mirror fan-out.
/// 2. Spawns a best-effort `update_session_state(&pool, sid, "ended")` DB
///    update (warns on error; does NOT fail the RPC reply).
/// 3. Aborts the translator + host_input pump `JoinHandle`s stored on
///    `entry.host_tasks` by T1 (`arc.host.session_register`).
/// 4. Clears `mirror.host_input_tx` — sets it to `None`; the `MirrorRegistry`
///    entry itself is left in place for connection-lifetime invariant stability.
///
/// All four steps are idempotent: if `host_tasks` is `None` (never registered
/// or already unregistered) step 3 is a no-op; `relay.unregister_session` is
/// idempotent boolean; `update_session_state` is a no-op if already ended.
///
/// If the entry is missing entirely, returns `{ok: true}` without notification
/// — matches msgpack's no-error-on-missing semantic.
///
/// Pattern mirrors `arc-relay/src/ws/host.rs:245-266` (the msgpack path).
/// `ws/host.rs` is NOT touched by this handler.
pub struct ArcHostSessionUnregisterHandler {
    store: ArcSessionStore,
    relay: Arc<crate::relay::router::RelayRouter>,
    mirror_registry: crate::mirror::MirrorRegistry,
    pool: sqlx::PgPool,
}

impl ArcHostSessionUnregisterHandler {
    pub fn new(
        store: ArcSessionStore,
        relay: Arc<crate::relay::router::RelayRouter>,
        mirror_registry: crate::mirror::MirrorRegistry,
        pool: sqlx::PgPool,
    ) -> Self {
        Self { store, relay, mirror_registry, pool }
    }
}

impl RpcHandler for ArcHostSessionUnregisterHandler {
    fn method_name(&self) -> &'static str {
        "arc.host.session_unregister"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        let relay = self.relay.clone();
        let mirror_registry = self.mirror_registry.clone();
        let pool = self.pool.clone();
        let store = self.store.clone();

        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();

            if let Some(entry) = store.get(&session_id).await {
                // ── Step 1: Unregister from RelayRouter ──────────────────────
                // Drops the broadcast channel; any attached watcher receivers
                // will see their streams close.  Idempotent: returns false if
                // already removed, which we ignore.
                let _was_registered = relay.unregister_session(&session_id);
                tracing::info!(
                    session_id = %session_id,
                    was_registered = _was_registered,
                    "arc.host.session_unregister: unregistered from RelayRouter",
                );

                // ── Step 2: DB state → "ended" (best-effort) ─────────────────
                let pool2 = pool.clone();
                let sid2 = session_id.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        crate::models::session::update_session_state(&pool2, &sid2, "ended").await
                    {
                        tracing::warn!(
                            session_id = %sid2,
                            "arc.host.session_unregister: update_session_state DB error: {e}",
                        );
                    }
                });

                // ── Step 3: Abort translator + pump JoinHandles ───────────────
                // Take the Option out so double-unregister doesn't abort twice.
                if let Some((xlat_handle, pump_handle)) =
                    entry.host_tasks.lock().await.take()
                {
                    xlat_handle.abort();
                    pump_handle.abort();
                    tracing::debug!(
                        session_id = %session_id,
                        "arc.host.session_unregister: aborted translator + pump handles",
                    );
                }

                // ── Step 4: Clear mirror host_input_tx ───────────────────────
                // Leaves the MirrorRegistry entry in place; just drops the
                // sender so subsequent SessionInput frames are silently dropped
                // (mirror/handlers.rs already handles None gracefully).
                if let Some(mirror_arc) = mirror_registry.get(&session_id).await {
                    let mut m = mirror_arc.lock().await;
                    m.host_input_tx = None;
                    tracing::debug!(
                        session_id = %session_id,
                        "arc.host.session_unregister: cleared mirror host_input_tx",
                    );
                }

                // ── Step 5: Emit arc.session.gone ────────────────────────────
                emit_session_gone(&entry, "host_unregistered");
                tracing::info!(
                    session_id = %session_id,
                    "arc.host.session_unregister: emitted arc.session.gone",
                );
            } else {
                tracing::debug!(
                    session_id = %session_id,
                    "arc.host.session_unregister: entry missing (idempotent no-op)"
                );
            }
            Ok(json!({"ok": true}))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.host.block_start
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.host.block_start`.
///
/// Decodes the `BlockHeader` from `params.header` and re-broadcasts it via
/// `emit_mirror_block_start` so subscribed watchers receive the
/// `arc.mirror.block_start` notification (Phase B wire shape, locked by
/// `block_header_json_shape_locked`).
pub struct ArcHostBlockStartHandler {
    store: ArcSessionStore,
}

impl ArcHostBlockStartHandler {
    pub fn new(store: ArcSessionStore) -> Self {
        Self { store }
    }
}

impl RpcHandler for ArcHostBlockStartHandler {
    fn method_name(&self) -> &'static str {
        "arc.host.block_start"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();

            let Some(entry) = self.store.get(&session_id).await else {
                return Err(RpcError::server_error(format!(
                    "arc session not found: {session_id}"
                )));
            };

            let header_value = params
                .get("header")
                .cloned()
                .ok_or_else(|| RpcError::invalid_params("missing 'header' field"))?;
            let header: beacon_model::BlockHeader = serde_json::from_value(header_value)
                .map_err(|e| RpcError::invalid_params(format!("invalid 'header': {e}")))?;

            emit_mirror_block_start(&entry, &header);
            Ok(json!({"ok": true}))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.host.block_chunk
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.host.block_chunk`.
///
/// Decodes base64 `data` and the ULID `block_id`, then re-broadcasts via
/// `emit_mirror_block_chunk`. `seq` is forwarded verbatim for watcher-side
/// gap detection.
pub struct ArcHostBlockChunkHandler {
    store: ArcSessionStore,
}

impl ArcHostBlockChunkHandler {
    pub fn new(store: ArcSessionStore) -> Self {
        Self { store }
    }
}

impl RpcHandler for ArcHostBlockChunkHandler {
    fn method_name(&self) -> &'static str {
        "arc.host.block_chunk"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();
            let block_id_str = require_str(&params, "block_id")?;
            let block_id: beacon_model::BlockId = block_id_str
                .parse()
                .map_err(|e| RpcError::invalid_params(format!("invalid 'block_id': {e}")))?;
            let seq = params
                .get("seq")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| RpcError::invalid_params("missing or invalid 'seq' field"))?;
            let data_b64 = require_str(&params, "data")?;
            let data = BASE64
                .decode(data_b64)
                .map_err(|e| RpcError::invalid_params(format!("data not valid base64: {e}")))?;

            let Some(entry) = self.store.get(&session_id).await else {
                return Err(RpcError::server_error(format!(
                    "arc session not found: {session_id}"
                )));
            };
            emit_mirror_block_chunk(&entry, &block_id, seq, &data);
            Ok(json!({"ok": true}))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.host.block_end
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.host.block_end`.
///
/// Decodes the `BlockFooter` from `params.footer` and re-broadcasts via
/// `emit_mirror_block_end` (Phase B wire shape).
pub struct ArcHostBlockEndHandler {
    store: ArcSessionStore,
}

impl ArcHostBlockEndHandler {
    pub fn new(store: ArcSessionStore) -> Self {
        Self { store }
    }
}

impl RpcHandler for ArcHostBlockEndHandler {
    fn method_name(&self) -> &'static str {
        "arc.host.block_end"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();

            let Some(entry) = self.store.get(&session_id).await else {
                return Err(RpcError::server_error(format!(
                    "arc session not found: {session_id}"
                )));
            };

            let footer_value = params
                .get("footer")
                .cloned()
                .ok_or_else(|| RpcError::invalid_params("missing 'footer' field"))?;
            let footer: beacon_model::BlockFooter = serde_json::from_value(footer_value)
                .map_err(|e| RpcError::invalid_params(format!("invalid 'footer': {e}")))?;

            emit_mirror_block_end(&entry, &footer);
            Ok(json!({"ok": true}))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.host.session_resize
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.host.session_resize`.
///
/// Phase F wire-up: calls `RelayRouter::update_size` so that subsequent
/// `arc.session.attach` replies carry the updated `initial_size`. No
/// notification fires (msgpack also does not broadcast resize events to
/// watchers).
pub struct ArcHostSessionResizeHandler {
    store: ArcSessionStore,
    relay: Arc<crate::relay::router::RelayRouter>,
}

impl ArcHostSessionResizeHandler {
    pub fn new(
        store: ArcSessionStore,
        relay: Arc<crate::relay::router::RelayRouter>,
    ) -> Self {
        Self { store, relay }
    }
}

impl RpcHandler for ArcHostSessionResizeHandler {
    fn method_name(&self) -> &'static str {
        "arc.host.session_resize"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        let relay = self.relay.clone();
        let store = self.store.clone();
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();
            let cols = require_u32(&params, "cols")? as u16;
            let rows = require_u32(&params, "rows")? as u16;

            let Some(entry) = store.get(&session_id).await else {
                return Err(RpcError::server_error(format!(
                    "not found: {session_id}"
                )));
            };
            relay.update_size(&session_id, cols, rows);
            // Fan the new geometry out to subscribed watchers so they
            // re-layout their terminal grid (spec: A2 / PR #6). One emit
            // site covers both driver-initiated and host-initiated
            // resizes because both terminate in this handler.
            emit_mirror_resize(&entry, cols, rows);
            tracing::info!(
                session_id = %session_id,
                cols, rows,
                "arc.host.session_resize: updated RelayRouter size + emitted arc.mirror.resize",
            );
            Ok(json!({"ok": true}))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.host.push_output
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.host.push_output`.
///
/// Phase F wire-up: decodes base64 `data`, appends to `ScrollbackRing` for a
/// canonical monotone `seq`, then calls `MirrorRegistry::broadcast` which fans
/// out via both the msgpack `Fanout` path AND the `ArcMirrorEmitter` JSON-RPC
/// path (registered in Phase D). The host-supplied `seq` parameter is
/// intentionally discarded — `ScrollbackRing::append` assigns the canonical
/// sequence number.
///
/// If the session has no `MirrorRegistry` entry (host pushed before
/// `arc.host.session_register`), the frame is silently dropped and `{ok: true}`
/// is returned — matches msgpack semantics.
pub struct ArcHostPushOutputHandler {
    store: ArcSessionStore,
    mirror_registry: crate::mirror::MirrorRegistry,
}

impl ArcHostPushOutputHandler {
    pub fn new(
        store: ArcSessionStore,
        mirror_registry: crate::mirror::MirrorRegistry,
    ) -> Self {
        Self { store, mirror_registry }
    }
}

impl RpcHandler for ArcHostPushOutputHandler {
    fn method_name(&self) -> &'static str {
        "arc.host.push_output"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        let store = self.store.clone();
        let mirror_registry = self.mirror_registry.clone();
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();
            // `seq` from host is discarded; ScrollbackRing assigns canonical seq.
            // Use as_str (not require_str) — empty string is valid base64 (zero bytes).
            let data_b64 = params
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| RpcError::invalid_params("missing 'data' field"))?;
            let data = BASE64
                .decode(data_b64)
                .map_err(|e| RpcError::invalid_params(format!("data not valid base64: {e}")))?;

            let Some(_entry) = store.get(&session_id).await else {
                return Err(RpcError::server_error(format!(
                    "arc session not found: {session_id}"
                )));
            };

            let Some(mirror_arc) = mirror_registry.get(&session_id).await else {
                tracing::debug!(
                    session_id = %session_id,
                    "arc.host.push_output: session not registered for mirroring; ignoring",
                );
                return Ok(json!({"ok": true, "seq": 0u64}));
            };

            let canonical_seq = {
                let mut m = mirror_arc.lock().await;
                m.scrollback.append(data.clone())
            };

            // Construct the typed SessionOutput for MirrorRegistry::broadcast.
            // session_id is a ULID in production; fall back to emit_mirror_frame
            // for non-ULID values (unit tests use plain string IDs).
            if let Ok(sid_typed) = session_id.parse::<beacon_model::SessionId>() {
                mirror_registry
                    .broadcast(
                        &session_id,
                        beacon_proto::envelope::ArcToClient::SessionOutput {
                            session_id: sid_typed,
                            bytes: data,
                            seq: canonical_seq,
                            replay: false,
                        },
                    )
                    .await;
            } else {
                // Non-ULID session_id (test-only path): broadcast directly via
                // emit_mirror_frame so notif_tx subscribers still receive the frame.
                if let Some(entry) = store.get(&session_id).await {
                    emit_mirror_frame(&entry, &data);
                }
            }

            tracing::info!(
                session_id = %session_id,
                seq = canonical_seq,
                "arc.host.push_output: broadcast via MirrorRegistry",
            );
            Ok(json!({"ok": true, "seq": canonical_seq}))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.host.shadow_diff_response
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.host.shadow_diff_response`.
///
/// Receives the host's diff payload for a previously-issued
/// `arc.host.shadow_diff_request` and resolves the corresponding
/// `RelayRouter::pending_shadow_diffs` waiter. If no waiter is present a warn
/// is logged but the RPC still returns `{ok: true}` — the host should not be
/// penalised for a late/orphaned response.
///
/// Note: also accepts an `Option<String>` `diff` field to allow `null` for
/// "host has no diff for this request" responses; required-string check would
/// reject that. Wire spec uses plain string per `ClientToArc::ShadowDiffResponse`.
pub struct ArcHostShadowDiffResponseHandler {
    relay: Arc<crate::relay::router::RelayRouter>,
}

impl ArcHostShadowDiffResponseHandler {
    pub fn new(relay: Arc<crate::relay::router::RelayRouter>) -> Self {
        Self { relay }
    }
}

impl RpcHandler for ArcHostShadowDiffResponseHandler {
    fn method_name(&self) -> &'static str {
        "arc.host.shadow_diff_response"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        let relay = self.relay.clone();
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();
            let request_id = require_str(&params, "request_id")?.to_string();
            // `diff` is required per the msgpack envelope, but allow empty string.
            let diff = params
                .get("diff")
                .and_then(|v| v.as_str())
                .ok_or_else(|| RpcError::invalid_params("missing 'diff' field"))?
                .to_string();

            let resolved = relay.resolve_shadow_diff(&request_id, diff);
            if !resolved {
                tracing::warn!(
                    session_id = %session_id,
                    request_id = %request_id,
                    "arc.host.shadow_diff_response: no waiter for request_id (late or orphaned response)",
                );
            } else {
                tracing::debug!(
                    session_id = %session_id,
                    request_id = %request_id,
                    "arc.host.shadow_diff_response: resolved waiter",
                );
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

/// Build a fresh dispatcher with all 19 arc.session.* + arc.host.* handlers registered.
/// Mainly used in tests; production wires through [`register`].
///
/// `relay`, `mirror_registry`, `pool`, and `user_id` are threaded into
/// `ArcHostSessionRegisterHandler` for full Phase F wire-up. Pass a nil Uuid
/// in unit tests that don't exercise session_register's relay/DB paths.
pub fn build_arc_session_dispatcher(
    store: ArcSessionStore,
    relay: Arc<crate::relay::router::RelayRouter>,
    mirror_registry: crate::mirror::MirrorRegistry,
    pool: sqlx::PgPool,
    user_id: Uuid,
) -> RpcDispatcher {
    let mut d = RpcDispatcher::new();
    // Watcher surface (Phase A + B + C)
    d.register(ArcSessionAttachHandler::new(store.clone()));
    d.register(ArcSessionDetachHandler::new(store.clone()));
    d.register(ArcSessionSubscribeHandler::new(store.clone()));
    d.register(ArcSessionUnsubscribeHandler::new(store.clone()));
    d.register(ArcSessionControlHandler::new(store.clone()));
    d.register(ArcSessionReleaseHandler::new(store.clone()));
    d.register(ArcSessionApproveControlHandler::new(store.clone()));
    d.register(ArcSessionDenyControlHandler::new(store.clone()));
    d.register(ArcSessionInputHandler::new(store.clone()));
    d.register(ArcSessionResizeHandler::new(store.clone()));
    // Host surface (Phase D + F)
    d.register(ArcHostHelloHandler::new());
    d.register(ArcHostSessionRegisterHandler::new(
        store.clone(),
        relay.clone(),
        mirror_registry.clone(),
        pool.clone(),
        user_id,
    ));
    d.register(ArcHostSessionUnregisterHandler::new(
        store.clone(),
        relay.clone(),
        mirror_registry.clone(),
        pool,
    ));
    d.register(ArcHostBlockStartHandler::new(store.clone()));
    d.register(ArcHostBlockChunkHandler::new(store.clone()));
    d.register(ArcHostBlockEndHandler::new(store.clone()));
    d.register(ArcHostSessionResizeHandler::new(store.clone(), relay.clone()));
    d.register(ArcHostPushOutputHandler::new(store, mirror_registry));
    d.register(ArcHostShadowDiffResponseHandler::new(relay));
    d
}

/// Register all 19 arc.session.* + arc.host.* handlers with `dispatcher`.
///
/// `relay`, `mirror_registry`, `pool`, and `user_id` are required for the
/// full Phase F wire-up of `arc.host.session_register`.
pub fn register(
    dispatcher: &mut RpcDispatcher,
    store: ArcSessionStore,
    relay: Arc<crate::relay::router::RelayRouter>,
    mirror_registry: crate::mirror::MirrorRegistry,
    pool: sqlx::PgPool,
    user_id: Uuid,
) {
    // Watcher surface (Phase A + B + C)
    dispatcher.try_register(ArcSessionAttachHandler::new(store.clone()));
    dispatcher.try_register(ArcSessionDetachHandler::new(store.clone()));
    dispatcher.try_register(ArcSessionSubscribeHandler::new(store.clone()));
    dispatcher.try_register(ArcSessionUnsubscribeHandler::new(store.clone()));
    dispatcher.try_register(ArcSessionControlHandler::new(store.clone()));
    dispatcher.try_register(ArcSessionReleaseHandler::new(store.clone()));
    dispatcher.try_register(ArcSessionApproveControlHandler::new(store.clone()));
    dispatcher.try_register(ArcSessionDenyControlHandler::new(store.clone()));
    dispatcher.try_register(ArcSessionInputHandler::new(store.clone()));
    dispatcher.try_register(ArcSessionResizeHandler::new(store.clone()));
    // Host surface (Phase D + F)
    dispatcher.try_register(ArcHostHelloHandler::new());
    dispatcher.try_register(ArcHostSessionRegisterHandler::new(
        store.clone(),
        relay.clone(),
        mirror_registry.clone(),
        pool.clone(),
        user_id,
    ));
    dispatcher.try_register(ArcHostSessionUnregisterHandler::new(
        store.clone(),
        relay.clone(),
        mirror_registry.clone(),
        pool,
    ));
    dispatcher.try_register(ArcHostBlockStartHandler::new(store.clone()));
    dispatcher.try_register(ArcHostBlockChunkHandler::new(store.clone()));
    dispatcher.try_register(ArcHostBlockEndHandler::new(store.clone()));
    dispatcher.try_register(ArcHostSessionResizeHandler::new(store.clone(), relay.clone()));
    dispatcher.try_register(ArcHostPushOutputHandler::new(store, mirror_registry));
    dispatcher.try_register(ArcHostShadowDiffResponseHandler::new(relay));
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use beacon_jsonrpc::Request;

    /// Build a dispatcher wired to a lazy (never-connecting) pool and a
    /// fresh RelayRouter + MirrorRegistry. Suitable for all unit tests that
    /// don't exercise DB or inter-task paths.
    fn make_dispatcher() -> (ArcSessionStore, RpcDispatcher) {
        let store = ArcSessionStore::new(64);
        let relay = Arc::new(crate::relay::router::RelayRouter::new());
        let mirror_registry = crate::mirror::MirrorRegistry::default();
        let pool = sqlx::PgPool::connect_lazy("postgres://arc:arc@localhost:5432/arc")
            .expect("lazy pool");
        let user_id = Uuid::nil();
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
        d.register(ArcSessionApproveControlHandler::new(store.clone()));
        d.register(ArcSessionDenyControlHandler::new(store.clone()));
        d.register(ArcSessionInputHandler::new(store.clone()));
        d.register(ArcSessionResizeHandler::new(store.clone()));
        d.register(ArcHostHelloHandler::new());
        d.register(ArcHostSessionRegisterHandler::new(
            store.clone(),
            relay.clone(),
            mirror_registry.clone(),
            pool.clone(),
            user_id,
        ));
        d.register(ArcHostSessionUnregisterHandler::new(
            store.clone(),
            relay.clone(),
            mirror_registry.clone(),
            pool,
        ));
        d.register(ArcHostBlockStartHandler::new(store.clone()));
        d.register(ArcHostBlockChunkHandler::new(store.clone()));
        d.register(ArcHostBlockEndHandler::new(store.clone()));
        d.register(ArcHostSessionResizeHandler::new(store.clone(), relay.clone()));
        d.register(ArcHostPushOutputHandler::new(store.clone(), mirror_registry));
        d.register(ArcHostShadowDiffResponseHandler::new(relay));
        (store, d)
    }

    /// Like `make_dispatcher` but also returns the `MirrorRegistry` and
    /// pre-registers `ArcMirrorEmitter` so that `mirror_registry.broadcast`
    /// flows through to `entry.notif_tx` via the emitter hook.
    fn make_dispatcher_with_mirror()
        -> (ArcSessionStore, crate::mirror::MirrorRegistry, RpcDispatcher)
    {
        let store = ArcSessionStore::new(64);
        let relay = Arc::new(crate::relay::router::RelayRouter::new());
        let mirror_registry = crate::mirror::MirrorRegistry::default();
        mirror_registry.add_emitter(Arc::new(ArcMirrorEmitter::new(store.clone())));
        let pool = sqlx::PgPool::connect_lazy("postgres://arc:arc@localhost:5432/arc")
            .expect("lazy pool");
        let user_id = Uuid::nil();
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
        d.register(ArcSessionApproveControlHandler::new(store.clone()));
        d.register(ArcSessionDenyControlHandler::new(store.clone()));
        d.register(ArcSessionInputHandler::new(store.clone()));
        d.register(ArcSessionResizeHandler::new(store.clone()));
        d.register(ArcHostHelloHandler::new());
        d.register(ArcHostSessionRegisterHandler::new(
            store.clone(),
            relay.clone(),
            mirror_registry.clone(),
            pool.clone(),
            user_id,
        ));
        d.register(ArcHostSessionUnregisterHandler::new(
            store.clone(),
            relay.clone(),
            mirror_registry.clone(),
            pool,
        ));
        d.register(ArcHostBlockStartHandler::new(store.clone()));
        d.register(ArcHostBlockChunkHandler::new(store.clone()));
        d.register(ArcHostBlockEndHandler::new(store.clone()));
        d.register(ArcHostSessionResizeHandler::new(store.clone(), relay.clone()));
        d.register(ArcHostPushOutputHandler::new(store.clone(), mirror_registry.clone()));
        d.register(ArcHostShadowDiffResponseHandler::new(relay));
        (store, mirror_registry, d)
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
    async fn build_arc_session_dispatcher_registers_nineteen_methods() {
        let store = ArcSessionStore::new(8);
        let relay = Arc::new(crate::relay::router::RelayRouter::new());
        let mirror_registry = crate::mirror::MirrorRegistry::default();
        let pool = sqlx::PgPool::connect_lazy("postgres://arc:arc@localhost:5432/arc")
            .expect("lazy pool");
        let d = build_arc_session_dispatcher(store, relay, mirror_registry, pool, Uuid::nil());
        let mut methods = d.registered_methods();
        methods.sort();
        assert_eq!(
            methods,
            vec![
                "arc.host.block_chunk",
                "arc.host.block_end",
                "arc.host.block_start",
                "arc.host.hello",
                "arc.host.push_output",
                "arc.host.session_register",
                "arc.host.session_resize",
                "arc.host.session_unregister",
                "arc.host.shadow_diff_response",
                "arc.session.approve_control",
                "arc.session.attach",
                "arc.session.control",
                "arc.session.deny_control",
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
    async fn input_also_emits_mirror_host_input_when_controller() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s1").await;
        *entry.controller.lock().await = Some("alice".into());
        let mut rx = entry.notif_tx.subscribe();

        let req = Request::new(
            "arc.session.input",
            json!({
                "session_id": "s1",
                "requester_id": "alice",
                "data": BASE64.encode(b"ls"),
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());

        // Drain notifications and assert BOTH arc.host.input AND arc.mirror.host_input fire.
        let mut saw_host = false;
        let mut saw_mirror = false;
        for _ in 0..2 {
            let raw = rx.recv().await.expect("notif");
            let v: Value = serde_json::from_str(&raw).unwrap();
            match v["method"].as_str().unwrap() {
                "arc.host.input" => {
                    saw_host = true;
                    let b64 = v["params"]["data"].as_str().unwrap();
                    assert_eq!(BASE64.decode(b64).unwrap(), b"ls");
                }
                "arc.mirror.host_input" => {
                    saw_mirror = true;
                    let b64 = v["params"]["data"].as_str().unwrap();
                    assert_eq!(BASE64.decode(b64).unwrap(), b"ls");
                    assert_eq!(v["params"]["from_user"], "alice");
                }
                other => panic!("unexpected notification: {other}"),
            }
        }
        assert!(saw_host && saw_mirror, "expected both notifications");
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
    async fn emit_mirror_resize_round_trip() {
        // Closes A2 from arc-009 (PR #6 spec). Wire shape:
        //   { method: "arc.mirror.resize",
        //     params: { session_id, cols, rows } }
        // Emitted from ArcHostSessionResizeHandler after
        // RelayRouter::update_size succeeds — covers both driver-initiated
        // and host-initiated resizes through the same emit site.
        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s-resize").await;
        let mut rx = entry.notif_tx.subscribe();

        emit_mirror_resize(&entry, 120, 40);

        let raw = rx.recv().await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.resize");
        assert_eq!(v["params"]["session_id"], "s-resize");
        assert_eq!(v["params"]["cols"], 120);
        assert_eq!(v["params"]["rows"], 40);
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

    #[tokio::test]
    async fn emit_mirror_host_input_round_trip() {
        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s-echo").await;
        let mut rx = entry.notif_tx.subscribe();

        emit_mirror_host_input(&entry, b"hi\x1b[0m", "alice");

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.host_input");
        assert_eq!(v["params"]["session_id"], "s-echo");
        assert_eq!(v["params"]["from_user"], "alice");
        let b64 = v["params"]["data"].as_str().unwrap();
        assert_eq!(BASE64.decode(b64).unwrap(), b"hi\x1b[0m");
    }

    #[test]
    fn mirror_host_input_wire_shape_locked() {
        use serde_json::json;

        let n = beacon_jsonrpc::Notification::new(
            "arc.mirror.host_input",
            json!({
                "session_id": "s1",
                "data": "aGVsbG8=", // "hello"
                "from_user": "alice",
            }),
        );
        let s = serde_json::to_string(&n).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();

        // Lock the wire keys: top-level method + params shape.
        assert_eq!(v["method"], "arc.mirror.host_input");
        let params = v["params"].as_object().expect("params is object");
        let mut keys: Vec<&String> = params.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["data", "from_user", "session_id"],
            "arc.mirror.host_input wire keys drifted; sync MIGRATION.md Phase C + arc-web/Mac decoders",
        );
        assert!(params["data"].is_string(), "data must be base64 string");
        assert!(params["from_user"].is_string());
        assert!(params["session_id"].is_string());
    }

    #[tokio::test]
    async fn approve_control_promotes_requester_and_emits_grant() {
        let (store, d) = make_dispatcher();
        // Pre-create entry; set existing controller (the owner who's approving).
        let entry = store.get_or_create("s1").await;
        *entry.controller.lock().await = Some("owner".into());
        let mut rx = entry.notif_tx.subscribe();

        let req = Request::new(
            "arc.session.approve_control",
            json!({
                "session_id": "s1",
                "request_id": "req-42",
                "requester_id": "bob",
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());
        assert_eq!(resp.result().unwrap()["ok"], true);

        // Controller field promoted.
        assert_eq!(*entry.controller.lock().await, Some("bob".to_string()));

        // arc.mirror.control_grant fired with the new holder.
        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.control_grant");
        assert_eq!(v["params"]["session_id"], "s1");
        assert_eq!(v["params"]["holder_id"], "bob");
    }

    #[tokio::test]
    async fn approve_control_unknown_session_server_error() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.session.approve_control",
            json!({
                "session_id": "ghost",
                "request_id": "req-1",
                "requester_id": "bob",
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.err().unwrap().message.contains("not found"));
    }

    #[tokio::test]
    async fn approve_control_missing_request_id_invalid_params() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.session.approve_control",
            json!({"session_id": "s1", "requester_id": "bob"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(
            resp.err().unwrap().code,
            beacon_jsonrpc::RpcErrorCode::InvalidParams as i32,
        );
    }

    #[tokio::test]
    async fn deny_control_silent_no_notification() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s1").await;
        *entry.controller.lock().await = Some("owner".into());
        let mut rx = entry.notif_tx.subscribe();

        let req = Request::new(
            "arc.session.deny_control",
            json!({
                "session_id": "s1",
                "request_id": "req-42",
                "requester_id": "bob",
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());
        assert_eq!(resp.result().unwrap()["ok"], true);

        // Controller unchanged.
        assert_eq!(*entry.controller.lock().await, Some("owner".to_string()));

        // No notification emitted.
        use tokio::sync::broadcast::error::TryRecvError;
        match rx.try_recv() {
            Err(TryRecvError::Empty) => { /* expected */ }
            other => panic!("expected no notification, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn deny_control_unknown_session_server_error() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.session.deny_control",
            json!({
                "session_id": "ghost",
                "request_id": "req-1",
                "requester_id": "bob",
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.err().unwrap().message.contains("not found"));
    }

    #[tokio::test]
    async fn emit_host_shadow_diff_request_round_trip() {
        let store = ArcSessionStore::new(8);
        let entry = store.get_or_create("s-shadow").await;
        let mut rx = entry.notif_tx.subscribe();

        emit_host_shadow_diff_request(&entry, "req-abc");

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.host.shadow_diff_request");
        assert_eq!(v["params"]["session_id"], "s-shadow");
        assert_eq!(v["params"]["request_id"], "req-abc");
    }

    #[test]
    fn host_shadow_diff_request_wire_shape_locked() {
        use serde_json::json;

        let n = beacon_jsonrpc::Notification::new(
            "arc.host.shadow_diff_request",
            json!({
                "session_id": "s1",
                "request_id": "req-001",
            }),
        );
        let s = serde_json::to_string(&n).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();

        assert_eq!(v["method"], "arc.host.shadow_diff_request");
        let params = v["params"].as_object().expect("params is object");
        let mut keys: Vec<&String> = params.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["request_id", "session_id"],
            "arc.host.shadow_diff_request wire keys drifted; sync MIGRATION.md Phase D + arc-web/Mac decoders",
        );
        assert!(params["request_id"].is_string());
        assert!(params["session_id"].is_string());
    }

    // ── arc.host.hello ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn host_hello_returns_protocol_version() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.host.hello",
            json!({
                "host_id": "host-1",
                "auth_token": "tok",
                "protocol_version": beacon_proto::PROTOCOL_VERSION,
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());
        let result = resp.result().unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["protocol_version"], beacon_proto::PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn host_hello_rejects_mismatched_protocol_version() {
        let (_s, d) = make_dispatcher();
        let bad_version = beacon_proto::PROTOCOL_VERSION.wrapping_add(99);
        let req = Request::new(
            "arc.host.hello",
            json!({
                "host_id": "host-1",
                "auth_token": "tok",
                "protocol_version": bad_version,
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        let err = resp.err().unwrap();
        assert!(
            err.message.contains("protocol") || err.message.contains("unsupported"),
            "expected protocol-version mismatch error, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn host_hello_missing_host_id_invalid_params() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.host.hello",
            json!({"auth_token": "tok", "protocol_version": beacon_proto::PROTOCOL_VERSION}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    #[tokio::test]
    async fn host_hello_missing_protocol_version_invalid_params() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.host.hello",
            json!({"host_id": "host-1", "auth_token": "tok"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    // ── arc.host.session_register ─────────────────────────────────────────────

    #[tokio::test]
    async fn host_session_register_creates_entry() {
        let (store, d) = make_dispatcher();
        let req = Request::new(
            "arc.host.session_register",
            json!({"session_id": "s-host-1", "name": "shell"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());
        assert_eq!(resp.result().unwrap()["ok"], true);
        assert!(store.get("s-host-1").await.is_some());
    }

    #[tokio::test]
    async fn host_session_register_emits_no_notif() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s-host-2").await;
        let mut rx = entry.notif_tx.subscribe();

        let req = Request::new(
            "arc.host.session_register",
            json!({"session_id": "s-host-2", "name": "shell"}),
            1i64,
        );
        assert!(d.dispatch(req).await.is_success());

        // Host-side register is silent on the watcher broadcast channel.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn host_session_register_missing_name_invalid_params() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.host.session_register",
            json!({"session_id": "s1"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    // ── arc.host.session_unregister ───────────────────────────────────────────

    #[tokio::test]
    async fn host_session_unregister_emits_session_gone() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s-bye").await;
        let mut rx = entry.notif_tx.subscribe();

        let req = Request::new(
            "arc.host.session_unregister",
            json!({"session_id": "s-bye"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.session.gone");
        assert_eq!(v["params"]["session_id"], "s-bye");
        assert_eq!(v["params"]["reason"], "host_unregistered");
    }

    #[tokio::test]
    async fn host_session_unregister_missing_entry_idempotent_ok() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.host.session_unregister",
            json!({"session_id": "ghost"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());
        assert_eq!(resp.result().unwrap()["ok"], true);
    }

    #[tokio::test]
    async fn host_session_unregister_missing_session_id_invalid_params() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.host.session_unregister",
            json!({}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    // ── arc.host.block_start ─────────────────────────────────────────────────

    #[tokio::test]
    async fn host_block_start_emits_mirror_block_start() {
        use beacon_model::{BlockHeader, BlockId, HostInfo, SessionId, TabId};
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::time::SystemTime;

        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s-block").await;
        let mut rx = entry.notif_tx.subscribe();

        let header = BlockHeader {
            id: BlockId::new(),
            session_id: SessionId::new(),
            tab_id: TabId::new(),
            command: "echo hi".to_string(),
            cwd: PathBuf::from("/tmp"),
            started_at: SystemTime::UNIX_EPOCH,
            env_snapshot: HashMap::new(),
            host_info: HostInfo::current(),
        };
        let header_json = serde_json::to_value(&header).unwrap();

        let req = Request::new(
            "arc.host.block_start",
            json!({"session_id": "s-block", "header": header_json}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.block_start");
        assert_eq!(v["params"]["session_id"], "s-block");
        assert_eq!(v["params"]["header"]["command"], "echo hi");
    }

    #[tokio::test]
    async fn host_block_start_unknown_session_server_error() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.host.block_start",
            json!({"session_id": "ghost", "header": {}}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.err().unwrap().message.contains("not found"));
    }

    #[tokio::test]
    async fn host_block_start_invalid_header_invalid_params() {
        let (store, d) = make_dispatcher();
        store.get_or_create("s-block").await;
        let req = Request::new(
            "arc.host.block_start",
            json!({"session_id": "s-block", "header": "not-an-object"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    // ── arc.host.block_chunk ─────────────────────────────────────────────────

    #[tokio::test]
    async fn host_block_chunk_emits_mirror_block_chunk() {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine as _;
        use beacon_model::BlockId;

        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s-chunk").await;
        let mut rx = entry.notif_tx.subscribe();

        let block_id = BlockId::new();
        let payload = b"output bytes".to_vec();

        let req = Request::new(
            "arc.host.block_chunk",
            json!({
                "session_id": "s-chunk",
                "block_id": block_id.to_string(),
                "seq": 42u64,
                "data": BASE64.encode(&payload),
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.block_chunk");
        assert_eq!(v["params"]["session_id"], "s-chunk");
        assert_eq!(v["params"]["block_id"], block_id.to_string());
        assert_eq!(v["params"]["seq"], 42);
        let echoed_b64 = v["params"]["data"].as_str().unwrap();
        let echoed = BASE64.decode(echoed_b64).unwrap();
        assert_eq!(echoed, payload);
    }

    #[tokio::test]
    async fn host_block_chunk_invalid_block_id_invalid_params() {
        let (store, d) = make_dispatcher();
        store.get_or_create("s-chunk").await;
        let req = Request::new(
            "arc.host.block_chunk",
            json!({"session_id": "s-chunk", "block_id": "not-a-ulid", "seq": 0u64, "data": ""}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    #[tokio::test]
    async fn host_block_chunk_invalid_data_invalid_params() {
        use beacon_model::BlockId;
        let (store, d) = make_dispatcher();
        store.get_or_create("s-chunk").await;
        let req = Request::new(
            "arc.host.block_chunk",
            json!({
                "session_id": "s-chunk",
                "block_id": BlockId::new().to_string(),
                "seq": 0u64,
                "data": "not_base64!!!",
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    // ── arc.host.block_end ───────────────────────────────────────────────────

    #[tokio::test]
    async fn host_block_end_emits_mirror_block_end() {
        use beacon_model::{BlockFooter, BlockId};
        use std::time::SystemTime;

        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s-bend").await;
        let mut rx = entry.notif_tx.subscribe();

        let footer = BlockFooter {
            id: BlockId::new(),
            exit_code: Some(0),
            ended_at: SystemTime::UNIX_EPOCH,
            shadow_git_ref: None,
        };
        let footer_json = serde_json::to_value(&footer).unwrap();

        let req = Request::new(
            "arc.host.block_end",
            json!({"session_id": "s-bend", "footer": footer_json}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.block_end");
        assert_eq!(v["params"]["session_id"], "s-bend");
        assert_eq!(v["params"]["footer"]["exit_code"], 0);
    }

    #[tokio::test]
    async fn host_block_end_invalid_footer_invalid_params() {
        let (store, d) = make_dispatcher();
        store.get_or_create("s-bend").await;
        let req = Request::new(
            "arc.host.block_end",
            json!({"session_id": "s-bend", "footer": 123}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    // ── arc.host.session_resize ──────────────────────────────────────────────

    #[tokio::test]
    async fn host_session_resize_returns_ok() {
        let (store, d) = make_dispatcher();
        store.get_or_create("s-resize").await;
        let req = Request::new(
            "arc.host.session_resize",
            json!({"session_id": "s-resize", "cols": 120u32, "rows": 40u32}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());
        assert_eq!(resp.result().unwrap()["ok"], true);
    }

    #[tokio::test]
    async fn host_session_resize_unknown_session_server_error() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.host.session_resize",
            json!({"session_id": "ghost", "cols": 80u32, "rows": 24u32}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.err().unwrap().message.contains("not found"));
    }

    #[tokio::test]
    async fn host_session_resize_emits_no_notif() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s-resize").await;
        let mut rx = entry.notif_tx.subscribe();
        let req = Request::new(
            "arc.host.session_resize",
            json!({"session_id": "s-resize", "cols": 80u32, "rows": 24u32}),
            1i64,
        );
        assert!(d.dispatch(req).await.is_success());
        // Host-driven resize is not broadcast to watchers in msgpack either.
        assert!(rx.try_recv().is_err());
    }

    // ── arc.host.push_output ─────────────────────────────────────────────────

    #[tokio::test]
    async fn host_push_output_emits_mirror_frame() {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine as _;

        // Use a real ULID as session_id — SessionId is a ULID newtype.
        let sid = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

        let (store, mirror_registry, d) = make_dispatcher_with_mirror();
        let entry = store.get_or_create(sid).await;
        // Pre-init the MirrorRegistry entry so push_output finds it.
        mirror_registry
            .get_or_init(sid, uuid::Uuid::nil(), vec![])
            .await;
        let mut rx = entry.notif_tx.subscribe();

        let payload = b"PTY output \x1b[31mred\x1b[0m".to_vec();
        let req = Request::new(
            "arc.host.push_output",
            json!({
                "session_id": sid,
                "data": BASE64.encode(&payload),
                "seq": 99u64,   // bogus host seq — must be discarded
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());
        let result = resp.result().unwrap();
        assert_eq!(result["ok"], true);
        // Canonical seq comes from ScrollbackRing (starts at 0), NOT the host-supplied 99.
        let canonical_seq = result["seq"].as_u64().unwrap();
        assert_ne!(canonical_seq, 99, "host seq must be discarded");

        // Give the ArcMirrorEmitter tokio::spawn a chance to fire.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let raw = rx.recv().await.expect("notif");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["method"], "arc.mirror.frame");
        let echoed = BASE64.decode(v["params"]["data"].as_str().unwrap()).unwrap();
        assert_eq!(echoed, payload);
    }

    #[tokio::test]
    async fn host_push_output_unknown_session_server_error() {
        let (_s, d) = make_dispatcher();
        let req = Request::new(
            "arc.host.push_output",
            json!({"session_id": "ghost", "data": "", "seq": 0u64}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.err().unwrap().message.contains("not found"));
    }

    #[tokio::test]
    async fn host_push_output_invalid_base64_invalid_params() {
        let (store, d) = make_dispatcher();
        store.get_or_create("s-push").await;
        let req = Request::new(
            "arc.host.push_output",
            json!({"session_id": "s-push", "data": "@@@", "seq": 0u64}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    // ── arc.host.shadow_diff_response ────────────────────────────────────────

    #[tokio::test]
    async fn host_shadow_diff_response_silent_ok() {
        let (store, d) = make_dispatcher();
        let entry = store.get_or_create("s-diff").await;
        let mut rx = entry.notif_tx.subscribe();
        let req = Request::new(
            "arc.host.shadow_diff_response",
            json!({
                "session_id": "s-diff",
                "request_id": "req-001",
                "diff": "--- a\n+++ b\n@@ -1 +1 @@\n-x\n+y",
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "{:?}", resp.err());
        assert_eq!(resp.result().unwrap()["ok"], true);
        // No notif on the watcher channel — diff is host→relay, response is consumed
        // by the (Phase F) waiter map, not echoed.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn host_shadow_diff_response_missing_request_id_invalid_params() {
        let (store, d) = make_dispatcher();
        store.get_or_create("s-diff").await;
        let req = Request::new(
            "arc.host.shadow_diff_response",
            json!({"session_id": "s-diff", "diff": ""}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    // ── Phase F: arc.host.session_register relay + mirror wire-up ────────────

    /// Verify that calling `arc.host.session_register` wires the session into
    /// both the `ArcSessionStore` (entry exists) and `MirrorRegistry` (mirror
    /// entry created).  Uses the same lazy-pool dispatcher as other lib tests
    /// so no real DB is required.
    #[tokio::test]
    async fn host_session_register_wires_relay_router() {
        let store = ArcSessionStore::new(64);
        let relay = Arc::new(crate::relay::router::RelayRouter::new());
        let mirror_registry = crate::mirror::MirrorRegistry::default();
        let pool = sqlx::PgPool::connect_lazy("postgres://arc:arc@localhost:5432/arc")
            .expect("lazy pool");
        let user_id = Uuid::nil();

        let mut d = RpcDispatcher::new();
        d.register(ArcHostSessionRegisterHandler::new(
            store.clone(),
            relay.clone(),
            mirror_registry.clone(),
            pool,
            user_id,
        ));

        let sid = "phase-f-lib-test-1";
        let req = Request::new(
            "arc.host.session_register",
            json!({"session_id": sid, "name": "shell"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "register failed: {:?}", resp.err());
        assert_eq!(resp.result().unwrap()["ok"], true);

        // ArcSessionEntry must be in the store.
        assert!(store.get(sid).await.is_some(), "store entry missing after register");

        // Give the async mirror init a moment (get_or_init is awaited inline so
        // it should already be done, but yield once to be safe).
        tokio::task::yield_now().await;

        // MirrorRegistry must have an entry.
        assert!(
            mirror_registry.get(sid).await.is_some(),
            "MirrorRegistry entry missing after register",
        );

        // RelayRouter must have a session registered.
        assert!(
            relay.get_session_info(sid).is_some(),
            "RelayRouter session missing after register",
        );

        // JoinHandles must be stored on the entry.
        let entry = store.get(sid).await.unwrap();
        assert!(
            entry.host_tasks.lock().await.is_some(),
            "host_tasks JoinHandles not stored on ArcSessionEntry",
        );
    }
}
