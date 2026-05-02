//! Verify that `Sec-WebSocket-Protocol: arc.json-rpc` is honoured by the
//! upgrade handler. The msgpack path remains the default.

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

#[tokio::test]
async fn no_subprotocol_falls_back_to_msgpack_path() {
    let (addr, _state) = spawn_test_server().await;
    let token = mint_access_token();
    let req = format!("ws://{addr}/ws?token={token}")
        .into_client_request()
        .unwrap();
    let (_ws, response) = tokio_tungstenite::connect_async(req).await.unwrap();
    assert!(
        response.headers().get("Sec-WebSocket-Protocol").is_none(),
        "absent header → msgpack default, no subprotocol echoed"
    );
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
