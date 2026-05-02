//! Production [`ArcAiBackend`] adapter that fans out to the Anthropic
//! Messages API.
//!
//! Mirrors the upstream pattern used by `arc-relay/src/api/ai_routes.rs`
//! but returns domain-typed [`ArcAiStreamEvent`] values instead of
//! axum-tied SSE response streams. Configurable base URL so integration
//! tests can target a mock Anthropic server.
//!
//! Reads `ANTHROPIC_API_KEY` from the process environment at each call.
//! When unset, [`HttpAiBackend::list_models`] returns an empty list and
//! [`HttpAiBackend::chat`] / [`HttpAiBackend::stream_chat`] return an
//! `Err` with a `not configured` message.
//!
//! Provider prefix convention: model id `"anthropic:<bare>"` → forward
//! `<bare>` to the upstream API. No prefix → assume Anthropic.
//! Other prefixes → `Err`.

use std::pin::Pin;

use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::ai_rpc::{
    ArcAiBackend, ArcAiChatRequest, ArcAiChatResponse, ArcAiMessage, ArcAiModel, ArcAiStream,
    ArcAiStreamEvent, ArcAiToolCall, ArcAiToolCallFunction,
};

const ANTHROPIC_VERSION_HEADER: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 1024;
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Production AI backend. Calls the Anthropic Messages API for both
/// streaming and non-streaming requests.
pub struct HttpAiBackend {
    client: reqwest::Client,
    base_url: String,
}

impl HttpAiBackend {
    /// Production constructor. Uses `https://api.anthropic.com` as the
    /// upstream and a fresh default `reqwest::Client`.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    /// Test constructor. `base_url` MUST NOT include a trailing slash.
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Self::new()
        }
    }

    fn anthropic_messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url)
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
    ToolUse { id: String, name: String, input: Value },
    #[serde(rename = "tool_result")]
    ToolResult { tool_use_id: String, content: String },
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
    ContentBlockDelta {
        index: usize,
        delta: AnthropicDelta,
    },
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

// ─────────────────────────────────────────────────────────────────────────────
// SSE parser
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
    let initial: SseUnfoldState =
        (Box::pin(byte_stream), Vec::new(), Default::default(), Vec::new(), false);

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
                    let raw = if raw.last() == Some(&b'\r') { &raw[..raw.len() - 1] } else { raw };
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
                        AnthropicEvent::ContentBlockStart { index, content_block } => {
                            match content_block {
                                AnthropicBlockStart::Text => {
                                    blocks.insert(index, BlockAccum {
                                        is_tool: false,
                                        id: String::new(),
                                        name: String::new(),
                                        args_buf: String::new(),
                                    });
                                }
                                AnthropicBlockStart::ToolUse { id, name } => {
                                    blocks.insert(index, BlockAccum {
                                        is_tool: true,
                                        id,
                                        name,
                                        args_buf: String::new(),
                                    });
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
                                        serde_json::from_str(&b.args_buf)
                                            .unwrap_or(json!({}))
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
// ArcAiBackend impl
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl ArcAiBackend for HttpAiBackend {
    async fn chat(&self, req: &ArcAiChatRequest) -> Result<ArcAiChatResponse, String> {
        let (provider, bare_model) = parse_provider(&req.model);
        if provider != "anthropic" {
            return Err(format!("provider '{provider}' is not implemented on this relay"));
        }

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
                    let arguments = serde_json::to_string(&input)
                        .unwrap_or_else(|_| "{}".to_string());
                    tool_calls.push(ArcAiToolCall {
                        id,
                        kind: "function".into(),
                        function: ArcAiToolCallFunction {
                            name,
                            arguments,
                        },
                    });
                }
            }
        }

        // Match legacy /v1/ai/chat: when only tool_use blocks came back,
        // null out empty content (OpenAI convention).
        if !tool_calls.is_empty() && content.as_deref().map(str::is_empty).unwrap_or(true) {
            content = None;
        }

        Ok(ArcAiChatResponse { content, tool_calls })
    }

    async fn stream_chat(&self, req: &ArcAiChatRequest) -> Result<ArcAiStream, String> {
        let (provider, bare_model) = parse_provider(&req.model);
        if provider != "anthropic" {
            return Err(format!("provider '{provider}' is not implemented on this relay"));
        }

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
        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_rpc::{ArcAiBackend, ArcAiMessage};

    // Serialise env-var mutations across both tests.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn list_models_returns_anthropic_when_key_set() {
        let _guard = ENV_LOCK.lock().await;
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");
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
        let backend = HttpAiBackend::new();
        let models = backend.list_models().await;
        assert!(models.is_empty());
    }

    #[tokio::test]
    async fn chat_without_api_key_returns_not_configured_error() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("ANTHROPIC_API_KEY");
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
            model: "openai:gpt-4o".to_string(),
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
            msg.contains("openai") && msg.contains("not implemented"),
            "expected provider name + 'not implemented' in: {msg}"
        );
    }

    #[test]
    fn build_anthropic_request_explicit_system_overrides_messages() {
        let req = ArcAiChatRequest {
            model: "anthropic:foo".into(),
            system: Some("explicit".into()),
            messages: vec![
                ArcAiMessage { role: "system".into(), content: Some("ignored".into()), ..Default::default() },
                ArcAiMessage { role: "user".into(), content: Some("hi".into()), ..Default::default() },
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
                ArcAiMessage { role: "system".into(), content: Some("from-msg".into()), ..Default::default() },
                ArcAiMessage { role: "user".into(), content: Some("hi".into()), ..Default::default() },
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
        assert_eq!(build_anthropic_request("foo", &req, true).stream, Some(true));
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
            ArcAiMessage { role: "user".into(), content: Some("call ls".into()), ..Default::default() },
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
}
