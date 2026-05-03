mod common;
use common::{connect_jsonrpc, spawn_test_server, JsonRpcWs};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::json;

async fn attach_and_drain_participant(ws: &mut JsonRpcWs, session_id: &str) {
    let resp = ws
        .request(json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "arc.session.attach",
            "params": {"session_id": session_id, "token": "tok-test"}
        }))
        .await;
    assert_eq!(resp["result"]["ok"], json!(true), "attach failed: {resp:?}");
    let _ = ws.next_notification().await;
}

#[tokio::test]
async fn detach_emits_participant_joined_false() {
    let (addr, _state) = spawn_test_server().await;
    let mut ws = connect_jsonrpc(&addr).await;
    attach_and_drain_participant(&mut ws, "s-detach").await;

    let resp = ws
        .request(json!({
            "jsonrpc": "2.0", "id": 2,
            "method": "arc.session.detach",
            "params": {"session_id": "s-detach", "token": "tok-test"}
        }))
        .await;
    assert_eq!(resp["result"]["ok"], json!(true), "detach failed: {resp:?}");

    let n = ws.next_notification().await;
    assert_eq!(n["method"], "arc.session.participant");
    assert_eq!(n["params"]["joined"], false);
}

#[tokio::test]
async fn subscribe_then_unsubscribe_round_trip() {
    let (addr, _state) = spawn_test_server().await;
    let mut ws = connect_jsonrpc(&addr).await;

    let sub = ws
        .request(json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "arc.session.subscribe",
            "params": {"session_id": "s-sub", "token": "tok"}
        }))
        .await;
    assert_eq!(sub["result"]["ok"], json!(true));
    let n = ws.next_notification().await;
    assert_eq!(n["method"], "arc.session.participant");
    assert_eq!(n["params"]["joined"], true);

    let unsub = ws
        .request(json!({
            "jsonrpc": "2.0", "id": 2,
            "method": "arc.session.unsubscribe",
            "params": {"session_id": "s-sub", "token": "tok"}
        }))
        .await;
    assert_eq!(unsub["result"]["ok"], json!(true));
    let n = ws.next_notification().await;
    assert_eq!(n["method"], "arc.session.participant");
    assert_eq!(n["params"]["joined"], false);
}

#[tokio::test]
async fn input_only_works_after_control() {
    let (addr, _state) = spawn_test_server().await;
    let mut ws = connect_jsonrpc(&addr).await;
    attach_and_drain_participant(&mut ws, "s-input").await;

    // No controller yet — input must fail.
    let resp = ws
        .request(json!({
            "jsonrpc": "2.0", "id": 2,
            "method": "arc.session.input",
            "params": {
                "session_id": "s-input",
                "requester_id": "u-alice",
                "data": BASE64.encode(b"x"),
            }
        }))
        .await;
    assert_eq!(resp["error"]["code"], json!(-32000));

    // Take control.
    let resp = ws
        .request(json!({
            "jsonrpc": "2.0", "id": 3,
            "method": "arc.session.control",
            "params": {"session_id": "s-input", "requester_id": "u-alice"}
        }))
        .await;
    assert_eq!(resp["result"]["ok"], json!(true));
    // Drain control_grant.
    let _ = ws.next_notification().await;

    // Now input succeeds and emits arc.host.input.
    let resp = ws
        .request(json!({
            "jsonrpc": "2.0", "id": 4,
            "method": "arc.session.input",
            "params": {
                "session_id": "s-input",
                "requester_id": "u-alice",
                "data": BASE64.encode(b"echo hi\n"),
            }
        }))
        .await;
    assert_eq!(resp["result"]["ok"], json!(true));

    let n = ws.next_notification().await;
    assert_eq!(n["method"], "arc.host.input");
    let b64 = n["params"]["data"].as_str().unwrap();
    assert_eq!(BASE64.decode(b64).unwrap(), b"echo hi\n");
}

#[tokio::test]
async fn resize_only_works_after_control() {
    let (addr, _state) = spawn_test_server().await;
    let mut ws = connect_jsonrpc(&addr).await;
    attach_and_drain_participant(&mut ws, "s-resize").await;

    // Without control → -32000.
    let resp = ws
        .request(json!({
            "jsonrpc": "2.0", "id": 2,
            "method": "arc.session.resize",
            "params": {
                "session_id": "s-resize",
                "requester_id": "u-alice",
                "cols": 80, "rows": 24,
            }
        }))
        .await;
    assert_eq!(resp["error"]["code"], json!(-32000));

    // Take control.
    let resp = ws
        .request(json!({
            "jsonrpc": "2.0", "id": 3,
            "method": "arc.session.control",
            "params": {"session_id": "s-resize", "requester_id": "u-alice"}
        }))
        .await;
    assert_eq!(resp["result"]["ok"], json!(true));
    let _ = ws.next_notification().await;

    let resp = ws
        .request(json!({
            "jsonrpc": "2.0", "id": 4,
            "method": "arc.session.resize",
            "params": {
                "session_id": "s-resize",
                "requester_id": "u-alice",
                "cols": 200, "rows": 60,
            }
        }))
        .await;
    assert_eq!(resp["result"]["ok"], json!(true));

    let n = ws.next_notification().await;
    assert_eq!(n["method"], "arc.host.resize_request");
    assert_eq!(n["params"]["cols"], 200);
    assert_eq!(n["params"]["rows"], 60);
}
