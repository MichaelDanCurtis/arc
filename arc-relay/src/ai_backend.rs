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
    ArcAiBackend, ArcAiChatRequest, ArcAiModel, ArcAiStream, ArcAiStreamEvent,
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
}

#[derive(Debug, Serialize, Clone)]
struct AnthropicMessage {
    role: String,
    content: String,
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
        #[allow(dead_code)]
        id: String,
        #[allow(dead_code)]
        name: String,
        #[allow(dead_code)]
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
/// Extracts the first system message from `req.messages` (if any) into the
/// top-level `system` field; remaining non-system messages are mapped into
/// `messages`.
fn build_anthropic_request(
    bare_model: &str,
    req: &ArcAiChatRequest,
    stream: bool,
) -> AnthropicRequest {
    let mut system: Option<String> = None;
    let mut messages: Vec<AnthropicMessage> = Vec::new();

    for m in &req.messages {
        if m.role == "system" {
            if system.is_none() {
                system = m.content.clone();
            }
            continue;
        }
        messages.push(AnthropicMessage {
            role: m.role.clone(),
            content: m.content.clone().unwrap_or_default(),
        });
    }

    AnthropicRequest {
        model: bare_model.to_string(),
        max_tokens: DEFAULT_MAX_TOKENS,
        messages,
        system,
        stream: stream.then_some(true),
    }
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
    async fn chat(&self, req: &ArcAiChatRequest) -> Result<String, String> {
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

        let mut out = String::new();
        for block in parsed.content {
            if let AnthropicResponseBlock::Text { text } = block {
                out.push_str(&text);
            }
        }
        Ok(out)
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
            }],
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
            }],
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
    fn build_anthropic_request_extracts_first_system_message() {
        let req = ArcAiChatRequest {
            model: "anthropic:claude-haiku-4-5".to_string(),
            messages: vec![
                ArcAiMessage {
                    role: "system".to_string(),
                    content: Some("You are a helpful assistant.".to_string()),
                },
                ArcAiMessage {
                    role: "user".to_string(),
                    content: Some("What is 2+2?".to_string()),
                },
            ],
        };
        let built = build_anthropic_request("claude-haiku-4-5", &req, false);
        assert_eq!(
            built.system.as_deref(),
            Some("You are a helpful assistant.")
        );
        assert_eq!(built.messages.len(), 1);
        assert_eq!(built.messages[0].role, "user");
        assert_eq!(built.messages[0].content, "What is 2+2?");
        assert!(built.stream.is_none());
    }

    #[test]
    fn build_anthropic_request_stream_flag() {
        let req = ArcAiChatRequest {
            model: "anthropic:claude-haiku-4-5".to_string(),
            messages: vec![ArcAiMessage {
                role: "user".to_string(),
                content: Some("ping".to_string()),
            }],
        };
        let built_no_stream = build_anthropic_request("claude-haiku-4-5", &req, false);
        assert!(built_no_stream.stream.is_none());

        let built_stream = build_anthropic_request("claude-haiku-4-5", &req, true);
        assert_eq!(built_stream.stream, Some(true));
    }
}
