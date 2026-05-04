//! End-to-end WS test for Phase C approve/deny + host-input echo.
//!
//! Verifies that:
//! 1. `arc.session.approve_control` promotes the requester to controller and
//!    fires `arc.mirror.control_grant`.
//! 2. `arc.session.deny_control` is silent (no notification).
//! 3. `arc.session.input` from the controller fires BOTH `arc.host.input`
//!    and `arc.mirror.host_input`, with the latter carrying the controller's
//!    user id as `from_user`.

mod common;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::json;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn approve_promotes_controller_and_grants() {
    let (addr, state) = common::spawn_test_server().await;
    let mut ws = common::connect_jsonrpc(&addr).await;

    // Subscribe so we get the notification stream.
    let resp = ws
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.session.subscribe",
            "params": { "session_id": "sess-app", "token": "tok" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "subscribe failed: {resp}");
    let _participant = ws.next_notification().await; // drain participant notif

    // Pre-seed the controller as the owner via the shared AppState.
    let entry = state.arc_session_store.get_or_create("sess-app").await;
    *entry.controller.lock().await = Some("owner".into());

    // Approve a request: bob is the requester.
    let resp = ws
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "arc.session.approve_control",
            "params": {
                "session_id": "sess-app",
                "request_id": "req-1",
                "requester_id": "bob",
            },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "approve failed: {resp}");

    // Notification: arc.mirror.control_grant with new holder.
    let n = ws.next_notification().await;
    assert_eq!(n["method"], "arc.mirror.control_grant");
    assert_eq!(n["params"]["session_id"], "sess-app");
    assert_eq!(n["params"]["holder_id"], "bob");

    // Server-side state confirms promotion.
    assert_eq!(*entry.controller.lock().await, Some("bob".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn deny_is_silent() {
    let (addr, state) = common::spawn_test_server().await;
    let mut ws = common::connect_jsonrpc(&addr).await;

    // Subscribe so the side-effects hook creates the session entry and attaches
    // a forwarder; the deny handler uses store.get() which requires the entry
    // to exist.
    let _ = ws
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.session.subscribe",
            "params": { "session_id": "sess-deny", "token": "tok" },
        }))
        .await;
    let _ = ws.next_notification().await; // drain participant notif

    let entry = state.arc_session_store.get_or_create("sess-deny").await;
    *entry.controller.lock().await = Some("owner".into());

    let resp = ws
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "arc.session.deny_control",
            "params": {
                "session_id": "sess-deny",
                "request_id": "req-9",
                "requester_id": "bob",
            },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true);

    // Controller unchanged.
    assert_eq!(*entry.controller.lock().await, Some("owner".to_string()));

    // No notification within a short timeout.
    let timed_out = tokio::time::timeout(Duration::from_millis(150), ws.next_notification())
        .await
        .is_err();
    assert!(timed_out, "deny_control should not emit any notification");
}

#[tokio::test(flavor = "multi_thread")]
async fn input_emits_both_host_and_mirror() {
    let (addr, state) = common::spawn_test_server().await;
    let mut ws = common::connect_jsonrpc(&addr).await;

    let _ = ws
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.session.subscribe",
            "params": { "session_id": "sess-echo", "token": "tok" },
        }))
        .await;
    let _ = ws.next_notification().await; // drain participant notif

    let entry = state.arc_session_store.get_or_create("sess-echo").await;
    *entry.controller.lock().await = Some("alice".into());

    let resp = ws
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "arc.session.input",
            "params": {
                "session_id": "sess-echo",
                "requester_id": "alice",
                "data": BASE64.encode(b"echo hi"),
            },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true);

    // Read two notifications in any order; assert both methods appeared.
    let mut saw_host = false;
    let mut saw_mirror = false;
    for _ in 0..2 {
        let n = ws.next_notification().await;
        match n["method"].as_str().unwrap() {
            "arc.host.input" => {
                saw_host = true;
                assert_eq!(
                    BASE64.decode(n["params"]["data"].as_str().unwrap()).unwrap(),
                    b"echo hi",
                );
            }
            "arc.mirror.host_input" => {
                saw_mirror = true;
                assert_eq!(n["params"]["from_user"], "alice");
                assert_eq!(
                    BASE64.decode(n["params"]["data"].as_str().unwrap()).unwrap(),
                    b"echo hi",
                );
            }
            other => panic!("unexpected notif: {other}"),
        }
    }
    assert!(saw_host && saw_mirror, "expected both notifications");
}
