//! Integration tests for `arc.ai.*` JSON-RPC methods over WebSocket.
//!
//! Uses a mock [`ArcAiBackend`] so no external AI service is required.
//! The server is spun up with [`spawn_test_server_with_ai`] which injects
//! the backend into `AppState::arc_ai_backend` so the WS handler registers
//! the handlers.

mod common;
use common::{connect_jsonrpc, spawn_test_server_with_ai};

use std::sync::Arc;

use arc_relay::ai_rpc::{ArcAiBackend, ArcAiChatRequest, ArcAiModel, ArcAiStream, ArcAiStreamEvent};
use async_trait::async_trait;
use futures_util::stream;
use serde_json::json;

// ─────────────────────────────────────────────────────────────────────────────
// Mock backend
// ─────────────────────────────────────────────────────────────────────────────

/// Minimal mock backend: fixed chat reply, two-chunk stream, fixed model list.
struct ChunkBackend;

#[async_trait]
impl ArcAiBackend for ChunkBackend {
    async fn chat(&self, _req: &ArcAiChatRequest) -> Result<String, String> {
        Ok("hello world".into())
    }

    async fn stream_chat(&self, _req: &ArcAiChatRequest) -> Result<ArcAiStream, String> {
        let events: Vec<Result<ArcAiStreamEvent, String>> = vec![
            Ok(ArcAiStreamEvent::Text("hello ".into())),
            Ok(ArcAiStreamEvent::Text("world".into())),
            Ok(ArcAiStreamEvent::Done),
        ];
        Ok(Box::pin(stream::iter(events)))
    }

    async fn list_models(&self) -> Vec<ArcAiModel> {
        vec![
            ArcAiModel {
                id: "test-model-1".into(),
                name: Some("Test Model One".into()),
                context_window: Some(128_000),
            },
            ArcAiModel {
                id: "test-model-2".into(),
                name: None,
                context_window: None,
            },
        ]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.ai.chat over WS
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn arc_ai_chat_over_ws_returns_content() {
    let (addr, _state) = spawn_test_server_with_ai(Arc::new(ChunkBackend)).await;
    let mut ws = connect_jsonrpc(&addr).await;

    let resp = ws
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.ai.chat",
            "params": {
                "model": "test-model-1",
                "messages": [{"role": "user", "content": "hi"}]
            }
        }))
        .await;

    assert_eq!(resp["id"], json!(1));
    assert_eq!(
        resp["result"]["content"],
        json!("hello world"),
        "arc.ai.chat must return the backend's reply in result.content"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.ai.stream_chat over WS — receives ai.chunk + ai.done notifications
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn arc_ai_stream_chat_over_ws_emits_chunks_then_done() {
    let (addr, _state) = spawn_test_server_with_ai(Arc::new(ChunkBackend)).await;
    let mut ws = connect_jsonrpc(&addr).await;

    // Fire stream_chat. The WS handler subscribes to the AI session's broadcast
    // channel as a side effect of seeing this method (before dispatching to the
    // RPC handler), so notifications will be forwarded to us.
    let ack = ws
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "arc.ai.stream_chat",
            "params": {
                "session_id": "ai-sess-1",
                "model": "test-model-1",
                "messages": [{"role": "user", "content": "stream this"}]
            }
        }))
        .await;

    assert_eq!(ack["id"], json!(2));
    assert_eq!(
        ack["result"]["ok"],
        json!(true),
        "stream_chat ack must be {{ok:true}}"
    );

    // Collect notifications until ai.done arrives.
    let mut chunks: Vec<String> = Vec::new();
    let done_params = loop {
        let notif = ws.next_notification().await;
        let method = notif["method"].as_str().expect("method field");
        match method {
            "ai.chunk" => {
                let delta = notif["params"]["delta"]
                    .as_str()
                    .expect("delta field")
                    .to_string();
                chunks.push(delta);
            }
            "ai.done" => break notif["params"].clone(),
            other => panic!("unexpected notification method: {other}"),
        }
    };

    assert_eq!(chunks, vec!["hello ", "world"], "must receive both text chunks");
    assert_eq!(done_params["content"], json!("hello world"), "assembled content must match");
    assert_eq!(done_params["session_id"], json!("ai-sess-1"));
    assert_eq!(done_params["model"], json!("test-model-1"));
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.ai.list_models over WS
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn arc_ai_list_models_over_ws_returns_model_list() {
    let (addr, _state) = spawn_test_server_with_ai(Arc::new(ChunkBackend)).await;
    let mut ws = connect_jsonrpc(&addr).await;

    let resp = ws
        .request(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "arc.ai.list_models",
            "params": {}
        }))
        .await;

    assert_eq!(resp["id"], json!(3));
    let models = resp["result"]["models"]
        .as_array()
        .expect("models must be an array");
    assert_eq!(models.len(), 2);
    assert_eq!(models[0]["id"], json!("test-model-1"));
    assert_eq!(models[0]["name"], json!("Test Model One"));
    assert_eq!(models[0]["context_window"], json!(128_000u64));
    assert_eq!(models[1]["id"], json!("test-model-2"));
    // name/context_window are None → absent from serialized output
    assert!(models[1].get("name").is_none() || models[1]["name"].is_null());
}

// ─────────────────────────────────────────────────────────────────────────────
// arc.ai.chat — missing model → -32602 invalid params
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn arc_ai_chat_missing_model_returns_invalid_params() {
    let (addr, _state) = spawn_test_server_with_ai(Arc::new(ChunkBackend)).await;
    let mut ws = connect_jsonrpc(&addr).await;

    let resp = ws
        .request(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "arc.ai.chat",
            "params": {
                "messages": [{"role": "user", "content": "hi"}]
            }
        }))
        .await;

    assert_eq!(
        resp["error"]["code"],
        json!(-32602),
        "missing model must yield InvalidParams (-32602)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// ai.* methods absent when backend is None (guard test)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn arc_ai_methods_absent_without_backend() {
    // spawn_test_server leaves arc_ai_backend=None; arc.ai.* should be unregistered.
    let (addr, _state) = common::spawn_test_server().await;
    let mut ws = connect_jsonrpc(&addr).await;

    let resp = ws
        .request(json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "arc.ai.chat",
            "params": {
                "model": "test-model-1",
                "messages": [{"role": "user", "content": "hi"}]
            }
        }))
        .await;

    // -32601 = Method not found
    assert_eq!(
        resp["error"]["code"],
        json!(-32601),
        "arc.ai.chat must return MethodNotFound when no backend is configured"
    );
}
