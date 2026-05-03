//! End-to-end WS test for Phase B block stream notifications.
//!
//! Verifies that a subscribed JSON-RPC client receives
//! `arc.mirror.block_start` → `arc.mirror.block_chunk` → `arc.mirror.block_end`
//! when the server-side rpc layer fires the matching `emit_*` helpers.

mod common;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use beacon_model::{BlockFooter, BlockHeader, BlockId, HostInfo, SessionId, TabId};
use serde_json::json;

use arc_relay::rpc::{
    emit_mirror_block_chunk, emit_mirror_block_end, emit_mirror_block_start,
};

#[tokio::test(flavor = "multi_thread")]
async fn block_stream_round_trips_over_ws() {
    let (addr, state) = common::spawn_test_server().await;
    let mut ws = common::connect_jsonrpc(&addr).await;

    // Subscribe so the WS layer attaches us to the session's notif stream.
    let resp = ws
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "arc.session.subscribe",
            "params": { "session_id": "sess-blk", "token": "tok" },
        }))
        .await;
    assert_eq!(resp["result"]["ok"], true, "subscribe failed: {resp}");
    // Drain the participant notif that subscribe emits.
    let _ = ws.next_notification().await;

    // Drive the server-side store directly to fire the block stream.
    let entry = state.arc_session_store.get_or_create("sess-blk").await;

    let block_id = BlockId::new();
    let header = BlockHeader {
        id: block_id,
        session_id: SessionId::new(),
        tab_id: TabId::new(),
        command: "ls".to_string(),
        cwd: PathBuf::from("/tmp"),
        started_at: SystemTime::UNIX_EPOCH,
        env_snapshot: HashMap::new(),
        host_info: HostInfo::current(),
    };
    let footer = BlockFooter {
        id: block_id,
        exit_code: Some(0),
        ended_at: SystemTime::UNIX_EPOCH,
        shadow_git_ref: None,
    };

    emit_mirror_block_start(&entry, &header);
    emit_mirror_block_chunk(&entry, &header.id, 1, b"line1\n");
    emit_mirror_block_end(&entry, &footer);

    // Read three notifications in order.
    let n1 = ws.next_notification().await;
    assert_eq!(n1["method"], "arc.mirror.block_start");
    assert_eq!(n1["params"]["session_id"], "sess-blk");

    let n2 = ws.next_notification().await;
    assert_eq!(n2["method"], "arc.mirror.block_chunk");
    let b64 = n2["params"]["data"].as_str().unwrap();
    assert_eq!(BASE64.decode(b64).unwrap(), b"line1\n");
    assert_eq!(n2["params"]["seq"], 1);

    let n3 = ws.next_notification().await;
    assert_eq!(n3["method"], "arc.mirror.block_end");
    assert_eq!(n3["params"]["footer"]["exit_code"], 0);
}
