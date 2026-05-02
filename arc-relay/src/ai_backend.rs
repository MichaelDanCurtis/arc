//! Production [`ArcAiBackend`] adapter that fans out to the Anthropic
//! Messages API or the OpenAI Chat Completions API depending on the provider
//! prefix in the model id.
//!
//! Mirrors the upstream pattern used by `arc-relay/src/api/ai_routes.rs`
//! but returns domain-typed [`ArcAiStreamEvent`] values instead of
//! axum-tied SSE response streams. Configurable base URLs so integration
//! tests can target mock servers.
//!
//! Reads `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` from the process environment
//! at each call. When unset, [`HttpAiBackend::list_models`] omits those
//! provider's entries and [`HttpAiBackend::chat`] / [`HttpAiBackend::stream_chat`]
//! return an `Err` with a `not configured` message.
//!
//! Provider prefix convention:
//! - `"anthropic:<bare>"` → forward `<bare>` to the Anthropic Messages API.
//! - `"openai:<bare>"` → forward `<bare>` to OpenAI Chat Completions.
//! - No prefix → assume Anthropic.
//! - Other prefixes → `Err("provider '...' is not implemented on this relay")`.

use std::collections::HashMap;
use std::pin::Pin;

use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::ai_rpc::{
    ArcAiBackend, ArcAiChatRequest, ArcAiChatResponse, ArcAiMessage, ArcAiModel, ArcAiStream,
    ArcAiStreamEvent, ArcAiTool, ArcAiToolCall, ArcAiToolCallFunction,
};

const ANTHROPIC_VERSION_HEADER: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 1024;
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com";

/// Production AI backend. Dispatches to Anthropic Messages API or OpenAI
/// Chat Completions based on the provider prefix in the model id.
pub struct HttpAiBackend {
    client: reqwest::Client,
    base_url: String,
    openai_base_url: Option<String>,
}

impl HttpAiBackend {
    /// Production constructor. Uses canonical upstream URLs and a fresh
    /// default `reqwest::Client`.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            openai_base_url: None,
        }
    }

    /// Test constructor — override the Anthropic base URL.
    /// `base_url` MUST NOT include a trailing slash.
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Self::new()
        }
    }

    /// Test constructor — override the OpenAI base URL.
    /// `url` MUST NOT include a trailing slash.
    pub fn with_openai_base_url(mut self, url: impl Into<String>) -> Self {
        self.openai_base_url = Some(url.into());
        self
    }

    fn anthropic_messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url)
    }

    fn openai_chat_completions_url(&self) -> String {
        let base = self
            .openai_base_url
            .as_deref()
            .unwrap_or(DEFAULT_OPENAI_BASE_URL);
        format!("{base}/v1/chat/completions")
    }
}

impl Default for HttpAiBackend {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider parsing
// ─────────────────────────────────────────────────────────────────────────────

/// Split `"anthropic:claude-haiku-4-5"` into `("anthropic", "claude-haiku-4-5")`.
/// No prefix → `("anthropic", input)`.
fn parse_provider(model: &str) -> (&str, &str) {
    model.split_once(':').unwrap_or(("anthropic", model))
}

// ─────────────────────────────────────────────────────────────────────────────
// Anthropic wire types (request)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize, Clone)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

#[derive(Debug, Serialize, Clone)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<AnthropicBlock>),
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type")]
enum AnthropicBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: Value,
}

// ─────────────────────────────────────────────────────────────────────────────
// Anthropic wire types (non-streaming response)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    content: Vec<AnthropicResponseBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicResponseBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Anthropic wire types (streaming SSE)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicEvent {
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: AnthropicBlockStart,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: AnthropicDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "error")]
    Error { error: Value },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicBlockStart {
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

// ─────────────────────────────────────────────────────────────────────────────
// OpenAI wire types (request)
// ─────────────────────────────────────────────────────────────────────────────

/// Owned version used for both production serialisation and unit tests
/// (no lifetime parameters, so tests don't need to construct temporaries).
#[derive(Debug, Serialize)]
pub struct OpenAiRequestOwned {
    pub model: String,
    pub messages: Vec<ArcAiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ArcAiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

// ─────────────────────────────────────────────────────────────────────────────
// OpenAI wire types (non-streaming response)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiResponseMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ArcAiToolCall>>,
}

// ─────────────────────────────────────────────────────────────────────────────
// OpenAI wire types (streaming SSE delta)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OpenAiStreamChunk {
    #[serde(default)]
    choices: Vec<OpenAiStreamChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    delta: OpenAiDelta,
}

#[derive(Debug, Deserialize)]
struct OpenAiDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallDelta {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OpenAiFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAiFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Request building
// ─────────────────────────────────────────────────────────────────────────────

/// Build an Anthropic Messages API request from an `ArcAiChatRequest`.
///
/// - `system`: prefer `req.system`, else extract first `role:"system"` message.
/// - `tools`: forward all `req.tools` as Anthropic tool definitions.
/// - `temperature`: forward as-is (skipped from wire when `None`).
/// - `max_tokens`: prefer `req.max_tokens`, else `DEFAULT_MAX_TOKENS`.
/// - Messages with `role:"tool"` become Anthropic `role:"user"` with a
///   `tool_result` content block. Assistant messages with `tool_calls`
///   become a blocks array mixing `text` + `tool_use`. Plain messages stay
///   as `AnthropicContent::Text`.
fn build_anthropic_request(
    bare_model: &str,
    req: &ArcAiChatRequest,
    stream: bool,
) -> AnthropicRequest {
    let system = req.system.clone().or_else(|| {
        req.messages
            .iter()
            .find(|m| m.role == "system")
            .and_then(|m| m.content.clone())
    });

    let messages = build_anthropic_messages(&req.messages);

    let tools = req
        .tools
        .iter()
        .map(|t| AnthropicTool {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            input_schema: t.function.parameters.clone(),
        })
        .collect();

    AnthropicRequest {
        model: bare_model.to_string(),
        max_tokens: req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        messages,
        system,
        stream: stream.then_some(true),
        tools,
        temperature: req.temperature,
    }
}

/// Map OpenAI-compat messages to Anthropic wire format.
/// - `role:"system"` is skipped (extracted by the caller).
/// - `role:"tool"` becomes `role:"user"` with a single `tool_result` block.
/// - `role:"assistant"` with non-empty `tool_calls` becomes a blocks array
///   (`text` block if content present, then one `tool_use` block per call).
/// - All other roles pass through with `AnthropicContent::Text`.
fn build_anthropic_messages(messages: &[ArcAiMessage]) -> Vec<AnthropicMessage> {
    let mut out = Vec::new();

    for msg in messages {
        match msg.role.as_str() {
            "system" => continue, // handled separately
            "tool" => {
                let content_text = msg.content.clone().unwrap_or_default();
                let tool_use_id = msg.tool_call_id.clone().unwrap_or_default();
                out.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: AnthropicContent::Blocks(vec![AnthropicBlock::ToolResult {
                        tool_use_id,
                        content: content_text,
                    }]),
                });
            }
            "assistant" if !msg.tool_calls.is_empty() => {
                let mut blocks: Vec<AnthropicBlock> = Vec::new();
                if let Some(text) = &msg.content {
                    if !text.is_empty() {
                        blocks.push(AnthropicBlock::Text { text: text.clone() });
                    }
                }
                for tc in &msg.tool_calls {
                    // arguments is an opaque JSON string from the model; fall back to {} rather
                    // than propagating a parse error that would abort the entire tool roundtrip.
                    let input: Value = serde_json::from_str(&tc.function.arguments)
                        .unwrap_or(Value::Object(Default::default()));
                    blocks.push(AnthropicBlock::ToolUse {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                        input,
                    });
                }
                out.push(AnthropicMessage {
                    role: "assistant".to_string(),
                    content: AnthropicContent::Blocks(blocks),
                });
            }
            role => {
                out.push(AnthropicMessage {
                    role: role.to_string(),
                    content: AnthropicContent::Text(msg.content.clone().unwrap_or_default()),
                });
            }
        }
    }

    out
}

/// Build an OpenAI Chat Completions request from an `ArcAiChatRequest`.
///
/// - If `req.system` is `Some(s)` AND there is no `role:"system"` message
///   at index 0, prepend one.
/// - If `req.system` is `None` and a `role:"system"` already exists in
///   `messages`, leave it unchanged.
/// - `tools`: forwarded as-is (OpenAI shape matches `ArcAiTool`).
/// - `max_tokens`, `temperature`, `stream`: forwarded as-is.
pub fn build_openai_request(
    bare_model: &str,
    req: &ArcAiChatRequest,
    stream: bool,
) -> OpenAiRequestOwned {
    let has_system_at_front = req
        .messages
        .first()
        .map(|m| m.role == "system")
        .unwrap_or(false);

    let messages: Vec<ArcAiMessage> = if let Some(sys) = &req.system {
        if !has_system_at_front {
            let mut v = Vec::with_capacity(req.messages.len() + 1);
            v.push(ArcAiMessage {
                role: "system".to_string(),
                content: Some(sys.clone()),
                ..Default::default()
            });
            v.extend(req.messages.iter().cloned());
            v
        } else {
            req.messages.clone()
        }
    } else {
        req.messages.clone()
    };

    OpenAiRequestOwned {
        model: bare_model.to_string(),
        messages,
        tools: req.tools.clone(),
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        stream: stream.then_some(true),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Anthropic SSE parser
// ─────────────────────────────────────────────────────────────────────────────

struct BlockAccum {
    is_tool: bool,
    id: String,
    name: String,
    args_buf: String,
}

type SseByteStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>;
type SseUnfoldState = (
    SseByteStream,
    Vec<u8>,
    std::collections::HashMap<usize, BlockAccum>,
    Vec<Result<ArcAiStreamEvent, String>>,
    bool,
);

fn parse_anthropic_sse<S>(byte_stream: S) -> ArcAiStream
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    let initial: SseUnfoldState = (
        Box::pin(byte_stream),
        Vec::new(),
        Default::default(),
        Vec::new(),
        false,
    );

    let s = futures_util::stream::unfold(
        initial,
        |(mut byte_stream, mut buf, mut blocks, mut pending, mut done)| async move {
            if let Some(ev) = pending.pop() {
                return Some((ev, (byte_stream, buf, blocks, pending, done)));
            }
            if done {
                return None;
            }

            loop {
                if let Some(nl) = buf.iter().position(|&b| b == b'\n') {
                    // Decode the completed line. Lines are bounded by ASCII
                    // '\n' so all multi-byte chars within fit; from_utf8_lossy
                    // is only lossy for actually-malformed UTF-8.
                    let raw = &buf[..nl];
                    let raw = if raw.last() == Some(&b'\r') {
                        &raw[..raw.len() - 1]
                    } else {
                        raw
                    };
                    let line = String::from_utf8_lossy(raw).into_owned();
                    buf.drain(..=nl);

                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    let data = match line.strip_prefix("data: ") {
                        Some(d) => d.trim().to_string(),
                        None => continue,
                    };

                    let event: AnthropicEvent = match serde_json::from_str(&data) {
                        Ok(e) => e,
                        Err(_) => continue,
                    };

                    match event {
                        AnthropicEvent::ContentBlockStart {
                            index,
                            content_block,
                        } => {
                            match content_block {
                                AnthropicBlockStart::Text => {
                                    blocks.insert(
                                        index,
                                        BlockAccum {
                                            is_tool: false,
                                            id: String::new(),
                                            name: String::new(),
                                            args_buf: String::new(),
                                        },
                                    );
                                }
                                AnthropicBlockStart::ToolUse { id, name } => {
                                    blocks.insert(
                                        index,
                                        BlockAccum {
                                            is_tool: true,
                                            id,
                                            name,
                                            args_buf: String::new(),
                                        },
                                    );
                                }
                            }
                            continue;
                        }
                        AnthropicEvent::ContentBlockDelta { index, delta } => match delta {
                            AnthropicDelta::TextDelta { text } => {
                                return Some((
                                    Ok(ArcAiStreamEvent::Text(text)),
                                    (byte_stream, buf, blocks, pending, done),
                                ));
                            }
                            AnthropicDelta::InputJsonDelta { partial_json } => {
                                if let Some(b) = blocks.get_mut(&index) {
                                    b.args_buf.push_str(&partial_json);
                                }
                                continue;
                            }
                        },
                        AnthropicEvent::ContentBlockStop { index } => {
                            if let Some(b) = blocks.remove(&index) {
                                if b.is_tool {
                                    let arguments: Value = if b.args_buf.is_empty() {
                                        json!({})
                                    } else {
                                        serde_json::from_str(&b.args_buf).unwrap_or(json!({}))
                                    };
                                    return Some((
                                        Ok(ArcAiStreamEvent::ToolUse {
                                            id: b.id,
                                            name: b.name,
                                            arguments,
                                        }),
                                        (byte_stream, buf, blocks, pending, done),
                                    ));
                                }
                            }
                            continue;
                        }
                        AnthropicEvent::MessageStop => {
                            done = true;
                            return Some((
                                Ok(ArcAiStreamEvent::Done),
                                (byte_stream, buf, blocks, pending, done),
                            ));
                        }
                        AnthropicEvent::Error { error } => {
                            done = true;
                            pending.push(Ok(ArcAiStreamEvent::Done));
                            let msg = serde_json::to_string(&error)
                                .unwrap_or_else(|_| "upstream error".to_string());
                            return Some((
                                Ok(ArcAiStreamEvent::Error(msg)),
                                (byte_stream, buf, blocks, pending, done),
                            ));
                        }
                        AnthropicEvent::Other => continue,
                    }
                }

                match byte_stream.next().await {
                    Some(Ok(chunk)) => {
                        buf.extend_from_slice(&chunk);
                    }
                    Some(Err(e)) => {
                        done = true;
                        return Some((
                            Err(format!("upstream stream error: {e}")),
                            (byte_stream, buf, blocks, pending, done),
                        ));
                    }
                    None => {
                        done = true;
                        return Some((
                            Ok(ArcAiStreamEvent::Done),
                            (byte_stream, buf, blocks, pending, done),
                        ));
                    }
                }
            }
        },
    );

    Box::pin(s)
}

// ─────────────────────────────────────────────────────────────────────────────
// OpenAI SSE parser
// ─────────────────────────────────────────────────────────────────────────────

/// Accumulated state for a single tool call being assembled from streaming
/// delta chunks.
struct OpenAiToolAccum {
    id: String,
    name: String,
    args_buf: String,
}

/// Parse an OpenAI Chat Completions SSE byte stream into `ArcAiStreamEvent`s.
///
/// Generic over the error type so unit tests can use `std::convert::Infallible`
/// instead of having to construct `reqwest::Error` values.
///
/// Protocol:
/// - `data: [DONE]` → flush any accumulated tool calls (by index order), then
///   emit `Done`.
/// - `data: {...}` → decode `choices[0].delta`; emit `Text(...)` for content
///   deltas; accumulate tool-call deltas by `index`.
/// - Empty lines, comment lines, non-`data:` lines → skip.
pub fn parse_openai_sse<S, E>(byte_stream: S) -> ArcAiStream
where
    S: Stream<Item = Result<bytes::Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    type State<S> = (
        Pin<Box<S>>,
        Vec<u8>,
        HashMap<usize, OpenAiToolAccum>,
        Vec<Result<ArcAiStreamEvent, String>>,
        bool,
    );

    let initial: State<S> = (
        Box::pin(byte_stream),
        Vec::new(),
        HashMap::new(),
        Vec::new(),
        false,
    );

    let s = futures_util::stream::unfold(
        initial,
        |(mut byte_stream, mut buf, mut tools, mut pending, mut done)| async move {
            // Drain any queued events first (tool flushes on DONE).
            if let Some(ev) = pending.pop() {
                return Some((ev, (byte_stream, buf, tools, pending, done)));
            }
            if done {
                return None;
            }

            loop {
                if let Some(nl) = buf.iter().position(|&b| b == b'\n') {
                    let raw = &buf[..nl];
                    let raw = if raw.last() == Some(&b'\r') {
                        &raw[..raw.len() - 1]
                    } else {
                        raw
                    };
                    let line = String::from_utf8_lossy(raw).into_owned();
                    buf.drain(..=nl);

                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    let data = match line.strip_prefix("data: ") {
                        Some(d) => d.trim().to_string(),
                        None => continue,
                    };

                    // Terminal sentinel.
                    if data == "[DONE]" {
                        done = true;
                        // Flush accumulated tool calls in index order.
                        let mut indices: Vec<usize> = tools.keys().copied().collect();
                        indices.sort_unstable();
                        // Push in reverse so pending.pop() gives us index order.
                        pending.push(Ok(ArcAiStreamEvent::Done));
                        for idx in indices.into_iter().rev() {
                            if let Some(acc) = tools.remove(&idx) {
                                let arguments: Value = if acc.args_buf.is_empty() {
                                    json!({})
                                } else {
                                    serde_json::from_str(&acc.args_buf).unwrap_or(json!({}))
                                };
                                pending.push(Ok(ArcAiStreamEvent::ToolUse {
                                    id: acc.id,
                                    name: acc.name,
                                    arguments,
                                }));
                            }
                        }
                        // Return first item from the back (lowest index tool or Done).
                        if let Some(ev) = pending.pop() {
                            return Some((ev, (byte_stream, buf, tools, pending, done)));
                        }
                        return None;
                    }

                    let chunk: OpenAiStreamChunk = match serde_json::from_str(&data) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };

                    let delta = match chunk.choices.into_iter().next() {
                        Some(c) => c.delta,
                        None => continue,
                    };

                    // Text delta.
                    if let Some(text) = delta.content {
                        if !text.is_empty() {
                            return Some((
                                Ok(ArcAiStreamEvent::Text(text)),
                                (byte_stream, buf, tools, pending, done),
                            ));
                        }
                    }

                    // Tool-call deltas — accumulate by index.
                    if let Some(tcs) = delta.tool_calls {
                        for tc in tcs {
                            let acc = tools.entry(tc.index).or_insert_with(|| OpenAiToolAccum {
                                id: String::new(),
                                name: String::new(),
                                args_buf: String::new(),
                            });
                            if let Some(id) = tc.id {
                                acc.id = id;
                            }
                            if let Some(func) = tc.function {
                                if let Some(name) = func.name {
                                    acc.name = name;
                                }
                                if let Some(args) = func.arguments {
                                    acc.args_buf.push_str(&args);
                                }
                            }
                        }
                    }

                    continue;
                }

                match byte_stream.next().await {
                    Some(Ok(chunk)) => {
                        buf.extend_from_slice(&chunk);
                    }
                    Some(Err(e)) => {
                        done = true;
                        return Some((
                            Err(format!("upstream stream error: {e}")),
                            (byte_stream, buf, tools, pending, done),
                        ));
                    }
                    None => {
                        // Stream ended without [DONE] — emit Done anyway.
                        done = true;
                        return Some((
                            Ok(ArcAiStreamEvent::Done),
                            (byte_stream, buf, tools, pending, done),
                        ));
                    }
                }
            }
        },
    );

    Box::pin(s)
}

// ─────────────────────────────────────────────────────────────────────────────
// ArcAiBackend impl — provider dispatch
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl ArcAiBackend for HttpAiBackend {
    async fn chat(&self, req: &ArcAiChatRequest) -> Result<ArcAiChatResponse, String> {
        let (provider, bare) = parse_provider(&req.model);
        match provider {
            "anthropic" => self.chat_anthropic(req, bare).await,
            "openai" => self.chat_openai(req, bare).await,
            other => Err(format!(
                "provider '{other}' is not implemented on this relay"
            )),
        }
    }

    async fn stream_chat(&self, req: &ArcAiChatRequest) -> Result<ArcAiStream, String> {
        let (provider, bare) = parse_provider(&req.model);
        match provider {
            "anthropic" => self.stream_chat_anthropic(req, bare).await,
            "openai" => self.stream_chat_openai(req, bare).await,
            other => Err(format!(
                "provider '{other}' is not implemented on this relay"
            )),
        }
    }

    async fn list_models(&self) -> Vec<ArcAiModel> {
        let mut out = Vec::new();

        if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            out.push(ArcAiModel {
                id: "anthropic:claude-opus-4-5".to_string(),
                name: Some("Claude Opus 4.5".to_string()),
                context_window: Some(200_000),
            });
            out.push(ArcAiModel {
                id: "anthropic:claude-sonnet-4-5".to_string(),
                name: Some("Claude Sonnet 4.5".to_string()),
                context_window: Some(200_000),
            });
            out.push(ArcAiModel {
                id: "anthropic:claude-haiku-4-5".to_string(),
                name: Some("Claude Haiku 4.5".to_string()),
                context_window: Some(200_000),
            });
        }

        if std::env::var("OPENAI_API_KEY").is_ok() {
            out.push(ArcAiModel {
                id: "openai:gpt-4o-2024-08-06".to_string(),
                name: Some("GPT-4o".to_string()),
                context_window: Some(128_000),
            });
            out.push(ArcAiModel {
                id: "openai:gpt-4o-mini-2024-07-18".to_string(),
                name: Some("GPT-4o mini".to_string()),
                context_window: Some(128_000),
            });
        }

        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Private provider methods
// ─────────────────────────────────────────────────────────────────────────────

impl HttpAiBackend {
    async fn chat_anthropic(
        &self,
        req: &ArcAiChatRequest,
        bare_model: &str,
    ) -> Result<ArcAiChatResponse, String> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| "ANTHROPIC_API_KEY is not configured on this relay".to_string())?;

        let body = build_anthropic_request(bare_model, req, false);

        let resp = self
            .client
            .post(self.anthropic_messages_url())
            .header("x-api-key", &api_key)
            .header("anthropic-version", ANTHROPIC_VERSION_HEADER)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("upstream request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(format!("upstream returned {status}: {body_text}"));
        }

        let parsed: AnthropicResponse = resp
            .json()
            .await
            .map_err(|e| format!("upstream response parse error: {e}"))?;

        let mut content: Option<String> = None;
        let mut tool_calls: Vec<ArcAiToolCall> = Vec::new();

        for block in parsed.content {
            match block {
                AnthropicResponseBlock::Text { text } => {
                    content = Some(match content.take() {
                        Some(prev) => prev + &text,
                        None => text,
                    });
                }
                AnthropicResponseBlock::ToolUse { id, name, input } => {
                    let arguments =
                        serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
                    tool_calls.push(ArcAiToolCall {
                        id,
                        kind: "function".into(),
                        function: ArcAiToolCallFunction { name, arguments },
                    });
                }
            }
        }

        // Match legacy /v1/ai/chat: when only tool_use blocks came back,
        // null out empty content (OpenAI convention).
        if !tool_calls.is_empty() && content.as_deref().map(str::is_empty).unwrap_or(true) {
            content = None;
        }

        Ok(ArcAiChatResponse {
            content,
            tool_calls,
        })
    }

    async fn stream_chat_anthropic(
        &self,
        req: &ArcAiChatRequest,
        bare_model: &str,
    ) -> Result<ArcAiStream, String> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| "ANTHROPIC_API_KEY is not configured on this relay".to_string())?;

        let body = build_anthropic_request(bare_model, req, true);

        let resp = self
            .client
            .post(self.anthropic_messages_url())
            .header("x-api-key", &api_key)
            .header("anthropic-version", ANTHROPIC_VERSION_HEADER)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("upstream request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(format!("upstream returned {status}: {body_text}"));
        }

        Ok(parse_anthropic_sse(resp.bytes_stream()))
    }

    async fn chat_openai(
        &self,
        req: &ArcAiChatRequest,
        bare_model: &str,
    ) -> Result<ArcAiChatResponse, String> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| "OPENAI_API_KEY is not configured on this relay".to_string())?;

        let body = build_openai_request(bare_model, req, false);

        let resp = self
            .client
            .post(self.openai_chat_completions_url())
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("upstream request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(format!("upstream returned {status}: {body_text}"));
        }

        let parsed: OpenAiResponse = resp
            .json()
            .await
            .map_err(|e| format!("upstream response parse error: {e}"))?;

        let msg = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message)
            .unwrap_or(OpenAiResponseMessage {
                content: None,
                tool_calls: None,
            });

        // Normalize empty string content to None.
        let content = msg.content.filter(|s| !s.is_empty());
        let tool_calls = msg.tool_calls.unwrap_or_default();

        Ok(ArcAiChatResponse {
            content,
            tool_calls,
        })
    }

    async fn stream_chat_openai(
        &self,
        req: &ArcAiChatRequest,
        bare_model: &str,
    ) -> Result<ArcAiStream, String> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| "OPENAI_API_KEY is not configured on this relay".to_string())?;

        let body = build_openai_request(bare_model, req, true);

        let resp = self
            .client
            .post(self.openai_chat_completions_url())
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("upstream request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(format!("upstream returned {status}: {body_text}"));
        }

        Ok(parse_openai_sse(resp.bytes_stream()))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_rpc::{ArcAiBackend, ArcAiMessage};

    // Serialise env-var mutations across all tests.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    // ── Existing Anthropic tests (UNCHANGED) ──────────────────────────────────

    #[tokio::test]
    async fn list_models_returns_anthropic_when_key_set() {
        let _guard = ENV_LOCK.lock().await;
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");
        std::env::remove_var("OPENAI_API_KEY");
        let backend = HttpAiBackend::new();
        let models = backend.list_models().await;
        std::env::remove_var("ANTHROPIC_API_KEY");
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"anthropic:claude-opus-4-5"));
        assert!(ids.contains(&"anthropic:claude-sonnet-4-5"));
        assert!(ids.contains(&"anthropic:claude-haiku-4-5"));
        assert_eq!(models.len(), 3);
    }

    #[tokio::test]
    async fn list_models_empty_when_key_unset() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
        let backend = HttpAiBackend::new();
        let models = backend.list_models().await;
        assert!(models.is_empty());
    }

    #[tokio::test]
    async fn chat_without_api_key_returns_not_configured_error() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
        let backend = HttpAiBackend::new();
        let req = ArcAiChatRequest {
            model: "anthropic:claude-haiku-4-5".to_string(),
            messages: vec![ArcAiMessage {
                role: "user".to_string(),
                content: Some("hello".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let result = backend.chat(&req).await;
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("not configured"),
            "expected 'not configured' in: {msg}"
        );
    }

    #[tokio::test]
    async fn chat_unknown_provider_returns_not_implemented() {
        let _guard = ENV_LOCK.lock().await;
        let backend = HttpAiBackend::new();
        let req = ArcAiChatRequest {
            model: "foobar:gpt-4o".to_string(),
            messages: vec![ArcAiMessage {
                role: "user".to_string(),
                content: Some("hello".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let result = backend.chat(&req).await;
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("foobar") && msg.contains("not implemented"),
            "expected provider name + 'not implemented' in: {msg}"
        );
    }

    #[test]
    fn build_anthropic_request_explicit_system_overrides_messages() {
        let req = ArcAiChatRequest {
            model: "anthropic:foo".into(),
            system: Some("explicit".into()),
            messages: vec![
                ArcAiMessage {
                    role: "system".into(),
                    content: Some("ignored".into()),
                    ..Default::default()
                },
                ArcAiMessage {
                    role: "user".into(),
                    content: Some("hi".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let body = build_anthropic_request("foo", &req, false);
        assert_eq!(body.system.as_deref(), Some("explicit"));
        // System message is filtered out of the messages array
        assert_eq!(body.messages.len(), 1);
    }

    #[test]
    fn build_anthropic_request_falls_back_to_message_system() {
        let req = ArcAiChatRequest {
            model: "anthropic:foo".into(),
            messages: vec![
                ArcAiMessage {
                    role: "system".into(),
                    content: Some("from-msg".into()),
                    ..Default::default()
                },
                ArcAiMessage {
                    role: "user".into(),
                    content: Some("hi".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let body = build_anthropic_request("foo", &req, false);
        assert_eq!(body.system.as_deref(), Some("from-msg"));
    }

    #[test]
    fn build_anthropic_request_stream_flag_respected() {
        let req = ArcAiChatRequest::default();
        assert_eq!(build_anthropic_request("foo", &req, false).stream, None);
        assert_eq!(
            build_anthropic_request("foo", &req, true).stream,
            Some(true)
        );
    }

    #[test]
    fn build_anthropic_request_honors_temperature_max_tokens() {
        let req = ArcAiChatRequest {
            model: "anthropic:foo".into(),
            temperature: Some(0.7),
            max_tokens: Some(2048),
            ..Default::default()
        };
        let body = build_anthropic_request("foo", &req, false);
        assert_eq!(body.temperature, Some(0.7));
        assert_eq!(body.max_tokens, 2048);
    }

    #[test]
    fn build_anthropic_request_default_max_tokens() {
        let body = build_anthropic_request("foo", &ArcAiChatRequest::default(), false);
        assert_eq!(body.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(body.temperature, None);
    }

    #[test]
    fn build_anthropic_request_forwards_tools() {
        use crate::ai_rpc::{ArcAiTool, ArcAiToolFunction};
        let req = ArcAiChatRequest {
            model: "anthropic:foo".into(),
            tools: vec![ArcAiTool {
                kind: "function".into(),
                function: ArcAiToolFunction {
                    name: "ls".into(),
                    description: "list".into(),
                    parameters: json!({"type": "object"}),
                },
            }],
            ..Default::default()
        };
        let body = build_anthropic_request("foo", &req, false);
        assert_eq!(body.tools.len(), 1);
        assert_eq!(body.tools[0].name, "ls");
        assert_eq!(body.tools[0].input_schema, json!({"type": "object"}));
    }

    #[test]
    fn build_anthropic_messages_maps_tool_role_to_user_tool_result() {
        use crate::ai_rpc::ArcAiMessage;
        let msgs = vec![
            ArcAiMessage {
                role: "user".into(),
                content: Some("call ls".into()),
                ..Default::default()
            },
            ArcAiMessage {
                role: "tool".into(),
                content: Some("file1\nfile2".into()),
                tool_call_id: Some("call_42".into()),
                ..Default::default()
            },
        ];
        let out = build_anthropic_messages(&msgs);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, "user");
        assert_eq!(out[1].role, "user");
        let blob = serde_json::to_value(&out[1]).unwrap();
        assert_eq!(blob["content"][0]["type"], "tool_result");
        assert_eq!(blob["content"][0]["tool_use_id"], "call_42");
        assert_eq!(blob["content"][0]["content"], "file1\nfile2");
    }

    #[test]
    fn build_anthropic_messages_preserves_assistant_tool_calls() {
        use crate::ai_rpc::{ArcAiMessage, ArcAiToolCall, ArcAiToolCallFunction};
        let msgs = vec![ArcAiMessage {
            role: "assistant".into(),
            content: Some("let me check".into()),
            tool_calls: vec![ArcAiToolCall {
                id: "call_42".into(),
                kind: "function".into(),
                function: ArcAiToolCallFunction {
                    name: "ls".into(),
                    arguments: "{\"path\":\".\"}".into(),
                },
            }],
            ..Default::default()
        }];
        let out = build_anthropic_messages(&msgs);
        assert_eq!(out.len(), 1);
        let blob = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(blob["role"], "assistant");
        let blocks = blob["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "let me check");
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["id"], "call_42");
        assert_eq!(blocks[1]["name"], "ls");
        assert_eq!(blocks[1]["input"], json!({"path": "."}));
    }

    // ── New OpenAI tests ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_models_includes_openai_when_openai_key_set() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::set_var("OPENAI_API_KEY", "test-openai-key");
        let backend = HttpAiBackend::new();
        let models = backend.list_models().await;
        std::env::remove_var("OPENAI_API_KEY");
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert!(
            ids.contains(&"openai:gpt-4o-2024-08-06"),
            "expected openai:gpt-4o-2024-08-06 in {ids:?}"
        );
        assert!(
            ids.contains(&"openai:gpt-4o-mini-2024-07-18"),
            "expected openai:gpt-4o-mini-2024-07-18 in {ids:?}"
        );
        assert_eq!(models.len(), 2);
    }

    #[tokio::test]
    async fn list_models_omits_openai_when_openai_key_unset() {
        let _guard = ENV_LOCK.lock().await;
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");
        std::env::remove_var("OPENAI_API_KEY");
        let backend = HttpAiBackend::new();
        let models = backend.list_models().await;
        std::env::remove_var("ANTHROPIC_API_KEY");
        assert!(
            models.iter().all(|m| !m.id.starts_with("openai:")),
            "expected no openai entries"
        );
    }

    #[tokio::test]
    async fn chat_openai_without_api_key_returns_not_configured() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("OPENAI_API_KEY");
        let backend = HttpAiBackend::new();
        let req = ArcAiChatRequest {
            model: "openai:gpt-4o-2024-08-06".to_string(),
            messages: vec![ArcAiMessage {
                role: "user".to_string(),
                content: Some("hello".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let result = backend.chat(&req).await;
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("not configured"),
            "expected 'not configured' in: {msg}"
        );
    }

    #[test]
    fn build_openai_request_prepends_explicit_system() {
        let req = ArcAiChatRequest {
            model: "openai:gpt-4o-2024-08-06".into(),
            system: Some("hi".into()),
            messages: vec![ArcAiMessage {
                role: "user".into(),
                content: Some("hello".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let body = build_openai_request("gpt-4o-2024-08-06", &req, false);
        assert_eq!(
            body.messages.len(),
            2,
            "should have prepended system message"
        );
        assert_eq!(body.messages[0].role, "system");
        assert_eq!(body.messages[0].content.as_deref(), Some("hi"));
        assert_eq!(body.messages[1].role, "user");
    }

    #[test]
    fn build_openai_request_preserves_existing_system_message() {
        let req = ArcAiChatRequest {
            model: "openai:gpt-4o-2024-08-06".into(),
            system: None,
            messages: vec![
                ArcAiMessage {
                    role: "system".into(),
                    content: Some("you are helpful".into()),
                    ..Default::default()
                },
                ArcAiMessage {
                    role: "user".into(),
                    content: Some("hello".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let body = build_openai_request("gpt-4o-2024-08-06", &req, false);
        // Messages unchanged: system still at index 0, no extra prepend.
        assert_eq!(body.messages.len(), 2);
        assert_eq!(body.messages[0].role, "system");
        assert_eq!(body.messages[0].content.as_deref(), Some("you are helpful"));
    }

    #[test]
    fn build_openai_request_passes_through_tools() {
        use crate::ai_rpc::{ArcAiTool, ArcAiToolFunction};
        let req = ArcAiChatRequest {
            model: "openai:gpt-4o-2024-08-06".into(),
            tools: vec![ArcAiTool {
                kind: "function".into(),
                function: ArcAiToolFunction {
                    name: "search".into(),
                    description: "search the web".into(),
                    parameters: json!({"type": "object", "properties": {}}),
                },
            }],
            ..Default::default()
        };
        let body = build_openai_request("gpt-4o-2024-08-06", &req, false);
        assert_eq!(body.tools.len(), 1);
        assert_eq!(body.tools[0].function.name, "search");
    }

    #[tokio::test]
    async fn parse_openai_sse_text_chunks_then_done() {
        use bytes::Bytes;
        use futures_util::stream;

        let chunks: Vec<Result<Bytes, std::convert::Infallible>> = vec![
            Ok(Bytes::from(
                "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"choices\":[{\"delta\":{\"content\":\" there\"}}]}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ];

        let s = parse_openai_sse(stream::iter(chunks));
        let events: Vec<_> = s.collect().await;

        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], Ok(ArcAiStreamEvent::Text(t)) if t == "hi"));
        assert!(matches!(&events[1], Ok(ArcAiStreamEvent::Text(t)) if t == " there"));
        assert!(matches!(&events[2], Ok(ArcAiStreamEvent::Done)));
    }

    #[tokio::test]
    async fn parse_openai_sse_tool_call_assembly() {
        use bytes::Bytes;
        use futures_util::stream;

        // Build valid SSE data lines using serde_json to avoid manual escaping bugs.
        //
        // Chunk 1: id + function name arrive on the first delta (empty arguments).
        // Chunk 2: arguments fragment #1 – `{"path":`.
        // Chunk 3: arguments fragment #2 – `"."}`.
        // Then [DONE].
        //
        // serde_json::json! produces valid JSON; format! embeds it into the
        // SSE `data:` line so we never hand-escape nested quotes.
        let chunk1 = {
            let j = json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_1",
                            "function": { "name": "ls", "arguments": "" }
                        }]
                    }
                }]
            });
            format!("data: {j}\n\n")
        };
        let chunk2 = {
            let j = json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "function": { "arguments": "{\"path\":" }
                        }]
                    }
                }]
            });
            format!("data: {j}\n\n")
        };
        let chunk3 = {
            let j = json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "function": { "arguments": "\".\"}"}
                        }]
                    }
                }]
            });
            format!("data: {j}\n\n")
        };

        let chunks: Vec<Result<Bytes, std::convert::Infallible>> = vec![
            Ok(Bytes::from(chunk1)),
            Ok(Bytes::from(chunk2)),
            Ok(Bytes::from(chunk3)),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ];

        let s = parse_openai_sse(stream::iter(chunks));
        let events: Vec<_> = s.collect().await;

        // Expect: ToolUse then Done.
        assert_eq!(events.len(), 2, "events: {events:?}");
        match &events[0] {
            Ok(ArcAiStreamEvent::ToolUse {
                id,
                name,
                arguments,
            }) => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "ls");
                // Arguments should be {"path": "."}
                assert_eq!(arguments["path"], json!("."));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
        assert!(matches!(&events[1], Ok(ArcAiStreamEvent::Done)));
    }
}
