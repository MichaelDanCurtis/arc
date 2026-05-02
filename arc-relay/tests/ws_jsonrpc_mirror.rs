//! Integration tests for arc.mirror.* JSON-RPC notifications bridged via
//! `MirrorRegistry::broadcast`.
//!
//! These tests exercise the full emitter pipeline:
//!   1. `ArcMirrorEmitter` registered on `MirrorRegistry` at server startup.
//!   2. `MirrorRegistry::broadcast(session_id, frame)` called directly on the
//!      in-process `AppState`.
//!   3. JSON-RPC WS client receives `arc.mirror.frame` /
//!      `arc.mirror.control_grant` / `arc.mirror.control_revoke` notifications.
//!
//! The tests do NOT go through the full host WebSocket path (PushOutput) —
//! they call `mirror_registry.broadcast` directly so they don't need a live
//! Postgres DB. A session entry must be pre-created in `arc_session_store` so
//! the emitter can look it up; `mirror_registry.get_or_init` seeds the msgpack
//! side.

mod common;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use beacon_proto::envelope::ArcToClient;
use serde_json::json;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Helper: seed both sides of the bridge for a session.
// ─────────────────────────────────────────────────────────────────────────────

/// Pre-create the session in both `mirror_registry` and `arc_session_store`.
///
/// `mirror_registry.get_or_init` seeds the msgpack `SessionMirror`.
/// `arc_session_store.get_or_create` seeds the JSON-RPC `ArcSessionEntry` that
/// `ArcMirrorEmitter::on_broadcast` looks up via `.get`.
async fn seed_session(state: &arc_relay::api::AppState, session_id: &str) {
    let owner = Uuid::new_v4();
    state
        .mirror_registry
        .get_or_init(session_id, owner, vec![owner])
        .await;
    state.arc_session_store.get_or_create(session_id).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// `SessionOutput` frame → `arc.mirror.frame` notification with base64 data.
#[tokio::test]
async fn mirror_broadcast_session_output_emits_frame_notification() {
    let (addr, state) = common::spawn_test_server().await;

    // Attach a JSON-RPC WS client and subscribe to the session's notif stream.
    let mut ws = common::connect_jsonrpc(&addr).await;
    let attach = json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "arc.session.attach",
        "params": {"session_id": "s-mirror", "token": "tok-test"}
    });
    let resp = ws.request(attach).await;
    assert_eq!(resp["result"]["ok"], json!(true), "attach failed: {resp:?}");

    // Drain the participant notification from attach.
    let _participant = ws.next_notification().await;

    // Seed the session so the emitter can look it up.
    seed_session(&state, "s-mirror").await;

    let payload = b"hello mirror";
    state
        .mirror_registry
        .broadcast(
            "s-mirror",
            ArcToClient::SessionOutput {
                session_id: beacon_model::SessionId::new(),
                bytes: payload.to_vec(),
                seq: 1,
                replay: false,
            },
        )
        .await;

    // Give the spawned task a moment to run.
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let notif = ws.next_notification().await;
    assert_eq!(notif["method"], "arc.mirror.frame", "unexpected method: {notif:?}");
    assert_eq!(notif["params"]["session_id"], "s-mirror");
    let data_b64 = notif["params"]["data"].as_str().expect("data field missing");
    let decoded = BASE64.decode(data_b64).expect("base64 decode");
    assert_eq!(decoded, payload);
}

/// `LeaseChanged { driver: Some(...) }` → `arc.mirror.control_grant`.
#[tokio::test]
async fn mirror_broadcast_lease_changed_some_emits_control_grant() {
    let (addr, state) = common::spawn_test_server().await;

    let mut ws = common::connect_jsonrpc(&addr).await;
    let attach = json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "arc.session.attach",
        "params": {"session_id": "s-mirror-grant", "token": "tok-test"}
    });
    let resp = ws.request(attach).await;
    assert_eq!(resp["result"]["ok"], json!(true), "attach failed: {resp:?}");

    let _participant = ws.next_notification().await;

    seed_session(&state, "s-mirror-grant").await;

    let driver = beacon_proto::ipc::DriverInfo {
        user_id: "u-alice".to_string(),
        device_label: "MacBook".to_string(),
        since: 0,
    };
    state
        .mirror_registry
        .broadcast(
            "s-mirror-grant",
            ArcToClient::LeaseChanged {
                session_id: beacon_model::SessionId::new(),
                driver: Some(driver),
            },
        )
        .await;

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let notif = ws.next_notification().await;
    assert_eq!(notif["method"], "arc.mirror.control_grant", "unexpected method: {notif:?}");
    assert_eq!(notif["params"]["session_id"], "s-mirror-grant");
    assert_eq!(notif["params"]["holder_id"], "u-alice");
}

/// End-to-end watcher lifecycle: attach → PTY frame → take control → release.
///
/// Exercises all three notification types (`arc.mirror.frame`,
/// `arc.mirror.control_grant`, `arc.mirror.control_revoke`) in a single
/// connected scenario via one WS client.
#[tokio::test]
async fn watcher_full_lifecycle() {
    let (addr, state) = common::spawn_test_server().await;
    let mut watcher = common::connect_jsonrpc(&addr).await;

    // Watcher attaches; server creates the ArcSessionEntry and emits a
    // participant notification.
    let resp = watcher
        .request(json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "arc.session.attach",
            "params": {"session_id": "s-life", "token": "t"}
        }))
        .await;
    assert_eq!(resp["result"]["ok"], json!(true), "attach failed: {resp:?}");

    // Drain the participant notification that attach always emits.
    let _participant = watcher.next_notification().await;

    // Seed the mirror-registry side so broadcast can look up the entry.
    seed_session(&state, "s-life").await;

    // PTY emits a frame via mirror_registry.broadcast (T6 path).
    state
        .mirror_registry
        .broadcast(
            "s-life",
            ArcToClient::SessionOutput {
                session_id: beacon_model::SessionId::new(),
                bytes: b"$ ls\n".to_vec(),
                seq: 0,
                replay: false,
            },
        )
        .await;

    // Give the spawned emitter task time to deliver.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let n = watcher.next_notification().await;
    assert_eq!(n["method"], "arc.mirror.frame", "expected arc.mirror.frame, got: {n:?}");

    // Watcher takes write control; emits arc.mirror.control_grant directly.
    let resp = watcher
        .request(json!({
            "jsonrpc": "2.0", "id": 2,
            "method": "arc.session.control",
            "params": {"session_id": "s-life", "requester_id": "u-bob"}
        }))
        .await;
    assert_eq!(resp["result"]["ok"], json!(true), "control failed: {resp:?}");

    let n = watcher.next_notification().await;
    assert_eq!(
        n["method"],
        "arc.mirror.control_grant",
        "expected arc.mirror.control_grant, got: {n:?}"
    );
    assert_eq!(n["params"]["holder_id"], "u-bob");

    // Watcher releases; emits arc.mirror.control_revoke because holder was set.
    let resp = watcher
        .request(json!({
            "jsonrpc": "2.0", "id": 3,
            "method": "arc.session.release",
            "params": {"session_id": "s-life"}
        }))
        .await;
    assert_eq!(resp["result"]["ok"], json!(true), "release failed: {resp:?}");

    let n = watcher.next_notification().await;
    assert_eq!(
        n["method"],
        "arc.mirror.control_revoke",
        "expected arc.mirror.control_revoke, got: {n:?}"
    );
    assert_eq!(n["params"]["session_id"], "s-life");
}

/// `LeaseChanged { driver: None }` → `arc.mirror.control_revoke`.
#[tokio::test]
async fn mirror_broadcast_lease_changed_none_emits_control_revoke() {
    let (addr, state) = common::spawn_test_server().await;

    let mut ws = common::connect_jsonrpc(&addr).await;
    let attach = json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "arc.session.attach",
        "params": {"session_id": "s-mirror-revoke", "token": "tok-test"}
    });
    let resp = ws.request(attach).await;
    assert_eq!(resp["result"]["ok"], json!(true), "attach failed: {resp:?}");

    let _participant = ws.next_notification().await;

    seed_session(&state, "s-mirror-revoke").await;

    state
        .mirror_registry
        .broadcast(
            "s-mirror-revoke",
            ArcToClient::LeaseChanged {
                session_id: beacon_model::SessionId::new(),
                driver: None,
            },
        )
        .await;

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let notif = ws.next_notification().await;
    assert_eq!(notif["method"], "arc.mirror.control_revoke", "unexpected method: {notif:?}");
    assert_eq!(notif["params"]["session_id"], "s-mirror-revoke");
}
