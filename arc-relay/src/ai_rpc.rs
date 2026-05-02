//! JSON-RPC handlers for `arc.ai.*` methods.
//!
//! Final P7 jsonrpc cut-over sub-slice. Mirrors the local `ai.*` slice in
//! `beacon-multi-agent::ai_rpc` but routes through Arc's HTTP-fronted model
//! gateway instead of a local `Provider`. Spec lives at
//! `arc-relay/docs/ai-route.md`; the legacy HTTP fan-out implementation stays
//! at `arc-relay/src/api/ai_routes.rs` and can be wrapped by an
//! [`ArcAiBackend`] impl once the WS endpoint cuts over to jsonrpc.
//!
//! Streaming reuses the existing `ai.chunk` / `ai.done` / `ai.error`
//! notification surface so that watchers can subscribe with the same code
//! whether the chat ran locally or through Arc.
//!
//! Gated by [`beacon_features::FeatureFlag::JsonRpcSpine`]. When the flag is
//! OFF the legacy `/v1/ai/*` HTTP routes remain authoritative.
//!
//! # Wire format — methods
//!
//! ```text
//! arc.ai.chat        {"model":"<id>","messages":[...]}                   → {"content":"..."}
//! arc.ai.stream_chat {"session_id":"<id>","model":"<id>","messages":[..]}→ {"ok":true}
//! arc.ai.list_models {}                                                  → {"models":[{"id","name","context_window?"}]}
//! ```
//!
//! `arc.ai.stream_chat` is fire-and-forget on the request side; the caller
//! subscribes to the session's broadcast channel to receive the streamed
//! response (`ai.chunk` / `ai.done` / `ai.error`). A `session_id` parameter
//! is required even though it is absent from the original method-table
//! sketch — without it concurrent streams would interleave on a single
//! channel and watchers would have no way to demultiplex.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use beacon_jsonrpc::{BoxFuture, Notification, RpcDispatcher, RpcError, RpcHandler};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex};

// ─────────────────────────────────────────────────────────────────────────────
// Public types — backend trait + DTOs
// ─────────────────────────────────────────────────────────────────────────────

/// Single chat message in the OpenAI-compat shape used on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcAiMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Request envelope passed from RPC handlers to the [`ArcAiBackend`].
#[derive(Debug, Clone, Default)]
pub struct ArcAiChatRequest {
    pub model: String,
    pub messages: Vec<ArcAiMessage>,
}

/// One model entry returned by `arc.ai.list_models`.
#[derive(Debug, Clone, Serialize)]
pub struct ArcAiModel {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

/// Stream events emitted by [`ArcAiBackend::stream_chat`].
#[derive(Debug, Clone)]
pub enum ArcAiStreamEvent {
    /// Partial text delta to append to the assistant message.
    Text(String),
    /// Stream finished cleanly. Emitted before the underlying `Stream` drops.
    Done,
    /// Stream produced an in-band error and is terminating.
    Error(String),
}

/// Boxed stream of [`ArcAiStreamEvent`]s used by [`ArcAiBackend::stream_chat`].
pub type ArcAiStream =
    Pin<Box<dyn Stream<Item = Result<ArcAiStreamEvent, String>> + Send>>;

/// Pluggable AI backend the rpc handlers fan out to.
///
/// Production wires this against the relay's HTTP-fronted gateway (the same
/// code path that powers `arc-relay/src/api/ai_routes.rs`). Tests use mock
/// implementations to assert event ordering without an external dependency.
#[async_trait]
pub trait ArcAiBackend: Send + Sync {
    /// Non-streaming chat. Returns the assembled assistant content.
    async fn chat(&self, req: &ArcAiChatRequest) -> Result<String, String>;

    /// Streaming chat. Returns a boxed stream of events terminating in
    /// `Done` (or `Error` on failure).
    async fn stream_chat(&self, req: &ArcAiChatRequest) -> Result<ArcAiStream, String>;

    /// List the models exposed by this backend.
    async fn list_models(&self) -> Vec<ArcAiModel>;
}

// ─────────────────────────────────────────────────────────────────────────────
// Session store
// ─────────────────────────────────────────────────────────────────────────────

/// Per-arc-ai-session state held by the rpc layer. Each entry owns the
/// broadcast channel that watchers subscribe to in order to receive
/// `ai.chunk` / `ai.done` / `ai.error` notifications scoped to a single
/// `stream_chat` call.
#[derive(Debug)]
pub struct ArcAiSessionEntry {
    pub session_id: String,
    pub notif_tx: broadcast::Sender<String>,
}

/// Thread-safe `session_id` → [`ArcAiSessionEntry`] map. Sessions are
/// allocated lazily on the first `arc.ai.stream_chat` call.
#[derive(Clone)]
pub struct ArcAiSessionStore {
    inner: Arc<Mutex<HashMap<String, Arc<ArcAiSessionEntry>>>>,
    notif_capacity: usize,
}

impl ArcAiSessionStore {
    pub fn new(notif_capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            notif_capacity,
        }
    }

    /// Get an existing entry or insert a fresh one with a new broadcast
    /// channel. Returns the entry.
    pub async fn get_or_create(&self, session_id: &str) -> Arc<ArcAiSessionEntry> {
        let mut guard = self.inner.lock().await;
        if let Some(e) = guard.get(session_id) {
            return e.clone();
        }
        let (notif_tx, _) = broadcast::channel::<String>(self.notif_capacity);
        let entry = Arc::new(ArcAiSessionEntry {
            session_id: session_id.to_string(),
            notif_tx,
        });
        guard.insert(session_id.to_string(), entry.clone());
        entry
    }

    pub async fn get(&self, session_id: &str) -> Option<Arc<ArcAiSessionEntry>> {
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

impl Default for ArcAiSessionStore {
    fn default() -> Self {
        Self::new(64)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Notification helpers (reuse local ai.* method names)
// ─────────────────────────────────────────────────────────────────────────────

fn send_notif(tx: &broadcast::Sender<String>, method: &'static str, params: Value) {
    let n = Notification::new(method, params);
    let _ = tx.send(serde_json::to_string(&n).unwrap_or_default());
}

/// Emit `ai.chunk` into the session's broadcast channel.
pub fn emit_chunk(entry: &ArcAiSessionEntry, delta: &str) {
    send_notif(
        &entry.notif_tx,
        "ai.chunk",
        json!({"session_id": entry.session_id, "delta": delta}),
    );
}

/// Emit `ai.done` into the session's broadcast channel.
pub fn emit_done(entry: &ArcAiSessionEntry, model: &str, full_content: &str) {
    send_notif(
        &entry.notif_tx,
        "ai.done",
        json!({
            "session_id": entry.session_id,
            "model": model,
            "content": full_content,
        }),
    );
}

/// Emit `ai.error` into the session's broadcast channel.
pub fn emit_error(entry: &ArcAiSessionEntry, error: &str) {
    send_notif(
        &entry.notif_tx,
        "ai.error",
        json!({"session_id": entry.session_id, "error": error}),
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

fn parse_messages(params: &Value) -> Result<Vec<ArcAiMessage>, RpcError> {
    let arr = params
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| RpcError::invalid_params("missing 'messages' array".to_string()))?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        let m: ArcAiMessage = serde_json::from_value(v.clone())
            .map_err(|e| RpcError::invalid_params(format!("invalid message: {e}")))?;
        out.push(m);
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.ai.chat
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.ai.chat`. Calls [`ArcAiBackend::chat`] and returns
/// `{content: <assembled assistant text>}`.
pub struct ArcAiChatHandler {
    backend: Arc<dyn ArcAiBackend>,
}

impl ArcAiChatHandler {
    pub fn new(backend: Arc<dyn ArcAiBackend>) -> Self {
        Self { backend }
    }
}

impl RpcHandler for ArcAiChatHandler {
    fn method_name(&self) -> &'static str {
        "arc.ai.chat"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let model = require_str(&params, "model")?.to_string();
            let messages = parse_messages(&params)?;
            let req = ArcAiChatRequest { model, messages };
            match self.backend.chat(&req).await {
                Ok(content) => Ok(json!({"content": content})),
                Err(e) => Err(RpcError::server_error(e)),
            }
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.ai.stream_chat
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.ai.stream_chat`. Spawns a task that drives the backend's
/// stream and emits `ai.chunk` / `ai.done` / `ai.error` notifications into
/// the session's broadcast channel. Returns `{ok:true}` immediately as an
/// acknowledgement.
pub struct ArcAiStreamChatHandler {
    backend: Arc<dyn ArcAiBackend>,
    store: ArcAiSessionStore,
}

impl ArcAiStreamChatHandler {
    pub fn new(backend: Arc<dyn ArcAiBackend>, store: ArcAiSessionStore) -> Self {
        Self { backend, store }
    }
}

impl RpcHandler for ArcAiStreamChatHandler {
    fn method_name(&self) -> &'static str {
        "arc.ai.stream_chat"
    }

    fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let session_id = require_str(&params, "session_id")?.to_string();
            let model = require_str(&params, "model")?.to_string();
            let messages = parse_messages(&params)?;
            let req = ArcAiChatRequest {
                model: model.clone(),
                messages,
            };

            let entry = self.store.get_or_create(&session_id).await;
            let backend = self.backend.clone();
            let entry_for_task = entry.clone();
            let model_for_task = model;
            tokio::spawn(async move {
                stream_and_emit(backend.as_ref(), &entry_for_task, &model_for_task, req).await;
            });

            Ok(json!({"ok": true}))
        })
    }
}

async fn stream_and_emit(
    backend: &dyn ArcAiBackend,
    entry: &ArcAiSessionEntry,
    model: &str,
    req: ArcAiChatRequest,
) {
    let mut stream = match backend.stream_chat(&req).await {
        Ok(s) => s,
        Err(e) => {
            emit_error(entry, &e);
            return;
        }
    };

    let mut full = String::new();
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(ArcAiStreamEvent::Text(t)) => {
                full.push_str(&t);
                emit_chunk(entry, &t);
            }
            Ok(ArcAiStreamEvent::Done) => break,
            Ok(ArcAiStreamEvent::Error(msg)) => {
                emit_error(entry, &msg);
                return;
            }
            Err(e) => {
                emit_error(entry, &e);
                return;
            }
        }
    }

    emit_done(entry, model, &full);
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.ai.list_models
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for `arc.ai.list_models`. Returns `{"models": [...]}`.
pub struct ArcAiListModelsHandler {
    backend: Arc<dyn ArcAiBackend>,
}

impl ArcAiListModelsHandler {
    pub fn new(backend: Arc<dyn ArcAiBackend>) -> Self {
        Self { backend }
    }
}

impl RpcHandler for ArcAiListModelsHandler {
    fn method_name(&self) -> &'static str {
        "arc.ai.list_models"
    }

    fn handle<'a>(&'a self, _params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let models = self.backend.list_models().await;
            Ok(json!({"models": models}))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Registration
// ─────────────────────────────────────────────────────────────────────────────

/// Build a fresh dispatcher with all three `arc.ai.*` handlers registered.
/// Mainly used in tests; production wires through [`register_if_enabled`].
pub fn build_arc_ai_dispatcher(
    backend: Arc<dyn ArcAiBackend>,
    store: ArcAiSessionStore,
) -> RpcDispatcher {
    let mut d = RpcDispatcher::new();
    d.register(ArcAiChatHandler::new(backend.clone()));
    d.register(ArcAiStreamChatHandler::new(backend.clone(), store));
    d.register(ArcAiListModelsHandler::new(backend));
    d
}

/// Register all three `arc.ai.*` handlers with `dispatcher` if
/// [`beacon_features::FeatureFlag::JsonRpcSpine`] is enabled. Returns `true`
/// when handlers were registered.
pub fn register_if_enabled(
    dispatcher: &mut RpcDispatcher,
    backend: Arc<dyn ArcAiBackend>,
    store: ArcAiSessionStore,
) -> bool {
    if beacon_features::is_enabled(beacon_features::FeatureFlag::JsonRpcSpine) {
        dispatcher.try_register(ArcAiChatHandler::new(backend.clone()));
        dispatcher.try_register(ArcAiStreamChatHandler::new(backend.clone(), store));
        dispatcher.try_register(ArcAiListModelsHandler::new(backend));
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
    use futures_util::stream;

    // ── Mock backends ────────────────────────────────────────────────────────

    /// Mock backend that returns a fixed chat response, fixed stream chunks,
    /// and a fixed model list.
    struct MockBackend {
        chat_response: String,
        stream_chunks: Vec<String>,
        models: Vec<ArcAiModel>,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                chat_response: "hello".into(),
                stream_chunks: vec!["hel".into(), "lo".into()],
                models: vec![
                    ArcAiModel {
                        id: "anthropic:claude-opus-4-5".into(),
                        name: Some("Claude Opus 4.5".into()),
                        context_window: Some(200_000),
                    },
                ],
            }
        }
    }

    #[async_trait]
    impl ArcAiBackend for MockBackend {
        async fn chat(&self, _req: &ArcAiChatRequest) -> Result<String, String> {
            Ok(self.chat_response.clone())
        }

        async fn stream_chat(&self, _req: &ArcAiChatRequest) -> Result<ArcAiStream, String> {
            let mut events: Vec<Result<ArcAiStreamEvent, String>> = self
                .stream_chunks
                .iter()
                .map(|c| Ok(ArcAiStreamEvent::Text(c.clone())))
                .collect();
            events.push(Ok(ArcAiStreamEvent::Done));
            Ok(Box::pin(stream::iter(events)))
        }

        async fn list_models(&self) -> Vec<ArcAiModel> {
            self.models.clone()
        }
    }

    /// Mock backend whose `chat` and `stream_chat` always fail.
    struct FailingBackend;

    #[async_trait]
    impl ArcAiBackend for FailingBackend {
        async fn chat(&self, _req: &ArcAiChatRequest) -> Result<String, String> {
            Err("upstream offline".into())
        }

        async fn stream_chat(&self, _req: &ArcAiChatRequest) -> Result<ArcAiStream, String> {
            Err("upstream offline".into())
        }

        async fn list_models(&self) -> Vec<ArcAiModel> {
            vec![]
        }
    }

    fn mock_backend() -> Arc<dyn ArcAiBackend> {
        Arc::new(MockBackend::new())
    }

    fn build(backend: Arc<dyn ArcAiBackend>) -> (RpcDispatcher, ArcAiSessionStore) {
        let store = ArcAiSessionStore::new(64);
        (build_arc_ai_dispatcher(backend, store.clone()), store)
    }

    /// Drain a broadcast receiver until `ai.done` or `ai.error` arrives.
    /// Returns the collected `(method, params)` pairs.
    async fn drain_until_terminal(rx: &mut broadcast::Receiver<String>) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        loop {
            let raw = match tokio::time::timeout(
                std::time::Duration::from_secs(2),
                rx.recv(),
            )
            .await
            {
                Ok(Ok(s)) => s,
                _ => return out,
            };
            let parsed: Value = serde_json::from_str(&raw).unwrap();
            let method = parsed["method"].as_str().unwrap().to_string();
            let params = parsed["params"].clone();
            let terminal = method == "ai.done" || method == "ai.error";
            out.push((method, params));
            if terminal {
                return out;
            }
        }
    }

    // ── arc.ai.chat ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn chat_returns_content() {
        let (d, _) = build(mock_backend());
        let req = Request::new(
            "arc.ai.chat",
            json!({
                "model": "anthropic:claude-opus-4-5",
                "messages": [{"role": "user", "content": "hi"}],
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "chat failed: {:?}", resp.err());
        let result = resp.result().unwrap();
        assert_eq!(result["content"], "hello");
    }

    #[tokio::test]
    async fn chat_missing_model_invalid_params() {
        let (d, _) = build(mock_backend());
        let req = Request::new(
            "arc.ai.chat",
            json!({"messages": [{"role": "user", "content": "hi"}]}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    #[tokio::test]
    async fn chat_missing_messages_invalid_params() {
        let (d, _) = build(mock_backend());
        let req = Request::new(
            "arc.ai.chat",
            json!({"model": "anthropic:claude-opus-4-5"}),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    #[tokio::test]
    async fn chat_backend_failure_returns_server_error() {
        let backend: Arc<dyn ArcAiBackend> = Arc::new(FailingBackend);
        let (d, _) = build(backend);
        let req = Request::new(
            "arc.ai.chat",
            json!({
                "model": "anthropic:claude-opus-4-5",
                "messages": [{"role": "user", "content": "hi"}],
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        let err = resp.err().unwrap();
        assert!(err.message.contains("upstream offline"));
    }

    // ── arc.ai.stream_chat ───────────────────────────────────────────────────

    #[tokio::test]
    async fn stream_chat_emits_chunks_then_done() {
        let (d, store) = build(mock_backend());

        // Pre-create the session + subscribe BEFORE dispatching so we don't
        // race the spawned stream task.
        let _ = store.get_or_create("s1").await;
        let mut rx = store.subscribe("s1").await.expect("subscribe");

        let req = Request::new(
            "arc.ai.stream_chat",
            json!({
                "session_id": "s1",
                "model": "anthropic:claude-opus-4-5",
                "messages": [{"role": "user", "content": "hi"}],
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "stream_chat ack failed: {:?}", resp.err());
        assert_eq!(resp.result().unwrap()["ok"], true);

        let events = drain_until_terminal(&mut rx).await;
        let methods: Vec<&str> = events.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(methods, vec!["ai.chunk", "ai.chunk", "ai.done"]);
        assert_eq!(events[0].1["delta"], "hel");
        assert_eq!(events[1].1["delta"], "lo");
        assert_eq!(events[2].1["content"], "hello");
        assert_eq!(events[2].1["model"], "anthropic:claude-opus-4-5");
        assert_eq!(events[2].1["session_id"], "s1");
    }

    #[tokio::test]
    async fn stream_chat_backend_error_emits_ai_error() {
        let backend: Arc<dyn ArcAiBackend> = Arc::new(FailingBackend);
        let (d, store) = build(backend);

        // Pre-create the session so we can subscribe before the stream task runs.
        let _ = store.get_or_create("s1").await;
        let mut rx = store.subscribe("s1").await.expect("subscribe");

        let req = Request::new(
            "arc.ai.stream_chat",
            json!({
                "session_id": "s1",
                "model": "anthropic:claude-opus-4-5",
                "messages": [{"role": "user", "content": "hi"}],
            }),
            1i64,
        );
        assert!(d.dispatch(req).await.is_success());

        let events = drain_until_terminal(&mut rx).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "ai.error");
        assert!(events[0].1["error"].as_str().unwrap().contains("upstream offline"));
    }

    #[tokio::test]
    async fn stream_chat_missing_session_id_invalid_params() {
        let (d, _) = build(mock_backend());
        let req = Request::new(
            "arc.ai.stream_chat",
            json!({
                "model": "anthropic:claude-opus-4-5",
                "messages": [],
            }),
            1i64,
        );
        let resp = d.dispatch(req).await;
        assert_eq!(resp.err().unwrap().code, -32602);
    }

    // ── arc.ai.list_models ───────────────────────────────────────────────────

    #[tokio::test]
    async fn list_models_returns_backend_list() {
        let (d, _) = build(mock_backend());
        let req = Request::new("arc.ai.list_models", json!({}), 1i64);
        let resp = d.dispatch(req).await;
        assert!(resp.is_success(), "list_models failed: {:?}", resp.err());
        let result = resp.result().unwrap();
        let models = result["models"].as_array().expect("models array");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["id"], "anthropic:claude-opus-4-5");
        assert_eq!(models[0]["name"], "Claude Opus 4.5");
        assert_eq!(models[0]["context_window"], 200_000u64);
    }

    // ── store / registration ─────────────────────────────────────────────────

    #[tokio::test]
    async fn session_store_get_or_create_idempotent() {
        let store = ArcAiSessionStore::new(8);
        let a = store.get_or_create("s").await;
        let b = store.get_or_create("s").await;
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(store.session_count().await, 1);
    }

    #[tokio::test]
    async fn build_dispatcher_registers_all_three() {
        let (d, _) = build(mock_backend());
        let methods = d.registered_methods();
        assert!(methods.contains(&"arc.ai.chat"));
        assert!(methods.contains(&"arc.ai.stream_chat"));
        assert!(methods.contains(&"arc.ai.list_models"));
    }
}
