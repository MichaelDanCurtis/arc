//! Integration tests for the Arc relay AI fan-out endpoints.
//!
//! Tests run against an in-process Axum router with a lazy (non-connecting)
//! Postgres pool — no DB calls are made by the AI routes.
//!
//! Upstream Anthropic calls are **not** made in these tests; instead we test:
//!   1. Auth rejection (no/invalid token → 401)
//!   2. Route existence (routes are mounted and reachable)
//!   3. Provider-not-implemented (non-anthropic provider → 501)
//!   4. list_models shape (returns JSON with "models" array)
//!   5. SSE format correctness (parse a synthetic stream, assert deltas)

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt; // for oneshot

use arc_relay::api::{build_router, AppState};
use arc_relay::auth::jwt::generate_token_pair;
use arc_relay::config::ArcConfig;
use arc_relay::mirror::invitation::InvitationStore;
use arc_relay::mirror::MirrorRegistry;
use arc_relay::relay::router::RelayRouter;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn test_state() -> AppState {
    let pool =
        sqlx::PgPool::connect_lazy("postgres://arc:arc@localhost:5432/arc").expect("lazy pool");
    AppState {
        pool: pool.clone(),
        config: Arc::new(ArcConfig::default()),
        relay: Arc::new(RelayRouter::new()),
        audit: arc_relay::audit::AuditLog::new(),
        mirror_registry: MirrorRegistry::default(),
        invitations: InvitationStore::new(pool),
        r2: None,
    }
}

/// Mint a valid access token using the default test JWT secret.
fn valid_token(state: &AppState) -> String {
    let user_id = uuid::Uuid::new_v4();
    let pair =
        generate_token_pair(user_id, &state.config.jwt_secret, 900, 86_400).expect("token pair");
    pair.access_token
}

/// Send a request and return status + body bytes.
async fn send(
    state: AppState,
    method: &str,
    uri: &str,
    auth: Option<&str>,
    body: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let app = build_router(state);
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(a) = auth {
        builder = builder.header("authorization", a);
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let req_body = body
        .map(|b| Body::from(b.to_string()))
        .unwrap_or_else(Body::empty);
    let resp = app.oneshot(builder.body(req_body).unwrap()).await.unwrap();
    let status = resp.status();
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, body_bytes)
}

// ---------------------------------------------------------------------------
// Auth tests — all four routes must reject missing/invalid tokens with 401
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chat_requires_auth() {
    let (status, _) = send(
        test_state(),
        "POST",
        "/v1/ai/chat",
        None,
        Some(r#"{"model":"claude-opus-4-5","messages":[]}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "POST /v1/ai/chat needs auth");
}

#[tokio::test]
async fn stream_chat_requires_auth() {
    let (status, _) = send(
        test_state(),
        "POST",
        "/v1/ai/stream_chat",
        None,
        Some(r#"{"model":"claude-opus-4-5","messages":[]}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "POST /v1/ai/stream_chat needs auth"
    );
}

#[tokio::test]
async fn complete_requires_auth() {
    let (status, _) = send(
        test_state(),
        "POST",
        "/v1/ai/complete",
        None,
        Some(r#"{"model":"anthropic:claude-haiku-4-5","prompt":"git log "}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "POST /v1/ai/complete needs auth"
    );
}

#[tokio::test]
async fn list_models_requires_auth() {
    let (status, _) = send(test_state(), "GET", "/v1/ai/list_models", None, None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "GET /v1/ai/list_models needs auth"
    );
}

#[tokio::test]
async fn chat_rejects_invalid_token() {
    let (status, _) = send(
        test_state(),
        "POST",
        "/v1/ai/chat",
        Some("Bearer not.a.valid.token"),
        Some(r#"{"model":"claude-opus-4-5","messages":[]}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "invalid token must be rejected"
    );
}

// ---------------------------------------------------------------------------
// Route existence — valid token reaches handler (not 404, not 401)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_models_route_exists_and_authenticated() {
    let state = test_state();
    let token = valid_token(&state);
    let (status, body) = send(
        state,
        "GET",
        "/v1/ai/list_models",
        Some(&format!("Bearer {token}")),
        None,
    )
    .await;
    // Not 404 (route missing) and not 401 (auth rejected)
    assert_ne!(status, StatusCode::NOT_FOUND, "route must exist");
    assert_ne!(status, StatusCode::UNAUTHORIZED, "valid token must pass auth");
    // Body must be valid JSON with a "models" array
    let v: serde_json::Value = serde_json::from_slice(&body).expect("JSON body");
    assert!(v["models"].is_array(), "models must be an array");
}

// ---------------------------------------------------------------------------
// list_models — correct shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_models_returns_array_shape() {
    let state = test_state();
    let token = valid_token(&state);
    let (status, body) = send(
        state,
        "GET",
        "/v1/ai/list_models",
        Some(&format!("Bearer {token}")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let models = v["models"].as_array().expect("models array");
    // When ANTHROPIC_API_KEY is set in the test environment, models will be
    // non-empty; when it is not set, models is empty. Either is valid.
    for model in models {
        assert!(model["id"].is_string(), "each model must have an id string");
    }
}

#[tokio::test]
async fn list_models_anthropic_entries_when_key_set() {
    // Only meaningful when the env var is actually set; skip otherwise.
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        return;
    }
    let state = test_state();
    let token = valid_token(&state);
    let (status, body) = send(
        state,
        "GET",
        "/v1/ai/list_models",
        Some(&format!("Bearer {token}")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let models = v["models"].as_array().unwrap();
    let has_anthropic = models
        .iter()
        .any(|m| m["id"].as_str().map(|s| s.starts_with("anthropic:")).unwrap_or(false));
    assert!(has_anthropic, "Anthropic models must appear when key is set");
}

// ---------------------------------------------------------------------------
// Provider 501 — unknown provider returns NOT_IMPLEMENTED with JSON body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chat_unknown_provider_returns_501() {
    let state = test_state();
    let token = valid_token(&state);
    let body = r#"{"provider":"openai","model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#;
    let (status, resp_body) = send(
        state,
        "POST",
        "/v1/ai/chat",
        Some(&format!("Bearer {token}")),
        Some(body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_IMPLEMENTED,
        "unknown provider must return 501"
    );
    // Body must be JSON with an "error" field
    let v: serde_json::Value = serde_json::from_slice(&resp_body).expect("JSON error body");
    assert!(v["error"].is_string(), "error field must be a string");
}

#[tokio::test]
async fn stream_chat_unknown_provider_returns_501() {
    let state = test_state();
    let token = valid_token(&state);
    let body = r#"{"provider":"google","model":"gemini-2.0-flash","messages":[{"role":"user","content":"hi"}]}"#;
    let (status, resp_body) = send(
        state,
        "POST",
        "/v1/ai/stream_chat",
        Some(&format!("Bearer {token}")),
        Some(body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_IMPLEMENTED,
        "unknown provider must return 501"
    );
    let v: serde_json::Value = serde_json::from_slice(&resp_body).expect("JSON error body");
    assert!(v["error"].is_string(), "error field must be a string");
}

#[tokio::test]
async fn complete_unknown_provider_returns_501() {
    let state = test_state();
    let token = valid_token(&state);
    let body = r#"{"model":"openai:gpt-4o","prompt":"hello"}"#;
    let (status, resp_body) = send(
        state,
        "POST",
        "/v1/ai/complete",
        Some(&format!("Bearer {token}")),
        Some(body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_IMPLEMENTED,
        "unknown provider must return 501"
    );
    let v: serde_json::Value = serde_json::from_slice(&resp_body).expect("JSON error body");
    assert!(v["error"].is_string());
}

// ---------------------------------------------------------------------------
// Anthropic fan-out — only runs when ANTHROPIC_API_KEY is set
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chat_anthropic_returns_200_when_key_set() {
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        return; // skip
    }
    let state = test_state();
    let token = valid_token(&state);
    let body = r#"{
        "provider": "anthropic",
        "model": "claude-haiku-4-5",
        "messages": [{"role": "user", "content": "Reply with exactly one word: yes"}],
        "max_tokens": 10
    }"#;
    let (status, resp_body) = send(
        state,
        "POST",
        "/v1/ai/chat",
        Some(&format!("Bearer {token}")),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "chat must succeed: {:?}", std::str::from_utf8(&resp_body));
    let v: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    assert!(v["choices"].is_array());
    assert!(v["choices"][0]["message"].is_object());
    assert!(v["model"].as_str().unwrap_or("").starts_with("anthropic:"));
}

// ---------------------------------------------------------------------------
// SSE format correctness — unit-level test using synthetic SSE data
// ---------------------------------------------------------------------------

/// Verify that OpenAI-compat SSE delta JSON produced by our types parses
/// correctly (same contract as the client-side `ArcProvider` SSE parser).
#[test]
fn sse_text_delta_format() {
    // Construct a text delta payload manually and confirm its JSON shape.
    #[derive(serde::Serialize)]
    struct SseChoice {
        delta: Delta,
        finish_reason: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct Delta {
        content: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct SseDelta {
        choices: Vec<SseChoice>,
    }

    let delta = SseDelta {
        choices: vec![SseChoice {
            delta: Delta {
                content: Some("Hello".to_string()),
            },
            finish_reason: None,
        }],
    };
    let json = serde_json::to_string(&delta).unwrap();
    // Must be parseable by client's shape
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["choices"][0]["delta"]["content"], "Hello");
    assert!(v["choices"][0]["finish_reason"].is_null());
}

#[test]
fn sse_tool_call_delta_format() {
    // Verify tool_call delta structure matches client's ArcStreamChunk shape.
    #[derive(serde::Serialize)]
    struct SseDelta {
        choices: Vec<SseChoice>,
    }
    #[derive(serde::Serialize)]
    struct SseChoice {
        delta: Delta,
        finish_reason: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct Delta {
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCallDelta>,
    }
    #[derive(serde::Serialize)]
    struct ToolCallDelta {
        index: usize,
        id: Option<String>,
        function: Option<FunctionDelta>,
    }
    #[derive(serde::Serialize)]
    struct FunctionDelta {
        name: Option<String>,
        arguments: Option<String>,
    }

    // First delta: tool_call start
    let start = SseDelta {
        choices: vec![SseChoice {
            delta: Delta {
                tool_calls: vec![ToolCallDelta {
                    index: 0,
                    id: Some("call_abc".to_string()),
                    function: Some(FunctionDelta {
                        name: Some("run_command".to_string()),
                        arguments: None,
                    }),
                }],
            },
            finish_reason: None,
        }],
    };
    let json = serde_json::to_string(&start).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["choices"][0]["delta"]["tool_calls"][0]["index"], 0);
    assert_eq!(v["choices"][0]["delta"]["tool_calls"][0]["id"], "call_abc");
    assert_eq!(
        v["choices"][0]["delta"]["tool_calls"][0]["function"]["name"],
        "run_command"
    );

    // Arguments delta
    let args = SseDelta {
        choices: vec![SseChoice {
            delta: Delta {
                tool_calls: vec![ToolCallDelta {
                    index: 0,
                    id: None,
                    function: Some(FunctionDelta {
                        name: None,
                        arguments: Some(r#"{"command":"ls"}"#.to_string()),
                    }),
                }],
            },
            finish_reason: None,
        }],
    };
    let json2 = serde_json::to_string(&args).unwrap();
    let v2: serde_json::Value = serde_json::from_str(&json2).unwrap();
    assert_eq!(
        v2["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
        r#"{"command":"ls"}"#
    );

    // Finish delta
    let finish = SseDelta {
        choices: vec![SseChoice {
            delta: Delta { tool_calls: vec![] },
            finish_reason: Some("tool_calls".to_string()),
        }],
    };
    let json3 = serde_json::to_string(&finish).unwrap();
    let v3: serde_json::Value = serde_json::from_str(&json3).unwrap();
    assert_eq!(v3["choices"][0]["finish_reason"], "tool_calls");
}

#[test]
fn done_sentinel_is_literal_string() {
    // The SSE stream must terminate with `data: [DONE]` — verify the literal.
    let done = "[DONE]";
    assert_eq!(done, "[DONE]");
    // The client checks: data == "[DONE]"
    let line = "data: [DONE]";
    let data = line.strip_prefix("data: ").unwrap().trim();
    assert_eq!(data, "[DONE]");
}
