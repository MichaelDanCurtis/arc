//! End-to-end WS tests for Phase D host surface.
//!
//! Verifies that:
//! 1. `arc.host.hello` returns `{ok:true, protocol_version:<N>}`.
//! 2. Host-fired `arc.host.block_start/chunk/end` reach a subscribed watcher
//!    as `arc.mirror.block_start/chunk/end` with matching fields.
//! 3. `arc.host.push_output` reaches a subscribed watcher as `arc.mirror.frame`.
//! 4. `arc.host.shadow_diff_response` is silent (no notification to watcher).

mod common;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use beacon_model::{BlockFooter, BlockHeader, BlockId, HostInfo, SessionId, TabId};
use beacon_proto::PROTOCOL_VERSION;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

// ─────────────────────────────────────────────────────────────────────────────
// Helper: drain the mandatory `arc.session.participant` notification that fires
// when a client calls `arc.session.subscribe` on a fresh session.
// ─────────────────────────────────────────────────────────────────────────────
async fn drain_participant(ws: &mut common::JsonRpcWs) {
    let n = ws.next_notification().await;
    assert_eq!(
        n["method"], "arc.session.participant",
        "expected participant notif, got: {n}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 1 — arc.host.hello
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn host_hello_returns_ack() {
    let (addr, _state) = common::spawn_test_server().await;
    let mut host = common::connect_jsonrpc(&addr).await;

    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.host.hello",
            "params": {
                "host_id": "host-A",
                "auth_token": "tok",
                "protocol_version": PROTOCOL_VERSION,
            },
        }))
        .await;

    assert_eq!(resp["result"]["ok"], true, "hello failed: {resp}");
    assert_eq!(
        resp["result"]["protocol_version"],
        PROTOCOL_VERSION,
        "wrong protocol_version: {resp}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2 — block_start / block_chunk / block_end reach subscribed watcher
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn host_block_stream_reaches_subscribed_watcher() {
    let (addr, _state) = common::spawn_test_server().await;
    let mut host = common::connect_jsonrpc(&addr).await;
    let mut watcher = common::connect_jsonrpc(&addr).await;

    // 1. Watcher subscribes first — this wires the broadcast forwarder so the
    //    watcher's WS connection is on the session's notification channel before
    //    any host messages arrive.
    let resp = watcher
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.session.subscribe",
            "params": { "session_id": "bs-s1", "token": "t" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "subscribe failed: {resp}");
    drain_participant(&mut watcher).await;

    // 2. Host registers the session (allocates the ArcSessionEntry and also
    //    wires the host WS to the same session notification channel).
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "arc.host.session_register",
            "params": { "session_id": "bs-s1", "name": "shell" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "session_register failed: {resp}");

    // 3. Build a BlockHeader — handler deserializes it from params.header.
    let block_id = BlockId::new();
    let header = BlockHeader {
        id: block_id,
        session_id: SessionId::new(),
        tab_id: TabId::new(),
        command: "ls".to_string(),
        cwd: PathBuf::from("/tmp"),
        started_at: std::time::SystemTime::UNIX_EPOCH,
        env_snapshot: HashMap::new(),
        host_info: HostInfo::current(),
    };

    // 4. Host fires block_start.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "arc.host.block_start",
            "params": {
                "session_id": "bs-s1",
                "header": serde_json::to_value(&header).unwrap(),
            },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "block_start failed: {resp}");
    // The host WS is also subscribed to this session's notification channel
    // (arc.host.session_register triggers side-effects subscription).  Drain
    // the arc.mirror.block_start echo off the host stream before the next
    // host.request() so it does not get returned as a response frame.
    let _ = host.next_notification().await;

    // Watcher receives arc.mirror.block_start.
    let n = watcher.next_notification().await;
    assert_eq!(n["method"], "arc.mirror.block_start", "unexpected notif: {n}");
    assert_eq!(n["params"]["session_id"], "bs-s1");
    assert_eq!(n["params"]["header"]["command"], "ls");

    // 5. Host fires block_chunk.
    let payload = b"output-bytes".to_vec();
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "arc.host.block_chunk",
            "params": {
                "session_id": "bs-s1",
                "block_id": block_id.to_string(),
                "seq": 1u64,
                "data": BASE64.encode(&payload),
            },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "block_chunk failed: {resp}");
    // Drain the arc.mirror.block_chunk echo off the host stream.
    let _ = host.next_notification().await;

    // Watcher receives arc.mirror.block_chunk.
    let n = watcher.next_notification().await;
    assert_eq!(n["method"], "arc.mirror.block_chunk", "unexpected notif: {n}");
    assert_eq!(n["params"]["block_id"], block_id.to_string());
    assert_eq!(n["params"]["seq"], 1u64);

    // 6. Host fires block_end.
    let footer = BlockFooter {
        id: block_id,
        exit_code: Some(0),
        ended_at: std::time::SystemTime::UNIX_EPOCH,
        shadow_git_ref: None,
    };
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "arc.host.block_end",
            "params": {
                "session_id": "bs-s1",
                "footer": serde_json::to_value(&footer).unwrap(),
            },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "block_end failed: {resp}");
    // (No need to drain block_end echo from host — no subsequent host.request().)

    // Watcher receives arc.mirror.block_end.
    let n = watcher.next_notification().await;
    assert_eq!(n["method"], "arc.mirror.block_end", "unexpected notif: {n}");
    assert_eq!(n["params"]["footer"]["exit_code"], 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3 — push_output reaches watcher as arc.mirror.frame
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn host_push_output_reaches_watcher_as_mirror_frame() {
    let (addr, _state) = common::spawn_test_server().await;
    let mut host = common::connect_jsonrpc(&addr).await;
    let mut watcher = common::connect_jsonrpc(&addr).await;

    // Watcher subscribes first.
    let resp = watcher
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.session.subscribe",
            "params": { "session_id": "po-s2", "token": "t" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "subscribe failed: {resp}");
    drain_participant(&mut watcher).await;

    // Host registers the session.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "arc.host.session_register",
            "params": { "session_id": "po-s2", "name": "shell" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "session_register failed: {resp}");

    // Host pushes output.
    let payload = b"hello\n".to_vec();
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "arc.host.push_output",
            "params": {
                "session_id": "po-s2",
                "data": BASE64.encode(&payload),
                "seq": 9u64,
            },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "push_output failed: {resp}");
    assert_eq!(resp["result"]["seq"], 9u64, "echoed seq wrong: {resp}");

    // Watcher receives arc.mirror.frame with the same bytes.
    let n = watcher.next_notification().await;
    assert_eq!(n["method"], "arc.mirror.frame", "unexpected notif: {n}");
    let echoed = BASE64
        .decode(n["params"]["data"].as_str().expect("data field"))
        .expect("decode data");
    assert_eq!(echoed, payload, "frame payload mismatch");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4 — shadow_diff_response is silent (no notification reaches watcher)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn host_shadow_diff_response_is_silent() {
    let (addr, _state) = common::spawn_test_server().await;
    let mut host = common::connect_jsonrpc(&addr).await;
    let mut watcher = common::connect_jsonrpc(&addr).await;

    // Watcher subscribes.
    let resp = watcher
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.session.subscribe",
            "params": { "session_id": "sd-s3", "token": "t" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "subscribe failed: {resp}");
    drain_participant(&mut watcher).await;

    // Host registers session.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "arc.host.session_register",
            "params": { "session_id": "sd-s3", "name": "shell" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "session_register failed: {resp}");

    // Host fires shadow_diff_response.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "arc.host.shadow_diff_response",
            "params": {
                "session_id": "sd-s3",
                "request_id": "req-99",
                "diff": "diff-bytes",
            },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "shadow_diff_response failed: {resp}");

    // Watcher should receive NO notification — the diff response is consumed
    // by the (Phase F) waiter map, not broadcast to watchers.
    let timed_out =
        tokio::time::timeout(Duration::from_millis(150), watcher.next_notification())
            .await
            .is_err();
    assert!(timed_out, "shadow_diff_response must not emit any notification to watcher");
}
