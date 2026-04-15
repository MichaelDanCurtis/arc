//! Integration test for Task 2.8: SubscribeSession handler with replay + presence.
//!
//! # Requirements
//! - Set DATABASE_URL to a Postgres instance with `arc` user (superuser).
//!   e.g.: `export DATABASE_URL=postgres://arc:arc@localhost:5432/arc`
//! - Tests are marked `#[ignore]` so they skip in CI without a DB.
//!
//! # Sanctioned deviations (Task 2.8)
//! - `host_push_output` is a no-op (Task 2.9 adds ClientToArc::PushOutput).
//! - Test verifies subscribe → PresenceChanged flow rather than replay,
//!   since scrollback is empty without a real host push.
//! - `device_label` is hardcoded as "Watcher" server-side (Task 2.10 refines).

mod common;
use common::*;

use beacon_proto::envelope::ArcToClient;

/// A watcher subscribes to a session and receives a `PresenceChanged` frame
/// confirming they are now listed as a watcher.
#[tokio::test]
#[ignore]
async fn watcher_subscribes_and_receives_presence_changed() {
    let relay = match try_spawn_relay().await {
        Some(r) => r,
        None => {
            eprintln!("Skipping: DATABASE_URL not set or DB unreachable");
            return;
        }
    };

    // Create owner and register a session.
    let alice = create_user(&relay, &format!("alice_{}@test.invalid", uuid::Uuid::new_v4())).await;
    let session_id = host_register_session(&relay, &alice, "Local").await;

    // No host output pushed for Task 2.8 — scrollback is empty.
    // host_push_output is a no-op here; replay assertions are deferred to Task 2.9.

    // Connect a watcher (alice subscribing to her own session — owner permission).
    let mut watcher = connect_watcher(&relay, &alice, "MacBook").await;

    // Send SubscribeSession. session_id is a ULID string from host_register_session.
    let sid: beacon_model::SessionId = session_id.parse().expect("parse session_id as SessionId");
    watcher
        .send(&beacon_proto::envelope::ClientToArc::SubscribeSession {
            session_id: sid,
            since_seq: None,
        })
        .await;

    // The first frame back should be PresenceChanged with the watcher listed.
    let frame = watcher.recv().await;
    assert!(
        matches!(frame, ArcToClient::PresenceChanged { .. }),
        "expected PresenceChanged, got: {frame:?}"
    );

    if let ArcToClient::PresenceChanged { watchers, .. } = frame {
        assert_eq!(watchers.len(), 1, "expected 1 watcher in presence snapshot");
        assert_eq!(
            watchers[0].user_id,
            alice.user_id.to_string(),
            "watcher user_id mismatch"
        );
        assert!(watchers[0].can_drive, "owner should have can_drive = true");
    }
}

/// Two watchers subscribe; each receives a PresenceChanged with both watchers listed.
///
/// Sanctioned deviation: requires two separate user accounts since the relay
/// tracks presence per (user_id, device_label). Alice grants Bob a session
/// grant so Bob can subscribe.
#[tokio::test]
#[ignore]
async fn two_watchers_both_see_presence_changed() {
    let _relay = match try_spawn_relay().await {
        Some(r) => r,
        None => {
            eprintln!("Skipping: DATABASE_URL not set or DB unreachable");
            return;
        }
    };

    // Two-watcher presence assertion is deferred to Task 2.10 when the wire
    // format supports multi-watcher PresenceChanged validation and the grant
    // REST API is available to give Bob subscribe permission.
    //
    // The #[ignore] attribute above keeps this test from running in CI.
    // This todo!() ensures that if someone removes #[ignore] without finishing
    // the implementation, the test will loud-fail immediately rather than
    // passing silently with zero assertions.
    todo!("two-watcher presence assertion deferred to Task 2.10 when wire format supports multi-watcher PresenceChanged validation");
}
