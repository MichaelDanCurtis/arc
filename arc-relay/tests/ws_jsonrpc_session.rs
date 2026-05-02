mod common;
use common::{connect_jsonrpc, spawn_test_server};
use serde_json::{json, Value};

#[tokio::test]
async fn arc_session_attach_returns_ok() {
    let (addr, _state) = spawn_test_server().await;
    let mut ws = connect_jsonrpc(&addr).await;
    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "arc.session.attach",
        "params": {"session_id": "s-1", "token": "tok-foo"}
    });
    let resp = ws.request(req).await;
    assert_eq!(resp["result"]["ok"], json!(true));
    assert_eq!(resp["id"], json!(1));
}

#[tokio::test]
async fn attach_then_control_emits_grant_notification() {
    let (addr, _state) = spawn_test_server().await;
    let mut ws = common::connect_jsonrpc(&addr).await;

    let attach = json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "arc.session.attach",
        "params": {"session_id": "s-1", "token": "tok-foo"}
    });
    let _ = ws.request(attach).await;

    let control = json!({
        "jsonrpc": "2.0", "id": 2,
        "method": "arc.session.control",
        "params": {"session_id": "s-1", "requester_id": "u-bob"}
    });
    let _ = ws.request(control).await;

    let notif = ws.next_notification().await;
    assert_eq!(notif["method"], "arc.mirror.control_grant");
    assert_eq!(notif["params"]["session_id"], "s-1");
    assert_eq!(notif["params"]["holder_id"], "u-bob");
}
