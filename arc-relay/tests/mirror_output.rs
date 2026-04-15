//! Integration test for Task 2.9: PushOutput → ScrollbackRing + Fanout.
//!
//! # Requirements
//! - Set DATABASE_URL to a Postgres instance with `arc` user (superuser).
//!   e.g.: `export DATABASE_URL=postgres://arc:arc@localhost:5432/arc`
//! - Tests are marked `#[ignore]` so they skip in CI without a DB.

mod common;
use common::*;

use beacon_proto::envelope::ArcToClient;

/// A host pushes output; two subscribed watchers both receive the live
/// `SessionOutput` frame with `replay: false`.
#[tokio::test]
#[ignore]
async fn host_push_output_fans_out_to_all_watchers() {
    let relay = match spawn_relay().await {
        Some(r) => r,
        None => {
            eprintln!("Skipping: DATABASE_URL not set or DB unreachable");
            return;
        }
    };

    let alice = create_user(&relay, &format!("alice_output_{}@test.invalid", uuid::Uuid::new_v4())).await;
    let session_id = host_register_session(&relay, &alice, "Local").await;

    // Connect two watcher sockets.
    let mut w1 = connect_watcher(&relay, &alice, "iPad").await;
    let mut w2 = connect_watcher(&relay, &alice, "Browser").await;

    // Parse session_id once.
    let sid: beacon_model::SessionId = session_id.parse().expect("parse session_id");

    // Both watchers subscribe.
    w1.send(&beacon_proto::envelope::ClientToArc::SubscribeSession {
        session_id: sid,
        since_seq: None,
    })
    .await;
    w2.send(&beacon_proto::envelope::ClientToArc::SubscribeSession {
        session_id: sid,
        since_seq: None,
    })
    .await;

    // Drain initial presence frames so both watchers are confirmed attached.
    drain_until(&mut w1, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;
    drain_until(&mut w2, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;

    // Host pushes output.
    host_push_output(&relay, &alice, &session_id, b"hello").await;

    // Each watcher should receive a live SessionOutput frame.
    let f1 = w1.recv().await;
    let f2 = w2.recv().await;

    assert!(
        matches!(&f1, ArcToClient::SessionOutput { bytes, replay: false, .. } if bytes == b"hello"),
        "watcher 1: expected SessionOutput {{ bytes: b\"hello\", replay: false }}, got: {f1:?}"
    );
    assert!(
        matches!(&f2, ArcToClient::SessionOutput { bytes, replay: false, .. } if bytes == b"hello"),
        "watcher 2: expected SessionOutput {{ bytes: b\"hello\", replay: false }}, got: {f2:?}"
    );
}

/// A watcher that subscribes with `since_seq: Some(0)` after the host has
/// pushed output receives a replay chunk (`replay: true`) covering seq 0.
#[tokio::test]
#[ignore]
async fn subscribe_with_since_seq_replays_scrollback() {
    let relay = match spawn_relay().await {
        Some(r) => r,
        None => {
            eprintln!("Skipping: DATABASE_URL not set or DB unreachable");
            return;
        }
    };

    let alice = create_user(&relay, &format!("alice_replay_{}@test.invalid", uuid::Uuid::new_v4())).await;
    let session_id = host_register_session(&relay, &alice, "Local").await;

    // Push some output to fill scrollback before the watcher subscribes.
    // We need a watcher to be attached first so the mirror is initialised;
    // subscribe as an ephemeral watcher, wait for presence, then close.
    let mut seeder = connect_watcher(&relay, &alice, "Seeder").await;
    let sid: beacon_model::SessionId = session_id.parse().expect("parse session_id");
    seeder
        .send(&beacon_proto::envelope::ClientToArc::SubscribeSession {
            session_id: sid,
            since_seq: None,
        })
        .await;
    drain_until(&mut seeder, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;

    // Host pushes output — this fills seq 0 in the scrollback ring.
    host_push_output(&relay, &alice, &session_id, b"world").await;

    // Drain the live frame from the seeder (seq 0 arrives as replay:false).
    drain_until(&mut seeder, |f| matches!(f, ArcToClient::SessionOutput { .. })).await;

    // Now a late-joining watcher subscribes asking for everything from seq 0.
    let mut late = connect_watcher(&relay, &alice, "LateJoiner").await;
    late.send(&beacon_proto::envelope::ClientToArc::SubscribeSession {
        session_id: sid,
        since_seq: Some(0),
    })
    .await;

    // The first frame should be the replay chunk (replay: true, seq: 0).
    let replay_frame = late.recv().await;
    assert!(
        matches!(&replay_frame, ArcToClient::SessionOutput { bytes, seq: 0, replay: true, .. } if bytes == b"world"),
        "expected replay SessionOutput {{ seq: 0, replay: true, bytes: b\"world\" }}, got: {replay_frame:?}"
    );
}
