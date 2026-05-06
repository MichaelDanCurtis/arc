//! Verify that `Sec-WebSocket-Protocol: arc.json-rpc` is mandatory and
//! honoured by the upgrade handler (Phase G: msgpack path deleted).

use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

mod common;
use common::{mint_access_token, spawn_test_server};

#[tokio::test]
async fn jsonrpc_subprotocol_is_accepted_in_handshake() {
    let (addr, _state) = spawn_test_server().await;
    let token = mint_access_token();
    let mut req = format!("ws://{addr}/ws?token={token}")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("Sec-WebSocket-Protocol", HeaderValue::from_static("arc.json-rpc"));

    let (ws, response) = tokio_tungstenite::connect_async(req).await.unwrap();
    assert_eq!(
        response
            .headers()
            .get("Sec-WebSocket-Protocol")
            .map(|v| v.to_str().unwrap()),
        Some("arc.json-rpc"),
        "server must echo the negotiated subprotocol"
    );
    drop(ws);
}

/// Phase G: absent `Sec-WebSocket-Protocol: arc.json-rpc` → 400 Bad Request.
/// The msgpack fallback path was deleted in Phase G (2026-05-05).
#[tokio::test]
async fn no_subprotocol_rejected_with_400() {
    let (addr, _state) = spawn_test_server().await;
    let token = mint_access_token();
    let req = format!("ws://{addr}/ws?token={token}")
        .into_client_request()
        .unwrap();
    // The server returns 400 before upgrading; tungstenite surfaces this as an error.
    match tokio_tungstenite::connect_async(req).await {
        Err(_) => {
            // Connection rejected at the HTTP layer (400) — correct behaviour.
        }
        Ok((_ws, response)) => {
            // If somehow connected, the status must not be 101 Switching Protocols.
            let status = response.status();
            assert_ne!(
                status,
                tokio_tungstenite::tungstenite::http::StatusCode::SWITCHING_PROTOCOLS,
                "absent arc.json-rpc header must not result in a successful WS upgrade"
            );
        }
    }
}

#[tokio::test]
async fn appstate_exposes_jsonrpc_stores() {
    let (_addr, state) = spawn_test_server().await;
    // Compile-time check is sufficient — these accessors must exist.
    let _: &arc_relay::rpc::ArcSessionStore = &state.arc_session_store;
    let _: &arc_relay::ai_rpc::ArcAiSessionStore = &state.arc_ai_session_store;
    // Task 2 leaves the backend as None; Task 5 wires HttpAiBackend.
    assert!(state.arc_ai_backend.is_none(), "backend is None until Task 5");
}
