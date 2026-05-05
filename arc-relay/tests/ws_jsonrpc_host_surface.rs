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
    // Phase F: seq is now the canonical seq from ScrollbackRing, not the host-supplied value.
    assert!(resp["result"]["seq"].as_u64().is_some(), "seq field missing: {resp}");

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

// ─────────────────────────────────────────────────────────────────────────────
// Test 5 — Phase F: session_register wires RelayRouter + MirrorRegistry
//
// Verifies that after `arc.host.session_register` the session is fully wired:
// a subscribed watcher can receive `arc.mirror.frame` emitted via
// `arc.host.push_output` (which is the simplest end-to-end broadcast path
// available without Phase F T4 push-output canonical seq).
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn host_session_register_wires_relay_router() {
    let (addr, state) = common::spawn_test_server().await;
    let mut host = common::connect_jsonrpc(&addr).await;
    let mut watcher = common::connect_jsonrpc(&addr).await;

    let sid = "phase-f-it-1";

    // 1. Watcher subscribes first so the forwarder is set up before the host
    //    sends anything.
    let resp = watcher
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.session.subscribe",
            "params": { "session_id": sid, "token": "t" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "subscribe failed: {resp}");
    drain_participant(&mut watcher).await;

    // 2. Host registers the session — Phase F wires RelayRouter + MirrorRegistry.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "arc.host.session_register",
            "params": { "session_id": sid, "name": "shell" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "session_register failed: {resp}");

    // Verify the MirrorRegistry entry was created.
    // Give the async init a tick to complete (it's awaited inline but yield anyway).
    tokio::task::yield_now().await;
    assert!(
        state.mirror_registry.get(sid).await.is_some(),
        "MirrorRegistry entry missing after session_register",
    );

    // Verify the RelayRouter has the session.
    assert!(
        state.relay.get_session_info(sid).is_some(),
        "RelayRouter session missing after session_register",
    );

    // 3. Use arc.host.push_output as the broadcast trigger: host sends bytes
    //    and we assert the watcher receives arc.mirror.frame.
    //    (The host WS is also subscribed via the side-effects forwarder set up
    //    by `arc.host.session_register` in jsonrpc_handler.rs; drain the echo.)
    let payload = b"hello from phase-f\n".to_vec();
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "arc.host.push_output",
            "params": {
                "session_id": sid,
                "data": BASE64.encode(&payload),
                "seq": 1u64,
            },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "push_output failed: {resp}");
    // Drain the arc.mirror.frame echo off the host stream before any next call.
    let _ = host.next_notification().await;

    // 4. Watcher must receive arc.mirror.frame with the same bytes.
    let n = watcher.next_notification().await;
    assert_eq!(n["method"], "arc.mirror.frame", "unexpected notif: {n}");
    let echoed = BASE64
        .decode(n["params"]["data"].as_str().expect("data field"))
        .expect("decode data");
    assert_eq!(echoed, payload, "frame payload mismatch");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 6 — Phase F T2: session_unregister drops RelayRouter entry
//
// Register a session, then call `arc.host.session_unregister`. Verify that
// `state.relay.get_session_info` returns `None` afterward.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn host_session_unregister_drops_relay_router() {
    let (addr, state) = common::spawn_test_server().await;
    let mut host = common::connect_jsonrpc(&addr).await;

    let sid = "unrg-relay-1";

    // Register the session.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.host.session_register",
            "params": { "session_id": sid, "name": "shell" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "session_register failed: {resp}");

    // Relay router must have the session before unregister.
    assert!(
        state.relay.get_session_info(sid).is_some(),
        "RelayRouter session missing after session_register",
    );

    // Unregister.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "arc.host.session_unregister",
            "params": { "session_id": sid },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "session_unregister failed: {resp}");

    // Give the async relay a tick to process.
    tokio::task::yield_now().await;

    // RelayRouter entry must be gone.
    assert!(
        state.relay.get_session_info(sid).is_none(),
        "RelayRouter entry still present after session_unregister",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 7 — Phase F T2: session_unregister aborts translator (double-register
//          succeeds, proving old handles were cleaned up)
//
// Register → unregister → register again. The second register must succeed and
// result in the relay router having the session, proving the old translator was
// properly cleaned up (if not, the second register would race-fail or leave
// stale state).
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn host_session_unregister_aborts_translator() {
    let (addr, state) = common::spawn_test_server().await;
    let mut host = common::connect_jsonrpc(&addr).await;

    let sid = "unrg-xlat-1";

    // First register.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.host.session_register",
            "params": { "session_id": sid, "name": "shell" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "first session_register failed: {resp}");

    // Unregister — aborts translator + pump handles.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "arc.host.session_unregister",
            "params": { "session_id": sid },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "session_unregister failed: {resp}");

    // Drain the arc.session.gone notification emitted by session_unregister on
    // the host stream; if left in the buffer it would be returned by the next
    // request() call in place of the register response.
    let gone = host.next_notification().await;
    assert_eq!(gone["method"], "arc.session.gone", "expected gone notif, got: {gone}");

    // Give tasks a tick to be aborted.
    tokio::task::yield_now().await;

    // Second register — must succeed cleanly.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "arc.host.session_register",
            "params": { "session_id": sid, "name": "shell-2" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "second session_register failed: {resp}");

    // Relay router must have the freshly re-registered session.
    tokio::task::yield_now().await;
    assert!(
        state.relay.get_session_info(sid).is_some(),
        "RelayRouter session missing after second session_register",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 8 — Phase F T2: session_unregister marks DB row state = "ended"
//
// Requires a live Postgres instance (DATABASE_URL env var). Gated with
// `#[ignore]` so it only runs when the caller opts in with
// `cargo test -- --include-ignored`.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn host_session_unregister_marks_db_ended() {
    // Skip gracefully when DATABASE_URL is not set.
    let relay = match common::try_spawn_relay().await {
        Some(r) => r,
        None => {
            eprintln!("host_session_unregister_marks_db_ended: skipped (no DATABASE_URL)");
            return;
        }
    };

    // Create a real user so the JWT is valid and session ownership is correct.
    let user = common::create_user(&relay, "unrg-db@test.example").await;

    // Connect with a real auth token.
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{client::IntoClientRequest, http::HeaderValue},
    };
    let mut req = format!("ws://{}/ws?token={}", relay.addr, user.access_token)
        .into_client_request()
        .unwrap();
    req.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("arc.json-rpc"),
    );
    let (ws_inner, _) = connect_async(req).await.unwrap();
    let mut host = common::JsonRpcWs { inner: ws_inner };

    let sid = format!("unrg-db-{}", uuid::Uuid::new_v4());

    // Register the session (creates the DB row via best-effort spawn).
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.host.session_register",
            "params": { "session_id": sid, "name": "db-test-shell" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "session_register failed: {resp}");

    // Give the best-effort DB insert a moment to complete.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Unregister — triggers the best-effort update_session_state("ended") spawn.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "arc.host.session_unregister",
            "params": { "session_id": &sid },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "session_unregister failed: {resp}");

    // Give the best-effort DB update a moment to complete.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Query the DB directly and assert state == "ended".
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&db_url).await.expect("connect pool");
    let session = arc_relay::models::session::find_session(&pool, &sid)
        .await
        .expect("find_session query")
        .expect("session row missing from DB");
    assert_eq!(
        session.state, "ended",
        "expected DB state='ended', got '{}'",
        session.state,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test T5 — Phase F T5: shadow_diff_response resolves waiter
//
// Register a session, plant a oneshot waiter directly in the RelayRouter, then
// have the host fire `arc.host.shadow_diff_response`. Assert the waiter fires
// with the diff text.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn host_shadow_diff_response_resolves_waiter() {
    let (addr, state) = common::spawn_test_server().await;
    let mut host = common::connect_jsonrpc(&addr).await;

    let sid = "sdr-waiter-1";
    let req_id = "req-abc-001";

    // Register the session so the handler can parse session_id without errors.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.host.session_register",
            "params": { "session_id": sid, "name": "shell" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "session_register failed: {resp}");

    // Plant a waiter directly in the relay router.
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    state.relay.register_shadow_diff_waiter(req_id, tx);

    // Host fires shadow_diff_response.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "arc.host.shadow_diff_response",
            "params": {
                "session_id": sid,
                "request_id": req_id,
                "diff": "patch text",
            },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "shadow_diff_response failed: {resp}");

    // The waiter should have received the diff payload.
    let received = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("waiter timed out — handler did not call resolve_shadow_diff")
        .expect("oneshot channel closed");
    assert_eq!(received, "patch text", "diff payload mismatch");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 9 — Phase F T3: host_session_resize_updates_router
//
// After `arc.host.session_resize`, `state.relay.get_session_info` must return
// the updated cols/rows.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn host_session_resize_updates_router() {
    let (addr, state) = common::spawn_test_server().await;
    let mut host = common::connect_jsonrpc(&addr).await;

    let sid = "resize-it-1";

    // Register the session so RelayRouter has an entry.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.host.session_register",
            "params": { "session_id": sid, "name": "shell" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "session_register failed: {resp}");

    // Resize the session.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "arc.host.session_resize",
            "params": { "session_id": sid, "cols": 120u32, "rows": 40u32 },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "session_resize failed: {resp}");

    // Verify RelayRouter reflects the new size.
    tokio::task::yield_now().await;
    let info = state
        .relay
        .get_session_info(sid)
        .expect("RelayRouter session missing after resize");
    assert_eq!(info.cols, 120, "cols not updated: {:?}", info);
    assert_eq!(info.rows, 40, "rows not updated: {:?}", info);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 10 — Phase F T4: host_push_output_uses_canonical_seq
//
// Push two frames with bogus host-supplied seq=99. The returned seq must be
// canonical (small, monotone, NOT 99) because ScrollbackRing assigns it.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn host_push_output_uses_canonical_seq() {
    let (addr, _state) = common::spawn_test_server().await;
    let mut host = common::connect_jsonrpc(&addr).await;
    let mut watcher = common::connect_jsonrpc(&addr).await;

    let sid = "canonical-seq-it-1";

    // Watcher subscribes first.
    let resp = watcher
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.session.subscribe",
            "params": { "session_id": sid, "token": "t" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "subscribe failed: {resp}");
    drain_participant(&mut watcher).await;

    // Host registers the session so MirrorRegistry is wired.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "arc.host.session_register",
            "params": { "session_id": sid, "name": "shell" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "session_register failed: {resp}");

    // Push frame 1 with intentionally bogus seq=99.
    let resp1 = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "arc.host.push_output",
            "params": {
                "session_id": sid,
                "data": BASE64.encode(b"frame-one"),
                "seq": 99u64,
            },
        }))
        .await;
    assert_eq!(resp1["result"]["ok"], true, "push_output #1 failed: {resp1}");
    let seq1 = resp1["result"]["seq"].as_u64().expect("seq1 missing");
    assert_ne!(seq1, 99, "host seq must be discarded (frame 1)");

    // Drain host echo.
    let _ = host.next_notification().await;

    // Watcher receives arc.mirror.frame for frame 1.
    let n1 = watcher.next_notification().await;
    assert_eq!(n1["method"], "arc.mirror.frame", "unexpected notif: {n1}");

    // Push frame 2 with the same bogus seq=99.
    let resp2 = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "arc.host.push_output",
            "params": {
                "session_id": sid,
                "data": BASE64.encode(b"frame-two"),
                "seq": 99u64,
            },
        }))
        .await;
    assert_eq!(resp2["result"]["ok"], true, "push_output #2 failed: {resp2}");
    let seq2 = resp2["result"]["seq"].as_u64().expect("seq2 missing");
    assert_ne!(seq2, 99, "host seq must be discarded (frame 2)");

    // Canonical seq must be strictly monotone and small (not 99).
    assert!(seq2 > seq1, "seq must be monotone: seq1={seq1} seq2={seq2}");
    assert!(seq1 < 10, "canonical seq should start small, got {seq1}");

    // Drain host echo.
    let _ = host.next_notification().await;

    // Watcher receives arc.mirror.frame for frame 2.
    let n2 = watcher.next_notification().await;
    assert_eq!(n2["method"], "arc.mirror.frame", "unexpected notif: {n2}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test T6 — Phase F T6: shadow_diff_request reaches JSON-RPC host
//
// Trigger `shadow_routes::get_shadow_diff` via HTTP; the route registers a
// waiter and emits arc.host.shadow_diff_request to the JSON-RPC host.
// Assert the host receives the notification within 2s.
// If the T1 translator also fires (double-fire scenario), both notifs are
// tolerated — the test drains up to 2 and verifies at least one is correct.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn shadow_diff_request_reaches_jsonrpc_host() {
    use beacon_model::SessionId;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (addr, _state) = common::spawn_test_server().await;
    let mut host = common::connect_jsonrpc(&addr).await;

    // Use a valid ULID-format session ID — the shadow_routes handler parses it
    // as `SessionId` when building the msgpack ArcToClient::ShadowDiffRequest.
    let sid = SessionId::new().to_string();

    // 1. Host registers the session so arc_session_store has an entry.
    let resp = host
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.host.session_register",
            "params": { "session_id": &sid, "name": "shell" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "session_register failed: {resp}");

    // 2. Call GET /api/sessions/:id/shadow-diff via raw HTTP with a Bearer token.
    //    The route registers a waiter + emits arc.host.shadow_diff_request BEFORE
    //    blocking on the 10s host-reply timeout. Spawn the HTTP call so the test
    //    can proceed to read the notification concurrently.
    let token = common::mint_access_token();
    let port = addr.port();
    let sid_clone = sid.clone();
    tokio::spawn(async move {
        let req = format!(
            "GET /api/sessions/{sid_clone}/shadow-diff HTTP/1.1\r\n\
             Host: 127.0.0.1:{port}\r\n\
             Authorization: Bearer {token}\r\n\
             Connection: close\r\n\
             \r\n"
        );
        if let Ok(mut stream) = tokio::net::TcpStream::connect(addr).await {
            let _ = stream.write_all(req.as_bytes()).await;
            // Drain the response (504 timeout after 10s) so the server closes cleanly.
            let mut buf = Vec::new();
            let _ = tokio::time::timeout(
                Duration::from_secs(15),
                stream.read_to_end(&mut buf),
            )
            .await;
        }
    });

    // 3. Host must receive arc.host.shadow_diff_request within 2s.
    //    The emit fires before the 10s route timeout, so this should complete
    //    well within the window.
    let n = tokio::time::timeout(Duration::from_secs(2), host.next_notification())
        .await
        .expect("timed out waiting for arc.host.shadow_diff_request");
    assert_eq!(
        n["method"], "arc.host.shadow_diff_request",
        "unexpected notif: {n}"
    );
    assert_eq!(n["params"]["session_id"], sid, "session_id mismatch: {n}");
    assert!(
        n["params"]["request_id"].as_str().is_some(),
        "request_id missing: {n}"
    );
}
