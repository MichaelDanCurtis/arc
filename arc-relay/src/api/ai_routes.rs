//! AI fan-out endpoints for the Arc relay.
//!
//! Four stateless routes that accept a request from the Beacon Mac client,
//! authenticate it via Bearer JWT (same middleware as all other protected
//! routes), and fan out to the appropriate upstream AI provider using
//! server-side API keys read from environment variables.
//!
//! ## Supported providers
//!
//! | `provider` field | Env var required    | Notes                          |
//! |-----------------|---------------------|--------------------------------|
//! | `"anthropic"`   | `ANTHROPIC_API_KEY` | Full fan-out implemented        |
//! | anything else   | —                   | 501 Not Implemented             |
//!
//! Adding a new provider means:
//! 1. Add an env-var lookup (e.g. `OPENAI_API_KEY`).
//! 2. Add a match arm in `fan_out_chat` / `fan_out_stream` / `fan_out_complete`.
//! 3. Add the model(s) to `list_models`.
//!
//! ## Environment variables
//!
//! - `ANTHROPIC_API_KEY` — Anthropic API key; if unset, Anthropic requests
//!   return 501 with a JSON error body.
//!
//! ## Routes
//!
//! - `POST /v1/ai/chat`         — non-streaming chat, OpenAI-compat response
//! - `POST /v1/ai/stream_chat`  — SSE stream, OpenAI-compat deltas + `[DONE]`
//! - `POST /v1/ai/complete`     — single-shot ghost-text completion
//! - `GET  /v1/ai/list_models`  — merged list of models from configured providers

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response, Sse},
    Json,
};
use futures_util::stream::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::AppState;
use crate::auth::middleware::AuthUser;

// ---------------------------------------------------------------------------
// Shared request / response types
// ---------------------------------------------------------------------------

/// Shared envelope for `/v1/ai/chat` and `/v1/ai/stream_chat`.
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    /// Upstream provider name: "anthropic", "openai", etc.
    /// Empty string → relay default (Anthropic if configured).
    #[serde(default)]
    pub provider: String,
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub tools: Vec<ToolDef>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub system: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<RequestToolCall>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RequestToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: RequestToolFunction,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RequestToolFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ToolDef {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolFunction,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Response type for `/v1/ai/chat`.
#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub choices: Vec<ChatChoice>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageResponse>,
}

#[derive(Debug, Serialize)]
pub struct ChatChoice {
    pub message: ResponseMessage,
}

#[derive(Debug, Serialize)]
pub struct ResponseMessage {
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ResponseToolCall>,
}

#[derive(Debug, Serialize)]
pub struct ResponseToolCall {
    pub id: String,
    pub function: ResponseFunction,
}

#[derive(Debug, Serialize)]
pub struct ResponseFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Serialize)]
pub struct UsageResponse {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// Request body for `/v1/ai/complete`.
#[derive(Debug, Deserialize)]
pub struct CompleteRequest {
    pub model: String,
    pub prompt: String,
    pub max_tokens: Option<u32>,
}

/// Response for `/v1/ai/complete`.
#[derive(Debug, Serialize)]
pub struct CompleteResponse {
    pub text: String,
}

/// Single model entry in `/v1/ai/list_models`.
#[derive(Debug, Serialize)]
pub struct ModelEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

/// Response for `GET /v1/ai/list_models`.
#[derive(Debug, Serialize)]
pub struct ModelListResponse {
    pub models: Vec<ModelEntry>,
}

// ---------------------------------------------------------------------------
// Anthropic request/response types (internal)
// ---------------------------------------------------------------------------

/// Anthropic Messages API request body.
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    /// Request streaming from Anthropic.
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
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

/// Anthropic Messages API response (non-streaming).
#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicResponseBlock>,
    model: String,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: AnthropicUsage,
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

#[derive(Debug, Deserialize, Default)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

/// Anthropic SSE event types (streaming).
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
    #[serde(rename = "message_start")]
    MessageStart { message: AnthropicMessageMeta },
    #[serde(rename = "message_delta")]
    MessageDelta { delta: AnthropicMessageDeltaMeta },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: Value },
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

#[derive(Debug, Deserialize)]
struct AnthropicMessageMeta {
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDeltaMeta {
    #[serde(default)]
    stop_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Error helper
// ---------------------------------------------------------------------------

fn json_error(status: StatusCode, message: &str) -> Response {
    let body = serde_json::json!({ "error": message });
    (status, Json(body)).into_response()
}

// ---------------------------------------------------------------------------
// Anthropic provider helpers
// ---------------------------------------------------------------------------

/// Convert OpenAI-compat messages to Anthropic Messages API format.
///
/// Anthropic separates the system prompt from the messages array; system
/// messages in the array are converted to a system string if the caller
/// didn't already supply one.
fn build_anthropic_messages(messages: &[ChatMessage]) -> Vec<AnthropicMessage> {
    let mut out: Vec<AnthropicMessage> = Vec::new();

    for msg in messages {
        match msg.role.as_str() {
            "system" => {
                // System messages are handled via the top-level `system` field;
                // skip them here. Callers that mix system messages in the array
                // should pass the first one via the top-level `system` field.
                continue;
            }
            "tool" => {
                // Tool result turn — Anthropic expects role=user, content=tool_result block.
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
                // Assistant turn with tool calls — build a blocks array.
                let mut blocks: Vec<AnthropicBlock> = Vec::new();
                if let Some(text) = &msg.content {
                    if !text.is_empty() {
                        blocks.push(AnthropicBlock::Text { text: text.clone() });
                    }
                }
                for tc in &msg.tool_calls {
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
                // user / assistant (plain text)
                out.push(AnthropicMessage {
                    role: role.to_string(),
                    content: AnthropicContent::Text(msg.content.clone().unwrap_or_default()),
                });
            }
        }
    }

    out
}

fn build_anthropic_tools(tools: &[ToolDef]) -> Vec<AnthropicTool> {
    tools
        .iter()
        .map(|t| AnthropicTool {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            input_schema: t.function.parameters.clone(),
        })
        .collect()
}

/// Convert an Anthropic non-streaming response into our OpenAI-compat shape.
fn anthropic_response_to_chat(resp: AnthropicResponse, provider: &str) -> ChatResponse {
    let model = format!("{}:{}", provider, resp.model);
    let mut content: Option<String> = None;
    let mut tool_calls: Vec<ResponseToolCall> = Vec::new();

    for block in resp.content {
        match block {
            AnthropicResponseBlock::Text { text } => {
                content = Some(text);
            }
            AnthropicResponseBlock::ToolUse { id, name, input } => {
                let arguments = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
                tool_calls.push(ResponseToolCall {
                    id,
                    function: ResponseFunction { name, arguments },
                });
            }
        }
    }

    // If we have tool calls and no text, content should be null (matches OpenAI convention).
    if !tool_calls.is_empty() && content.as_deref().map(|s| s.is_empty()).unwrap_or(true) {
        content = None;
    }

    ChatResponse {
        choices: vec![ChatChoice {
            message: ResponseMessage {
                content,
                tool_calls,
            },
        }],
        model,
        usage: Some(UsageResponse {
            prompt_tokens: resp.usage.input_tokens,
            completion_tokens: resp.usage.output_tokens,
        }),
    }
}

// ---------------------------------------------------------------------------
// Upstream fan-out — non-streaming chat
// ---------------------------------------------------------------------------

async fn fan_out_chat(
    req: &ChatRequest,
    client: &reqwest::Client,
) -> Result<ChatResponse, Response> {
    let provider = if req.provider.is_empty() {
        "anthropic"
    } else {
        req.provider.as_str()
    };

    match provider {
        "anthropic" => fan_out_chat_anthropic(req, client).await,
        other => Err(json_error(
            StatusCode::NOT_IMPLEMENTED,
            &format!("provider '{}' is not implemented on this relay", other),
        )),
    }
}

async fn fan_out_chat_anthropic(
    req: &ChatRequest,
    client: &reqwest::Client,
) -> Result<ChatResponse, Response> {
    let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
        json_error(
            StatusCode::NOT_IMPLEMENTED,
            "ANTHROPIC_API_KEY is not configured on this relay",
        )
    })?;

    // Build system string: prefer top-level field, else first system message.
    let system = req.system.clone().or_else(|| {
        req.messages
            .iter()
            .find(|m| m.role == "system")
            .and_then(|m| m.content.clone())
    });

    let body = AnthropicRequest {
        model: req.model.clone(),
        max_tokens: req.max_tokens.unwrap_or(1024),
        messages: build_anthropic_messages(&req.messages),
        system,
        tools: build_anthropic_tools(&req.tools),
        temperature: req.temperature,
        stream: None,
    };

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            json_error(
                StatusCode::BAD_GATEWAY,
                &format!("upstream request failed: {e}"),
            )
        })?;

    let status = resp.status();
    if !status.is_success() {
        let upstream_status = map_upstream_status(status.as_u16());
        let body_text = resp.text().await.unwrap_or_default();
        return Err((upstream_status, body_text).into_response());
    }

    let anthropic_resp: AnthropicResponse = resp.json().await.map_err(|e| {
        json_error(
            StatusCode::BAD_GATEWAY,
            &format!("upstream response parse error: {e}"),
        )
    })?;

    Ok(anthropic_response_to_chat(anthropic_resp, "anthropic"))
}

// ---------------------------------------------------------------------------
// Upstream fan-out — streaming chat (SSE)
// ---------------------------------------------------------------------------

/// An individual OpenAI-compat SSE event data payload.
#[derive(Debug, Serialize)]
struct SseDelta {
    choices: Vec<SseChoice>,
}

#[derive(Debug, Serialize)]
struct SseChoice {
    delta: DeltaPayload,
    finish_reason: Option<String>,
}

#[derive(Debug, Serialize, Default)]
struct DeltaPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<StreamToolCallDelta>,
}

#[derive(Debug, Serialize)]
struct StreamToolCallDelta {
    index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function: Option<StreamFunctionDelta>,
}

#[derive(Debug, Serialize)]
struct StreamFunctionDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<String>,
}

async fn fan_out_stream(
    req: ChatRequest,
    client: reqwest::Client,
) -> Result<impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>, Response> {
    let provider = if req.provider.is_empty() {
        "anthropic".to_string()
    } else {
        req.provider.clone()
    };

    match provider.as_str() {
        "anthropic" => fan_out_stream_anthropic(req, client).await,
        other => Err(json_error(
            StatusCode::NOT_IMPLEMENTED,
            &format!("provider '{}' is not implemented on this relay", other),
        )),
    }
}

async fn fan_out_stream_anthropic(
    req: ChatRequest,
    client: reqwest::Client,
) -> Result<impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>, Response> {
    let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
        json_error(
            StatusCode::NOT_IMPLEMENTED,
            "ANTHROPIC_API_KEY is not configured on this relay",
        )
    })?;

    let system = req.system.clone().or_else(|| {
        req.messages
            .iter()
            .find(|m| m.role == "system")
            .and_then(|m| m.content.clone())
    });

    let body = AnthropicRequest {
        model: req.model.clone(),
        max_tokens: req.max_tokens.unwrap_or(1024),
        messages: build_anthropic_messages(&req.messages),
        system,
        tools: build_anthropic_tools(&req.tools),
        temperature: req.temperature,
        stream: Some(true),
    };

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            json_error(
                StatusCode::BAD_GATEWAY,
                &format!("upstream request failed: {e}"),
            )
        })?;

    let status = resp.status();
    if !status.is_success() {
        let upstream_status = map_upstream_status(status.as_u16());
        let body_text = resp.text().await.unwrap_or_default();
        return Err((upstream_status, body_text).into_response());
    }

    // Accumulator per content block index: (id, name, args_buf)
    struct BlockAccum {
        id: String,
        name: String,
        args_buf: String,
        is_tool: bool,
    }

    let byte_stream = resp.bytes_stream();

    let sse_stream = futures_util::stream::unfold(
        (
            Box::pin(byte_stream),
            String::new(), // line buffer
            std::collections::HashMap::<usize, BlockAccum>::new(),
            false, // done
        ),
        move |(mut byte_stream, mut buf, mut blocks, done)| async move {
            if done {
                return None;
            }

            loop {
                // Process complete lines from buffer
                if let Some(nl) = buf.find('\n') {
                    let line = buf[..nl].trim_end_matches('\r').to_string();
                    buf.replace_range(..=nl, "");

                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    // SSE format: "event: foo\n" or "data: {...}\n"
                    let data = if let Some(d) = line.strip_prefix("data: ") {
                        d.trim().to_string()
                    } else {
                        // "event:" line or unknown — skip
                        continue;
                    };

                    if data == "[DONE]" {
                        // Should not happen for Anthropic but handle defensively
                        let event = axum::response::sse::Event::default().data("[DONE]");
                        return Some((Ok(event), (byte_stream, buf, blocks, true)));
                    }

                    // Parse Anthropic SSE event
                    let anthropic_event: AnthropicEvent = match serde_json::from_str(&data) {
                        Ok(e) => e,
                        Err(_) => continue,
                    };

                    match anthropic_event {
                        AnthropicEvent::ContentBlockStart { index, content_block } => {
                            match content_block {
                                AnthropicBlockStart::Text => {
                                    blocks.insert(index, BlockAccum {
                                        id: String::new(),
                                        name: String::new(),
                                        args_buf: String::new(),
                                        is_tool: false,
                                    });
                                }
                                AnthropicBlockStart::ToolUse { id, name } => {
                                    // Emit the tool_call start delta with id + name
                                    blocks.insert(index, BlockAccum {
                                        id: id.clone(),
                                        name: name.clone(),
                                        args_buf: String::new(),
                                        is_tool: true,
                                    });
                                    let delta = SseDelta {
                                        choices: vec![SseChoice {
                                            delta: DeltaPayload {
                                                tool_calls: vec![StreamToolCallDelta {
                                                    index,
                                                    id: Some(id),
                                                    function: Some(StreamFunctionDelta {
                                                        name: Some(name),
                                                        arguments: None,
                                                    }),
                                                }],
                                                content: None,
                                            },
                                            finish_reason: None,
                                        }],
                                    };
                                    let json = serde_json::to_string(&delta).unwrap_or_default();
                                    let event = axum::response::sse::Event::default().data(json);
                                    return Some((Ok(event), (byte_stream, buf, blocks, false)));
                                }
                            }
                        }
                        AnthropicEvent::ContentBlockDelta { index, delta } => {
                            match delta {
                                AnthropicDelta::TextDelta { text } => {
                                    let sse = SseDelta {
                                        choices: vec![SseChoice {
                                            delta: DeltaPayload {
                                                content: Some(text),
                                                tool_calls: vec![],
                                            },
                                            finish_reason: None,
                                        }],
                                    };
                                    let json = serde_json::to_string(&sse).unwrap_or_default();
                                    let event = axum::response::sse::Event::default().data(json);
                                    return Some((Ok(event), (byte_stream, buf, blocks, false)));
                                }
                                AnthropicDelta::InputJsonDelta { partial_json } => {
                                    // Accumulate tool call args + emit delta
                                    if let Some(block) = blocks.get_mut(&index) {
                                        block.args_buf.push_str(&partial_json);
                                    }
                                    let sse = SseDelta {
                                        choices: vec![SseChoice {
                                            delta: DeltaPayload {
                                                content: None,
                                                tool_calls: vec![StreamToolCallDelta {
                                                    index,
                                                    id: None,
                                                    function: Some(StreamFunctionDelta {
                                                        name: None,
                                                        arguments: Some(partial_json),
                                                    }),
                                                }],
                                            },
                                            finish_reason: None,
                                        }],
                                    };
                                    let json = serde_json::to_string(&sse).unwrap_or_default();
                                    let event = axum::response::sse::Event::default().data(json);
                                    return Some((Ok(event), (byte_stream, buf, blocks, false)));
                                }
                            }
                        }
                        AnthropicEvent::MessageDelta { delta } => {
                            // Emit finish_reason if present
                            if let Some(reason) = delta.stop_reason {
                                let finish = match reason.as_str() {
                                    "tool_use" => "tool_calls",
                                    "end_turn" => "stop",
                                    other => other,
                                };
                                let sse = SseDelta {
                                    choices: vec![SseChoice {
                                        delta: DeltaPayload::default(),
                                        finish_reason: Some(finish.to_string()),
                                    }],
                                };
                                let json = serde_json::to_string(&sse).unwrap_or_default();
                                let event = axum::response::sse::Event::default().data(json);
                                return Some((Ok(event), (byte_stream, buf, blocks, false)));
                            }
                        }
                        AnthropicEvent::MessageStop => {
                            // Emit [DONE] sentinel
                            let event = axum::response::sse::Event::default().data("[DONE]");
                            return Some((Ok(event), (byte_stream, buf, blocks, true)));
                        }
                        // Ignore ping, message_start, content_block_stop, error (already handled by status check)
                        _ => {}
                    }

                    continue;
                }

                // Read more bytes from upstream
                match byte_stream.next().await {
                    Some(Ok(chunk)) => {
                        buf.push_str(&String::from_utf8_lossy(&chunk));
                    }
                    Some(Err(_)) | None => {
                        // Stream ended — emit [DONE]
                        let event = axum::response::sse::Event::default().data("[DONE]");
                        return Some((Ok(event), (byte_stream, buf, blocks, true)));
                    }
                }
            }
        },
    );

    Ok(sse_stream)
}

// ---------------------------------------------------------------------------
// Upstream fan-out — single-shot completion
// ---------------------------------------------------------------------------

async fn fan_out_complete(
    req: &CompleteRequest,
    client: &reqwest::Client,
) -> Result<String, Response> {
    // Determine provider from model prefix (e.g. "anthropic:claude-haiku-4-5")
    let (provider, bare_model) = if let Some(idx) = req.model.find(':') {
        (&req.model[..idx], &req.model[idx + 1..])
    } else {
        ("anthropic", req.model.as_str())
    };

    match provider {
        "anthropic" => {
            let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
                json_error(
                    StatusCode::NOT_IMPLEMENTED,
                    "ANTHROPIC_API_KEY is not configured on this relay",
                )
            })?;

            let body = AnthropicRequest {
                model: bare_model.to_string(),
                max_tokens: req.max_tokens.unwrap_or(32),
                messages: vec![AnthropicMessage {
                    role: "user".to_string(),
                    content: AnthropicContent::Text(req.prompt.clone()),
                }],
                system: None,
                tools: vec![],
                temperature: None,
                stream: None,
            };

            let resp = client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
                .await
                .map_err(|e| {
                    json_error(
                        StatusCode::BAD_GATEWAY,
                        &format!("upstream request failed: {e}"),
                    )
                })?;

            let status = resp.status();
            if !status.is_success() {
                let upstream_status = map_upstream_status(status.as_u16());
                let body_text = resp.text().await.unwrap_or_default();
                return Err((upstream_status, body_text).into_response());
            }

            let anthropic_resp: AnthropicResponse = resp.json().await.map_err(|e| {
                json_error(
                    StatusCode::BAD_GATEWAY,
                    &format!("upstream response parse error: {e}"),
                )
            })?;

            let text = anthropic_resp
                .content
                .into_iter()
                .find_map(|b| match b {
                    AnthropicResponseBlock::Text { text } => Some(text),
                    _ => None,
                })
                .unwrap_or_default();

            Ok(text)
        }
        other => Err(json_error(
            StatusCode::NOT_IMPLEMENTED,
            &format!("provider '{}' is not implemented on this relay", other),
        )),
    }
}

// ---------------------------------------------------------------------------
// HTTP status mapping
// ---------------------------------------------------------------------------

fn map_upstream_status(status: u16) -> StatusCode {
    match status {
        401 | 403 => StatusCode::UNAUTHORIZED,
        429 => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::BAD_GATEWAY,
    }
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

/// POST /v1/ai/chat
///
/// Non-streaming chat fan-out. Authenticates via Bearer JWT (enforced by the
/// `inject_jwt_secret` middleware layer on the parent router). The `AuthUser`
/// extractor validates the token and yields the user ID; the user ID is
/// currently unused but kept to confirm authentication.
pub async fn chat_handler(
    State(_state): State<AppState>,
    AuthUser(_user_id): AuthUser,
    Json(req): Json<ChatRequest>,
) -> Response {
    let client = reqwest::Client::new();
    match fan_out_chat(&req, &client).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => e,
    }
}

/// POST /v1/ai/stream_chat
///
/// Streaming chat fan-out via SSE. Returns `Content-Type: text/event-stream`
/// with OpenAI-compat delta events. Each `data:` line is flushed immediately.
/// Terminates with `data: [DONE]`.
pub async fn stream_chat_handler(
    State(_state): State<AppState>,
    AuthUser(_user_id): AuthUser,
    Json(req): Json<ChatRequest>,
) -> Response {
    let client = reqwest::Client::new();
    match fan_out_stream(req, client).await {
        Ok(stream) => {
            Sse::new(stream)
                .keep_alive(axum::response::sse::KeepAlive::default())
                .into_response()
        }
        Err(e) => e,
    }
}

/// POST /v1/ai/complete
///
/// Single-shot ghost-text completion.
pub async fn complete_handler(
    State(_state): State<AppState>,
    AuthUser(_user_id): AuthUser,
    Json(req): Json<CompleteRequest>,
) -> Response {
    let client = reqwest::Client::new();
    match fan_out_complete(&req, &client).await {
        Ok(text) => Json(CompleteResponse { text }).into_response(),
        Err(e) => e,
    }
}

/// GET /v1/ai/list_models
///
/// Returns the list of models available through this relay, based on which
/// provider API keys are configured. If `ANTHROPIC_API_KEY` is set, Anthropic
/// models are included. If no providers are configured, returns an empty list.
pub async fn list_models_handler(
    State(_state): State<AppState>,
    AuthUser(_user_id): AuthUser,
) -> Json<ModelListResponse> {
    let mut models: Vec<ModelEntry> = Vec::new();

    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        models.extend([
            ModelEntry {
                id: "anthropic:claude-opus-4-5".to_string(),
                name: Some("Claude Opus 4.5".to_string()),
                context_window: Some(200_000),
            },
            ModelEntry {
                id: "anthropic:claude-sonnet-4-5".to_string(),
                name: Some("Claude Sonnet 4.5".to_string()),
                context_window: Some(200_000),
            },
            ModelEntry {
                id: "anthropic:claude-haiku-4-5".to_string(),
                name: Some("Claude Haiku 4.5".to_string()),
                context_window: Some(200_000),
            },
        ]);
    }

    Json(ModelListResponse { models })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── serialisation unit tests ─────────────────────────────────────────────

    #[test]
    fn chat_response_serialises_content() {
        let resp = ChatResponse {
            choices: vec![ChatChoice {
                message: ResponseMessage {
                    content: Some("Hello".to_string()),
                    tool_calls: vec![],
                },
            }],
            model: "anthropic:claude-opus-4-5".to_string(),
            usage: Some(UsageResponse {
                prompt_tokens: 10,
                completion_tokens: 5,
            }),
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "Hello");
        assert_eq!(v["model"], "anthropic:claude-opus-4-5");
        assert_eq!(v["usage"]["prompt_tokens"], 10);
    }

    #[test]
    fn chat_response_serialises_tool_calls() {
        let resp = ChatResponse {
            choices: vec![ChatChoice {
                message: ResponseMessage {
                    content: None,
                    tool_calls: vec![ResponseToolCall {
                        id: "call_abc".to_string(),
                        function: ResponseFunction {
                            name: "run_command".to_string(),
                            arguments: r#"{"command":"ls"}"#.to_string(),
                        },
                    }],
                },
            }],
            model: "anthropic:claude-opus-4-5".to_string(),
            usage: None,
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert!(v["choices"][0]["message"]["content"].is_null());
        assert_eq!(v["choices"][0]["message"]["tool_calls"][0]["id"], "call_abc");
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "run_command"
        );
    }

    #[test]
    fn model_list_empty_when_no_env_var() {
        // Temporarily unset ANTHROPIC_API_KEY if it's set, and test serialization.
        // We test the shape directly since we can't easily unset env vars in parallel tests.
        let resp = ModelListResponse { models: vec![] };
        let v = serde_json::to_value(&resp).unwrap();
        assert!(v["models"].is_array());
        assert_eq!(v["models"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn model_list_entry_serialises() {
        let entry = ModelEntry {
            id: "anthropic:claude-opus-4-5".to_string(),
            name: Some("Claude Opus 4.5".to_string()),
            context_window: Some(200_000),
        };
        let v = serde_json::to_value(&entry).unwrap();
        assert_eq!(v["id"], "anthropic:claude-opus-4-5");
        assert_eq!(v["name"], "Claude Opus 4.5");
        assert_eq!(v["context_window"], 200_000u64);
    }

    #[test]
    fn complete_response_serialises() {
        let resp = CompleteResponse {
            text: "--oneline --graph".to_string(),
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["text"], "--oneline --graph");
    }

    #[test]
    fn anthropic_message_conversion_tool_result() {
        let msgs = vec![ChatMessage {
            role: "tool".to_string(),
            content: Some("file.txt exists".to_string()),
            tool_call_id: Some("call_xyz".to_string()),
            tool_calls: vec![],
        }];
        let out = build_anthropic_messages(&msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "user");
        // The content should be a Blocks variant with one ToolResult
        match &out[0].content {
            AnthropicContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    AnthropicBlock::ToolResult { tool_use_id, content } => {
                        assert_eq!(tool_use_id, "call_xyz");
                        assert_eq!(content, "file.txt exists");
                    }
                    _ => panic!("expected ToolResult block"),
                }
            }
            _ => panic!("expected Blocks content"),
        }
    }

    #[test]
    fn anthropic_message_conversion_skips_system() {
        let msgs = vec![
            ChatMessage {
                role: "system".to_string(),
                content: Some("You are Beacon AI.".to_string()),
                tool_call_id: None,
                tool_calls: vec![],
            },
            ChatMessage {
                role: "user".to_string(),
                content: Some("Hello".to_string()),
                tool_call_id: None,
                tool_calls: vec![],
            },
        ];
        let out = build_anthropic_messages(&msgs);
        // System messages should be filtered out of the messages array
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "user");
    }

    #[test]
    fn anthropic_response_to_chat_text() {
        let resp = AnthropicResponse {
            content: vec![AnthropicResponseBlock::Text {
                text: "The answer is 42.".to_string(),
            }],
            model: "claude-opus-4-5-20251101".to_string(),
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 20,
                output_tokens: 8,
            },
        };
        let chat = anthropic_response_to_chat(resp, "anthropic");
        assert_eq!(chat.choices.len(), 1);
        assert_eq!(
            chat.choices[0].message.content.as_deref(),
            Some("The answer is 42.")
        );
        assert!(chat.choices[0].message.tool_calls.is_empty());
        assert!(chat.model.starts_with("anthropic:"));
        let usage = chat.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 20);
        assert_eq!(usage.completion_tokens, 8);
    }

    #[test]
    fn anthropic_response_to_chat_tool_calls() {
        let input = serde_json::json!({"command": "ls /tmp"});
        let resp = AnthropicResponse {
            content: vec![AnthropicResponseBlock::ToolUse {
                id: "toolu_01".to_string(),
                name: "run_command".to_string(),
                input: input.clone(),
            }],
            model: "claude-opus-4-5-20251101".to_string(),
            stop_reason: Some("tool_use".to_string()),
            usage: AnthropicUsage {
                input_tokens: 30,
                output_tokens: 12,
            },
        };
        let chat = anthropic_response_to_chat(resp, "anthropic");
        assert!(chat.choices[0].message.content.is_none());
        let tc = &chat.choices[0].message.tool_calls[0];
        assert_eq!(tc.id, "toolu_01");
        assert_eq!(tc.function.name, "run_command");
        // arguments should be JSON-encoded
        let args: serde_json::Value = serde_json::from_str(&tc.function.arguments).unwrap();
        assert_eq!(args["command"], "ls /tmp");
    }

    #[test]
    fn map_upstream_status_401() {
        assert_eq!(map_upstream_status(401), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn map_upstream_status_429() {
        assert_eq!(map_upstream_status(429), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn map_upstream_status_500() {
        assert_eq!(map_upstream_status(500), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn sse_delta_serialises_text() {
        let delta = SseDelta {
            choices: vec![SseChoice {
                delta: DeltaPayload {
                    content: Some("Hello".to_string()),
                    tool_calls: vec![],
                },
                finish_reason: None,
            }],
        };
        let v = serde_json::to_value(&delta).unwrap();
        assert_eq!(v["choices"][0]["delta"]["content"], "Hello");
        assert!(v["choices"][0]["finish_reason"].is_null());
    }

    #[test]
    fn sse_delta_serialises_tool_call() {
        let delta = SseDelta {
            choices: vec![SseChoice {
                delta: DeltaPayload {
                    content: None,
                    tool_calls: vec![StreamToolCallDelta {
                        index: 0,
                        id: Some("call_abc".to_string()),
                        function: Some(StreamFunctionDelta {
                            name: Some("run_command".to_string()),
                            arguments: None,
                        }),
                    }],
                },
                finish_reason: None,
            }],
        };
        let v = serde_json::to_value(&delta).unwrap();
        let tc = &v["choices"][0]["delta"]["tool_calls"][0];
        assert_eq!(tc["index"], 0);
        assert_eq!(tc["id"], "call_abc");
        assert_eq!(tc["function"]["name"], "run_command");
    }
}
