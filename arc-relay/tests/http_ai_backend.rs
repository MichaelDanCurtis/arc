//! Integration tests for `HttpAiBackend` driving a fake Anthropic server.
//!
//! Spawns an axum router on `127.0.0.1:0` that mimics the subset of the
//! Anthropic Messages API we depend on (non-streaming JSON response or SSE).
//! `HttpAiBackend::with_base_url` points the backend at the ephemeral port.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use arc_relay::ai_backend::HttpAiBackend;
use arc_relay::ai_rpc::{ArcAiBackend, ArcAiChatRequest, ArcAiMessage, ArcAiStreamEvent};
use axum::{
    extract::State,
    http::StatusCode,
    response::{sse::Event, IntoResponse, Sse},
    routing::post,
    Json, Router,
};
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use tokio::sync::Mutex;

#[derive(Default)]
struct FakeServerState {
    pending: Mutex<Vec<FakeResponse>>,
    received: AtomicUsize,
    last_body: Mutex<Option<Value>>,
    last_x_api_key: Mutex<Option<String>>,
}

#[derive(Clone)]
enum FakeResponse {
    JsonOk(Value),
    Status(StatusCode, String),
    Sse(Vec<Value>),
}

async fn handle_post(
    State(state): State<Arc<FakeServerState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    state.received.fetch_add(1, Ordering::SeqCst);
    if let Some(v) = headers.get("x-api-key").and_then(|h| h.to_str().ok()) {
        *state.last_x_api_key.lock().await = Some(v.to_string());
    }
    if let Ok(parsed) = serde_json::from_slice::<Value>(&body) {
        *state.last_body.lock().await = Some(parsed);
    }

    let plan = {
        let mut pending = state.pending.lock().await;
        if pending.is_empty() {
            None
        } else {
            Some(pending.remove(0))
        }
    };
    let plan = match plan {
        Some(p) => p,
        None => return (StatusCode::INTERNAL_SERVER_ERROR, "no plan").into_response(),
    };

    match plan {
        FakeResponse::JsonOk(v) => Json(v).into_response(),
        FakeResponse::Status(s, b) => (s, b).into_response(),
        FakeResponse::Sse(events) => {
            let stream = stream::iter(events.into_iter().map(|v| {
                Ok::<_, Infallible>(Event::default().data(v.to_string()))
            }));
            Sse::new(stream).into_response()
        }
    }
}

async fn spawn_fake_server(state: Arc<FakeServerState>) -> SocketAddr {
    let app = Router::new()
        .route("/v1/messages", post(handle_post))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn user_msg(text: &str) -> ArcAiMessage {
    ArcAiMessage {
        role: "user".to_string(),
        content: Some(text.to_string()),
    }
}

#[tokio::test]
async fn chat_returns_concatenated_text_blocks() {
    let _lock = ENV_LOCK.lock().await;
    std::env::set_var("ANTHROPIC_API_KEY", "test-key");

    let state = Arc::new(FakeServerState::default());
    state.pending.lock().await.push(FakeResponse::JsonOk(json!({
        "model": "claude-haiku-4-5",
        "content": [
            {"type": "text", "text": "hello "},
            {"type": "text", "text": "world"}
        ],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 1, "output_tokens": 2}
    })));

    let addr = spawn_fake_server(state.clone()).await;
    let backend = HttpAiBackend::with_base_url(format!("http://{addr}"));

    let req = ArcAiChatRequest {
        model: "anthropic:claude-haiku-4-5".to_string(),
        messages: vec![user_msg("hi")],
    };
    let out = backend.chat(&req).await.expect("chat ok");
    assert_eq!(out, "hello world");
    assert_eq!(state.received.load(Ordering::SeqCst), 1);
    assert_eq!(
        state.last_x_api_key.lock().await.as_deref(),
        Some("test-key")
    );
    let body = state.last_body.lock().await.clone().unwrap();
    assert_eq!(body["model"], "claude-haiku-4-5");
    assert_eq!(body["max_tokens"], 1024);
    assert!(body.get("stream").is_none(), "stream flag should be absent for chat()");
    std::env::remove_var("ANTHROPIC_API_KEY");
}

#[tokio::test]
async fn chat_returns_err_on_upstream_5xx() {
    let _lock = ENV_LOCK.lock().await;
    std::env::set_var("ANTHROPIC_API_KEY", "test-key");

    let state = Arc::new(FakeServerState::default());
    state
        .pending
        .lock()
        .await
        .push(FakeResponse::Status(StatusCode::BAD_GATEWAY, "boom".into()));

    let addr = spawn_fake_server(state).await;
    let backend = HttpAiBackend::with_base_url(format!("http://{addr}"));

    let req = ArcAiChatRequest {
        model: "anthropic:claude-haiku-4-5".to_string(),
        messages: vec![user_msg("hi")],
    };
    let err = backend.chat(&req).await.expect_err("chat should fail");
    assert!(err.contains("502"), "got: {err}");
    assert!(err.contains("boom"), "got: {err}");
    std::env::remove_var("ANTHROPIC_API_KEY");
}

#[tokio::test]
async fn stream_chat_emits_text_then_done() {
    let _lock = ENV_LOCK.lock().await;
    std::env::set_var("ANTHROPIC_API_KEY", "test-key");

    let state = Arc::new(FakeServerState::default());
    state.pending.lock().await.push(FakeResponse::Sse(vec![
        json!({"type": "message_start", "message": {"model": "claude-haiku-4-5"}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hi"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " there"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_stop"}),
    ]));

    let addr = spawn_fake_server(state.clone()).await;
    let backend = HttpAiBackend::with_base_url(format!("http://{addr}"));

    let req = ArcAiChatRequest {
        model: "anthropic:claude-haiku-4-5".to_string(),
        messages: vec![user_msg("hi")],
    };
    let mut s = backend.stream_chat(&req).await.expect("stream ok");

    let mut texts = Vec::new();
    let mut got_done = false;
    while let Some(ev) = s.next().await {
        match ev.expect("stream item") {
            ArcAiStreamEvent::Text(t) => texts.push(t),
            ArcAiStreamEvent::Done => {
                got_done = true;
                break;
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
    assert_eq!(texts, vec!["hi".to_string(), " there".to_string()]);
    assert!(got_done);

    let body = state.last_body.lock().await.clone().unwrap();
    assert_eq!(body["stream"], true, "stream_chat must set stream=true");
    std::env::remove_var("ANTHROPIC_API_KEY");
}

#[tokio::test]
async fn stream_chat_emits_tool_use_with_accumulated_arguments() {
    let _lock = ENV_LOCK.lock().await;
    std::env::set_var("ANTHROPIC_API_KEY", "test-key");

    let state = Arc::new(FakeServerState::default());
    state.pending.lock().await.push(FakeResponse::Sse(vec![
        json!({"type": "content_block_start", "index": 0, "content_block": {
            "type": "tool_use", "id": "call_1", "name": "echo"
        }}),
        json!({"type": "content_block_delta", "index": 0, "delta": {
            "type": "input_json_delta", "partial_json": "{\"text\":\"he"
        }}),
        json!({"type": "content_block_delta", "index": 0, "delta": {
            "type": "input_json_delta", "partial_json": "llo\"}"
        }}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_stop"}),
    ]));

    let addr = spawn_fake_server(state).await;
    let backend = HttpAiBackend::with_base_url(format!("http://{addr}"));

    let req = ArcAiChatRequest {
        model: "anthropic:claude-haiku-4-5".to_string(),
        messages: vec![user_msg("call it")],
    };
    let mut s = backend.stream_chat(&req).await.expect("stream ok");

    let mut events: Vec<ArcAiStreamEvent> = Vec::new();
    while let Some(ev) = s.next().await {
        let v = ev.expect("stream item");
        let stop = matches!(v, ArcAiStreamEvent::Done);
        events.push(v);
        if stop {
            break;
        }
    }
    assert_eq!(events.len(), 2, "expected 1 ToolUse + 1 Done, got {events:?}");
    match &events[0] {
        ArcAiStreamEvent::ToolUse { id, name, arguments } => {
            assert_eq!(id, "call_1");
            assert_eq!(name, "echo");
            assert_eq!(arguments, &json!({"text": "hello"}));
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
    assert!(matches!(events[1], ArcAiStreamEvent::Done));
    std::env::remove_var("ANTHROPIC_API_KEY");
}

#[tokio::test]
async fn stream_chat_propagates_inband_error_then_done() {
    let _lock = ENV_LOCK.lock().await;
    std::env::set_var("ANTHROPIC_API_KEY", "test-key");

    let state = Arc::new(FakeServerState::default());
    state.pending.lock().await.push(FakeResponse::Sse(vec![
        json!({"type": "error", "error": {"type": "overloaded", "message": "try later"}}),
    ]));

    let addr = spawn_fake_server(state).await;
    let backend = HttpAiBackend::with_base_url(format!("http://{addr}"));

    let req = ArcAiChatRequest {
        model: "anthropic:claude-haiku-4-5".to_string(),
        messages: vec![user_msg("hi")],
    };
    let mut s = backend.stream_chat(&req).await.expect("stream ok");

    let first = s.next().await.expect("first").expect("ok");
    let ArcAiStreamEvent::Error(msg) = first else {
        panic!("expected Error, got {first:?}");
    };
    assert!(msg.contains("overloaded"), "got: {msg}");

    let second = s.next().await.expect("second").expect("ok");
    assert!(matches!(second, ArcAiStreamEvent::Done));

    assert!(s.next().await.is_none(), "stream should end after Done");
    std::env::remove_var("ANTHROPIC_API_KEY");
}
