//! Integration tests for Task 2.11: disconnect handling clears lease + presence.
//!
//! Covers:
//! 1. `driver_disconnect_clears_lease` — the driver watcher disconnects; the
//!    remaining watcher sees `LeaseChanged { driver: None }`.
//! 2. `non_driver_disconnect_updates_presence_only` — a non-driver watcher
//!    disconnects; the remaining watcher sees `PresenceChanged` but NOT
//!    `LeaseChanged`.
//!
//! # Requirements
//! - Set DATABASE_URL to a Postgres instance with migrations applied.
//!   e.g.: `export DATABASE_URL=postgres://arc:arc@localhost:5432/arc`
//! - Tests are marked `#[ignore]` so they skip in CI without a DB.

mod common;
use common::*;

use beacon_model::SessionId;
use beacon_proto::envelope::{ArcToClient, ClientToArc};

// ---------------------------------------------------------------------------
// Helper: insert a session_grant row so a grantee can subscribe
// ---------------------------------------------------------------------------

async fn insert_grant(
    pool: &sqlx::PgPool,
    session_id: &str,
    grantee_id: uuid::Uuid,
    can_request_control: bool,
) {
    sqlx::query(
        "INSERT INTO session_grants \
         (session_id, grantee_user_id, can_request_control) \
         VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(session_id)
    .bind(grantee_id)
    .bind(can_request_control)
    .execute(pool)
    .await
    .expect("insert_grant: DB error");
}

// ---------------------------------------------------------------------------
// Test 1: driver disconnect clears lease
// ---------------------------------------------------------------------------

/// Alice subscribes twice (iPad + Browser).  iPad requests control (instant
/// grant since Alice is the owner).  iPad disconnects (WS close).  Browser
/// should receive `LeaseChanged { driver: None }`.
#[tokio::test]
#[ignore]
async fn driver_disconnect_clears_lease() {
    let relay = match spawn_relay().await {
        Some(r) => r,
        None => {
            eprintln!("Skipping: DATABASE_URL not set or DB unreachable");
            return;
        }
    };

    let alice = create_user(
        &relay,
        &format!("alice_disc1_{}@test.invalid", uuid::Uuid::new_v4()),
    )
    .await;

    let session_id = host_register_session(&relay, &alice, "Local").await;
    let sid: SessionId = session_id.parse().expect("parse session_id");

    // Alice connects as "iPad" watcher.
    let mut ipad = connect_watcher(&relay, &alice, "iPad").await;
    ipad.send(&ClientToArc::SubscribeSession {
        session_id: sid,
        since_seq: None,
    })
    .await;
    drain_until(&mut ipad, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;

    // Alice connects as "Browser" watcher.
    let mut browser = connect_watcher(&relay, &alice, "Browser").await;
    browser
        .send(&ClientToArc::SubscribeSession {
            session_id: sid,
            since_seq: None,
        })
        .await;
    // Both watchers see the updated PresenceChanged (two entries).
    drain_until(&mut ipad, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;
    drain_until(&mut browser, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;

    // iPad requests control → instant grant (Alice is the owner).
    ipad.send(&ClientToArc::RequestControl { session_id: sid })
        .await;

    // Both watchers should see LeaseChanged(Alice).
    drain_until(&mut ipad, |f| {
        matches!(f, ArcToClient::LeaseChanged { driver: Some(_), .. })
    })
    .await;
    drain_until(&mut browser, |f| {
        matches!(f, ArcToClient::LeaseChanged { driver: Some(_), .. })
    })
    .await;

    // iPad disconnects (drop the WS — tungstenite sends a Close frame on drop).
    drop(ipad);

    // Give the relay a moment to process the disconnect.
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Browser must see LeaseChanged { driver: None }.
    drain_until(&mut browser, |f| {
        matches!(f, ArcToClient::LeaseChanged { driver: None, .. })
    })
    .await;
}

// ---------------------------------------------------------------------------
// Test 2: non-driver disconnect updates presence only
// ---------------------------------------------------------------------------

/// Bob (non-driver) disconnects.  Alice (still connected, still driver) should
/// see `PresenceChanged` but NOT `LeaseChanged`.
#[tokio::test]
#[ignore]
async fn non_driver_disconnect_updates_presence_only() {
    let relay = match spawn_relay().await {
        Some(r) => r,
        None => {
            eprintln!("Skipping: DATABASE_URL not set or DB unreachable");
            return;
        }
    };

    let alice = create_user(
        &relay,
        &format!("alice_disc2_{}@test.invalid", uuid::Uuid::new_v4()),
    )
    .await;
    let bob = create_user(
        &relay,
        &format!("bob_disc2_{}@test.invalid", uuid::Uuid::new_v4()),
    )
    .await;

    let session_id = host_register_session(&relay, &alice, "Local").await;
    let sid: SessionId = session_id.parse().expect("parse session_id");

    // Give Bob subscribe permission (no control needed for this test).
    let pool = sqlx::PgPool::connect(
        &std::env::var("DATABASE_URL").expect("DATABASE_URL set"),
    )
    .await
    .expect("pool");
    insert_grant(&pool, &session_id, bob.user_id, false).await;

    // Alice subscribes and grabs the driver seat.
    let mut alice_ws = connect_watcher(&relay, &alice, "MacBook").await;
    alice_ws
        .send(&ClientToArc::SubscribeSession {
            session_id: sid,
            since_seq: None,
        })
        .await;
    drain_until(&mut alice_ws, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;

    alice_ws
        .send(&ClientToArc::RequestControl { session_id: sid })
        .await;
    drain_until(&mut alice_ws, |f| {
        matches!(f, ArcToClient::LeaseChanged { driver: Some(_), .. })
    })
    .await;

    // Bob subscribes.
    let mut bob_ws = connect_watcher(&relay, &bob, "iPad").await;
    bob_ws
        .send(&ClientToArc::SubscribeSession {
            session_id: sid,
            since_seq: None,
        })
        .await;
    drain_until(&mut alice_ws, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;
    drain_until(&mut bob_ws, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;

    // Bob disconnects (he is not the driver).
    drop(bob_ws);

    // Give the relay a moment to process the disconnect.
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Alice must see PresenceChanged (Bob removed).
    drain_until(&mut alice_ws, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;

    // Negative assertion: Alice must NOT receive LeaseChanged within 200 ms,
    // because Bob was not the driver.
    let lease_changed = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        async {
            loop {
                let f = alice_ws.recv().await;
                if matches!(f, ArcToClient::LeaseChanged { .. }) {
                    return f;
                }
            }
        },
    )
    .await;
    assert!(
        lease_changed.is_err(),
        "Alice must not receive LeaseChanged when a non-driver disconnects: {lease_changed:?}"
    );
}
