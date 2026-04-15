//! Integration tests for Task 2.10: control-flow handlers.
//!
//! Covers the four scenarios from plan lines 1757-1761:
//! 1. Owner-instant grab → LeaseChanged broadcast.
//! 2. Non-owner request → ControlRequested fans to owner only → approve → LeaseChanged.
//! 3. Owner reclaim while non-owner is driving → instant LeaseChanged.
//! 4. Implicit reclaim: owner sends SessionInput while not driver → reclaim happens.
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
// Helper: insert a session_grant row so Bob can subscribe to Alice's session
// ---------------------------------------------------------------------------

/// Insert a direct user grant into `session_grants`, giving `grantee_id`
/// permission to subscribe to `session_id` and optionally request control.
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
// Scenario 1: Owner-instant grab → LeaseChanged broadcast
// ---------------------------------------------------------------------------

/// Owner sends RequestControl; gets an instant lease grant and sees LeaseChanged.
#[tokio::test]
#[ignore]
async fn owner_request_control_instant_grant() {
    let relay = match spawn_relay().await {
        Some(r) => r,
        None => {
            eprintln!("Skipping: DATABASE_URL not set or DB unreachable");
            return;
        }
    };

    let alice = create_user(
        &relay,
        &format!("alice_ctrl1_{}@test.invalid", uuid::Uuid::new_v4()),
    )
    .await;
    let session_id = host_register_session(&relay, &alice, "Local").await;
    let sid: SessionId = session_id.parse().expect("parse session_id");

    // Alice subscribes as a watcher.
    let mut watcher = connect_watcher(&relay, &alice, "MacBook").await;
    watcher
        .send(&ClientToArc::SubscribeSession {
            session_id: sid,
            since_seq: None,
        })
        .await;

    // Drain the initial PresenceChanged frame.
    drain_until(&mut watcher, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;

    // Owner sends RequestControl.
    watcher
        .send(&ClientToArc::RequestControl { session_id: sid })
        .await;

    // Should receive LeaseChanged with driver set.
    drain_until(&mut watcher, |f| {
        matches!(f, ArcToClient::LeaseChanged { driver: Some(_), .. })
    })
    .await;
}

// ---------------------------------------------------------------------------
// Scenario 2: Non-owner request → ControlRequested fans to owner only
//             → owner approves → LeaseChanged
// ---------------------------------------------------------------------------

/// Bob requests control; Alice (owner) receives ControlRequested; Alice approves;
/// both see LeaseChanged with Bob as driver.
#[tokio::test]
#[ignore]
async fn non_owner_request_approved_by_owner() {
    let relay = match spawn_relay().await {
        Some(r) => r,
        None => {
            eprintln!("Skipping: DATABASE_URL not set or DB unreachable");
            return;
        }
    };

    let alice = create_user(
        &relay,
        &format!("alice_ctrl2_{}@test.invalid", uuid::Uuid::new_v4()),
    )
    .await;
    let bob = create_user(
        &relay,
        &format!("bob_ctrl2_{}@test.invalid", uuid::Uuid::new_v4()),
    )
    .await;

    let session_id = host_register_session(&relay, &alice, "Local").await;
    let sid: SessionId = session_id.parse().expect("parse session_id");

    // Give Bob subscribe + request-control permission.
    let pool = sqlx::PgPool::connect(
        &std::env::var("DATABASE_URL").expect("DATABASE_URL set"),
    )
    .await
    .expect("pool");
    insert_grant(&pool, &session_id, bob.user_id, true).await;

    // Alice subscribes (owner watcher).
    let mut alice_ws = connect_watcher(&relay, &alice, "MacBook").await;
    alice_ws
        .send(&ClientToArc::SubscribeSession {
            session_id: sid,
            since_seq: None,
        })
        .await;
    drain_until(&mut alice_ws, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;

    // Bob subscribes.
    let mut bob_ws = connect_watcher(&relay, &bob, "iPad").await;
    bob_ws
        .send(&ClientToArc::SubscribeSession {
            session_id: sid,
            since_seq: None,
        })
        .await;
    // Alice sees updated PresenceChanged (two watchers now).
    drain_until(&mut alice_ws, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;
    // Bob sees his own PresenceChanged.
    drain_until(&mut bob_ws, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;

    // Bob requests control.
    bob_ws
        .send(&ClientToArc::RequestControl { session_id: sid })
        .await;

    // Alice (owner) should receive ControlRequested; Bob should NOT receive it.
    let ctrl_req = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        async { loop {
            let f = alice_ws.recv().await;
            if matches!(f, ArcToClient::ControlRequested { .. }) {
                return f;
            }
        }},
    )
    .await
    .expect("Alice should receive ControlRequested within 5s");

    let request_id = match ctrl_req {
        ArcToClient::ControlRequested { request_id, from_user, .. } => {
            assert_eq!(from_user, bob.user_id.to_string(), "from_user should be Bob");
            request_id
        }
        other => panic!("expected ControlRequested, got: {other:?}"),
    };

    // Negative assertion: Bob (non-owner) must NOT receive ControlRequested.
    // broadcast_to_owner should have filtered his sink out.  We give a short
    // budget — by the time Alice has received + processed her frame, any
    // mis-routed frame to Bob would already be queued.
    let bob_leak = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        async { loop {
            let f = bob_ws.recv().await;
            if matches!(f, ArcToClient::ControlRequested { .. }) {
                return f;
            }
        }},
    )
    .await;
    assert!(
        bob_leak.is_err(),
        "Bob (non-owner) must not receive ControlRequested — broadcast_to_owner leak: {bob_leak:?}"
    );

    // Alice approves.
    alice_ws
        .send(&ClientToArc::ApproveControl {
            session_id: sid,
            request_id: request_id.clone(),
        })
        .await;

    // Both Alice and Bob should now receive LeaseChanged with Bob as driver.
    drain_until(&mut alice_ws, |f| {
        matches!(f, ArcToClient::LeaseChanged { driver: Some(d), .. } if d.user_id == bob.user_id.to_string())
    })
    .await;
    drain_until(&mut bob_ws, |f| {
        matches!(f, ArcToClient::LeaseChanged { driver: Some(d), .. } if d.user_id == bob.user_id.to_string())
    })
    .await;
}

// ---------------------------------------------------------------------------
// Scenario 3: Owner reclaim while non-owner is driving → instant LeaseChanged
// ---------------------------------------------------------------------------

/// Bob is the current driver; Alice sends RevokeControl; both see
/// LeaseChanged { driver: None }.
#[tokio::test]
#[ignore]
async fn owner_revokes_non_owner_driver() {
    let relay = match spawn_relay().await {
        Some(r) => r,
        None => {
            eprintln!("Skipping: DATABASE_URL not set or DB unreachable");
            return;
        }
    };

    let alice = create_user(
        &relay,
        &format!("alice_ctrl3_{}@test.invalid", uuid::Uuid::new_v4()),
    )
    .await;
    let bob = create_user(
        &relay,
        &format!("bob_ctrl3_{}@test.invalid", uuid::Uuid::new_v4()),
    )
    .await;

    let session_id = host_register_session(&relay, &alice, "Local").await;
    let sid: SessionId = session_id.parse().expect("parse session_id");

    let pool = sqlx::PgPool::connect(
        &std::env::var("DATABASE_URL").expect("DATABASE_URL set"),
    )
    .await
    .expect("pool");
    insert_grant(&pool, &session_id, bob.user_id, true).await;

    // Alice and Bob both subscribe.
    let mut alice_ws = connect_watcher(&relay, &alice, "MacBook").await;
    alice_ws
        .send(&ClientToArc::SubscribeSession { session_id: sid, since_seq: None })
        .await;
    drain_until(&mut alice_ws, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;

    let mut bob_ws = connect_watcher(&relay, &bob, "iPad").await;
    bob_ws
        .send(&ClientToArc::SubscribeSession { session_id: sid, since_seq: None })
        .await;
    drain_until(&mut alice_ws, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;
    drain_until(&mut bob_ws, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;

    // Bob requests control → Alice approves → Bob becomes driver.
    bob_ws
        .send(&ClientToArc::RequestControl { session_id: sid })
        .await;
    let ctrl_req = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        async { loop {
            let f = alice_ws.recv().await;
            if matches!(f, ArcToClient::ControlRequested { .. }) { return f; }
        }},
    )
    .await
    .expect("Alice should receive ControlRequested");

    let request_id = match ctrl_req {
        ArcToClient::ControlRequested { request_id, .. } => request_id,
        other => panic!("expected ControlRequested, got: {other:?}"),
    };
    alice_ws
        .send(&ClientToArc::ApproveControl { session_id: sid, request_id })
        .await;

    // Wait for LeaseChanged(Bob) on both sides.
    drain_until(&mut alice_ws, |f| {
        matches!(f, ArcToClient::LeaseChanged { driver: Some(d), .. } if d.user_id == bob.user_id.to_string())
    })
    .await;
    drain_until(&mut bob_ws, |f| {
        matches!(f, ArcToClient::LeaseChanged { driver: Some(d), .. } if d.user_id == bob.user_id.to_string())
    })
    .await;

    // Alice revokes.
    alice_ws
        .send(&ClientToArc::RevokeControl { session_id: sid })
        .await;

    // Both watchers should see LeaseChanged { driver: None }.
    drain_until(&mut alice_ws, |f| {
        matches!(f, ArcToClient::LeaseChanged { driver: None, .. })
    })
    .await;
    drain_until(&mut bob_ws, |f| {
        matches!(f, ArcToClient::LeaseChanged { driver: None, .. })
    })
    .await;
}

// ---------------------------------------------------------------------------
// Scenario 4: Implicit reclaim — owner sends SessionInput while not driver
// ---------------------------------------------------------------------------

/// Alice (owner) sends SessionInput without first requesting control.
/// The relay should implicitly reclaim the driver seat and broadcast LeaseChanged.
/// The input bytes are dropped (host_input_tx may not be plumbed via the
/// subscribe path in this test) but the lease transition must happen.
#[tokio::test]
#[ignore]
async fn owner_implicit_reclaim_on_session_input() {
    let relay = match spawn_relay().await {
        Some(r) => r,
        None => {
            eprintln!("Skipping: DATABASE_URL not set or DB unreachable");
            return;
        }
    };

    let alice = create_user(
        &relay,
        &format!("alice_ctrl4_{}@test.invalid", uuid::Uuid::new_v4()),
    )
    .await;
    let session_id = host_register_session(&relay, &alice, "Local").await;
    let sid: SessionId = session_id.parse().expect("parse session_id");

    // Alice subscribes (she is the owner).
    let mut alice_ws = connect_watcher(&relay, &alice, "MacBook").await;
    alice_ws
        .send(&ClientToArc::SubscribeSession { session_id: sid, since_seq: None })
        .await;
    drain_until(&mut alice_ws, |f| matches!(f, ArcToClient::PresenceChanged { .. })).await;

    // Alice sends SessionInput without any prior RequestControl.
    alice_ws
        .send(&ClientToArc::SessionInput {
            session_id: sid,
            bytes: b"hello".to_vec(),
        })
        .await;

    // The relay should broadcast LeaseChanged(Alice) due to implicit reclaim.
    drain_until(&mut alice_ws, |f| {
        matches!(f, ArcToClient::LeaseChanged { driver: Some(d), .. } if d.user_id == alice.user_id.to_string())
    })
    .await;
}
