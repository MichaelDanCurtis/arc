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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArcAiMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ArcAiToolCall>,
}

/// One tool invocation requested by the assistant. Mirrors OpenAI's
/// `tool_calls[]` entry; identical to legacy `RequestToolCall` in `ai_routes.rs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcAiToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ArcAiToolCallFunction,
}

/// Function name and JSON-serialized arguments, matching OpenAI's
/// `tool_calls[].function` shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcAiToolCallFunction {
    pub name: String,
    /// Arguments are an opaque JSON string (matches OpenAI). Backends are
    /// responsible for parsing if they need a structured value.
    pub arguments: String,
}

/// Tool definition supplied by the caller in the request envelope.
/// Mirrors legacy `ToolDef` in `ai_routes.rs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcAiTool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ArcAiToolFunction,
}

/// Tool function metadata: name, description, JSON-Schema parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcAiToolFunction {
    pub name: String,
    pub description: String,
    /// JSON-Schema for the tool's input. Forwarded as-is to upstream.
    pub parameters: Value,
}

/// Response payload returned by [`ArcAiBackend::chat`]. Mirrors the legacy
/// `/v1/ai/chat` `ChatResponse.choices[0].message` shape: `content` may be
/// null when the model only requested tools.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ArcAiChatResponse {
    /// Assistant text. `None` when the model only emitted tool_use blocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Tool calls requested by the model. Empty when the model returned
    /// only text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ArcAiToolCall>,
}

/// Request envelope passed from RPC handlers to the [`ArcAiBackend`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ArcAiChatRequest {
    pub model: String,
    pub messages: Vec<ArcAiMessage>,
    /// Explicit system prompt. If `None`, the backend may extract a
    /// `role:"system"` message from `messages` (legacy behaviour).
    pub system: Option<String>,
    /// Tools advertised to the model. Empty `Vec` = no tools.
    pub tools: Vec<ArcAiTool>,
    /// Sampling temperature forwarded to the upstream provider. `None` uses
    /// provider default.
    pub temperature: Option<f32>,
    /// Token budget for the response. `None` falls back to
    /// `DEFAULT_MAX_TOKENS` in `HttpAiBackend`.
    pub max_tokens: Option<u32>,
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
    /// The model wants to invoke a tool. Forwarded as an `ai.tool_use`
    /// notification so watchers (local or Arc) can render tool chips.
    ToolUse {
        id: String,
        name: String,
        arguments: Value,
    },
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
    /// Non-streaming chat. Returns the assembled response with optional text
    /// content and any tool_use blocks.
    async fn chat(&self, req: &ArcAiChatRequest) -> Result<ArcAiChatResponse, String>;

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

/// Emit `ai.tool_use` into the session's broadcast channel. Reuses the same
/// notification name as the local ai.* slice so watchers can render tool
/// invocations identically whether the chat ran locally or through Arc.
pub fn emit_tool_use(entry: &ArcAiSessionEntry, id: &str, name: &str, arguments: &Value) {
    send_notif(
        &entry.notif_tx,
        "ai.tool_use",
        json!({
            "session_id": entry.session_id,
            "id": id,
            "name": name,
            "arguments": arguments,
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

/// Parse a `ChatRequest`-shaped param object into an [`ArcAiChatRequest`].
/// Required: `model` (non-empty), `messages` (array). Optional: `system`,
/// `tools`, `temperature`, `max_tokens`.
fn parse_chat_request(params: &Value) -> Result<ArcAiChatRequest, RpcError> {
    let model = require_str(params, "model")?.to_string();
    let messages = parse_messages(params)?;

    let system = params
        .get("system")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let tools = match params.get("tools") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => serde_json::from_value::<Vec<ArcAiTool>>(v.clone())
            .map_err(|e| RpcError::invalid_params(format!("invalid 'tools': {e}")))?,
    };

    let temperature = params
        .get("temperature")
        .and_then(|v| v.as_f64())
        .map(|f| f as f32);

    let max_tokens = params
        .get("max_tokens")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);

    Ok(ArcAiChatRequest {
        model,
        messages,
        system,
        tools,
        temperature,
        max_tokens,
    })
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
            let req = parse_chat_request(&params)?;
            match self.backend.chat(&req).await {
                Ok(resp) => serde_json::to_value(&resp)
                    .map_err(|e| RpcError::server_error(format!("serialize response: {e}"))),
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
            let req = parse_chat_request(&params)?;
            let model = req.model.clone();

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
            Ok(ArcAiStreamEvent::ToolUse {
                id,
                name,
                arguments,
            }) => {
                emit_tool_use(entry, &id, &name, &arguments);
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
// Production adapter
// ─────────────────────────────────────────────────────────────────────────────

// Production [`ArcAiBackend`] impl lives in [`crate::ai_backend::HttpAiBackend`].
// It targets the Anthropic Messages API directly (mirroring the legacy
// `/v1/ai/*` HTTP routes in `ai_routes.rs`) and is wired into `AppState`
// from `main.rs`. The test backends below remain in this module so unit
// tests that exercise the rpc handlers in isolation don't pull in reqwest.

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
        async fn chat(&self, _req: &ArcAiChatRequest) -> Result<ArcAiChatResponse, String> {
            Ok(ArcAiChatResponse { content: Some(self.chat_response.clone()), tool_calls: Vec::new() })
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

    /// Mock backend whose stream emits Text → ToolUse → Text → Done. Used to
    /// verify that `ai.tool_use` notifications interleave correctly with
    /// `ai.chunk`s and that the assembled `content` skips tool events.
    struct ToolBackend;

    #[async_trait]
    impl ArcAiBackend for ToolBackend {
        async fn chat(&self, _req: &ArcAiChatRequest) -> Result<ArcAiChatResponse, String> {
            Ok(ArcAiChatResponse { content: Some("beforeafter".into()), tool_calls: Vec::new() })
        }
        async fn stream_chat(&self, _req: &ArcAiChatRequest) -> Result<ArcAiStream, String> {
            let events: Vec<Result<ArcAiStreamEvent, String>> = vec![
                Ok(ArcAiStreamEvent::Text("before".into())),
                Ok(ArcAiStreamEvent::ToolUse {
                    id: "call_1".into(),
                    name: "echo".into(),
                    arguments: json!({"text": "hi"}),
                }),
                Ok(ArcAiStreamEvent::Text("after".into())),
                Ok(ArcAiStreamEvent::Done),
            ];
            Ok(Box::pin(stream::iter(events)))
        }
        async fn list_models(&self) -> Vec<ArcAiModel> {
            vec![]
        }
    }

    /// Mock backend whose `chat` and `stream_chat` always fail.
    struct FailingBackend;

    #[async_trait]
    impl ArcAiBackend for FailingBackend {
        async fn chat(&self, _req: &ArcAiChatRequest) -> Result<ArcAiChatResponse, String> {
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
    async fn stream_chat_emits_tool_use_between_chunks() {
        let backend: Arc<dyn ArcAiBackend> = Arc::new(ToolBackend);
        let (d, store) = build(backend);

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
        let methods: Vec<&str> = events.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(
            methods,
            vec!["ai.chunk", "ai.tool_use", "ai.chunk", "ai.done"]
        );

        let tu = &events[1].1;
        assert_eq!(tu["session_id"], "s1");
        assert_eq!(tu["id"], "call_1");
        assert_eq!(tu["name"], "echo");
        assert_eq!(tu["arguments"], json!({"text": "hi"}));

        // Assembled content drops tool-use; text only.
        assert_eq!(events[3].1["content"], "beforeafter");
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

    #[tokio::test]
    async fn arc_ai_chat_handler_threads_tools_temperature_max_tokens() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct CaptureBackend {
            seen_tools: std::sync::Arc<std::sync::Mutex<Vec<ArcAiTool>>>,
            seen_temperature: std::sync::Arc<std::sync::Mutex<Option<f32>>>,
            seen_max_tokens: std::sync::Arc<std::sync::Mutex<Option<u32>>>,
            seen_system: std::sync::Arc<std::sync::Mutex<Option<String>>>,
            called: std::sync::Arc<AtomicBool>,
        }

        #[async_trait]
        impl ArcAiBackend for CaptureBackend {
            async fn chat(&self, req: &ArcAiChatRequest) -> Result<ArcAiChatResponse, String> {
                *self.seen_tools.lock().unwrap() = req.tools.clone();
                *self.seen_temperature.lock().unwrap() = req.temperature;
                *self.seen_max_tokens.lock().unwrap() = req.max_tokens;
                *self.seen_system.lock().unwrap() = req.system.clone();
                self.called.store(true, Ordering::SeqCst);
                Ok(ArcAiChatResponse { content: Some("ok".into()), tool_calls: Vec::new() })
            }
            async fn stream_chat(&self, _: &ArcAiChatRequest) -> Result<ArcAiStream, String> {
                unimplemented!()
            }
            async fn list_models(&self) -> Vec<ArcAiModel> { vec![] }
        }

        let seen_tools = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_temperature = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen_max_tokens = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen_system = std::sync::Arc::new(std::sync::Mutex::new(None));
        let called = std::sync::Arc::new(AtomicBool::new(false));

        let backend: Arc<dyn ArcAiBackend> = Arc::new(CaptureBackend {
            seen_tools: seen_tools.clone(),
            seen_temperature: seen_temperature.clone(),
            seen_max_tokens: seen_max_tokens.clone(),
            seen_system: seen_system.clone(),
            called: called.clone(),
        });
        let handler = ArcAiChatHandler::new(backend);

        let params = json!({
            "model": "anthropic:claude-haiku-4-5",
            "messages": [{"role": "user", "content": "hi"}],
            "system": "be brief",
            "temperature": 0.5,
            "max_tokens": 256,
            "tools": [{
                "type": "function",
                "function": {
                    "name": "ls",
                    "description": "list files",
                    "parameters": { "type": "object", "properties": {} }
                }
            }]
        });

        let res = handler.handle(params).await.expect("handle ok");
        assert_eq!(res["content"], "ok");
        assert!(called.load(Ordering::SeqCst));
        assert_eq!(seen_temperature.lock().unwrap().unwrap(), 0.5);
        assert_eq!(seen_max_tokens.lock().unwrap().unwrap(), 256);
        assert_eq!(seen_system.lock().unwrap().as_deref(), Some("be brief"));
        let tools = seen_tools.lock().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].function.name, "ls");
    }

    #[tokio::test]
    async fn arc_ai_chat_handler_serializes_tool_calls() {
        struct ToolCallBackend;

        #[async_trait]
        impl ArcAiBackend for ToolCallBackend {
            async fn chat(&self, _req: &ArcAiChatRequest) -> Result<ArcAiChatResponse, String> {
                Ok(ArcAiChatResponse {
                    content: None,
                    tool_calls: vec![ArcAiToolCall {
                        id: "call_1".into(),
                        kind: "function".into(),
                        function: ArcAiToolCallFunction {
                            name: "ls".into(),
                            arguments: "{\"path\":\".\"}".into(),
                        },
                    }],
                })
            }
            async fn stream_chat(&self, _: &ArcAiChatRequest) -> Result<ArcAiStream, String> {
                unimplemented!()
            }
            async fn list_models(&self) -> Vec<ArcAiModel> { vec![] }
        }

        let handler = ArcAiChatHandler::new(Arc::new(ToolCallBackend));
        let res = handler.handle(json!({
            "model": "anthropic:claude-haiku-4-5",
            "messages": [{"role": "user", "content": "list files"}]
        })).await.expect("ok");

        assert!(res.get("content").is_none());
        let calls = res.get("tool_calls").expect("tool_calls").as_array().expect("array");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["id"], "call_1");
        assert_eq!(calls[0]["function"]["name"], "ls");
    }

    #[test]
    fn arc_ai_message_round_trips_tool_call_fields() {
        let json = serde_json::json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": "call_42",
                "type": "function",
                "function": { "name": "ls", "arguments": "{\"path\":\".\"}" }
            }]
        });
        let m: ArcAiMessage = serde_json::from_value(json).expect("parse");
        assert_eq!(m.role, "assistant");
        assert_eq!(m.content, None);
        assert_eq!(m.tool_calls.len(), 1);
        assert_eq!(m.tool_calls[0].id, "call_42");
        assert_eq!(m.tool_calls[0].kind, "function");
        assert_eq!(m.tool_calls[0].function.name, "ls");
        assert_eq!(m.tool_calls[0].function.arguments, "{\"path\":\".\"}");

        let tool_msg: ArcAiMessage = serde_json::from_value(serde_json::json!({
            "role": "tool",
            "content": "ok",
            "tool_call_id": "call_42"
        })).expect("parse tool msg");
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_42"));
    }
}
