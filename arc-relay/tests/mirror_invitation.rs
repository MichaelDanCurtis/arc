//! Integration tests for [`InvitationStore::check_subscribe_permission`].
//!
//! # Requirements
//! - Set DATABASE_URL to a Postgres with CREATEDB privilege, e.g.:
//!   `export DATABASE_URL=postgres://arc:arc@localhost:5432/arc`
//! - The dev docker-compose stack (`docker compose -f arc-relay/docker-compose.yml up postgres -d`)
//!   gives the `arc` user superuser rights, so CREATEDB works out of the box.
//! - `#[sqlx::test]` creates a fresh per-test database, runs `./migrations`,
//!   passes the pool in, then drops the DB when the test returns.

use arc_relay::mirror::invitation::{InvitationStore, SubscribePermission};
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Test 1: session owner is always permitted
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn owner_always_allowed(pool: PgPool) {
    let store = InvitationStore::new(pool.clone());

    let owner: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, email, password_hash) \
         VALUES (gen_random_uuid(), 'o@b.c', 'x') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO sessions (id, host_id, owner_id, state, started_at, last_seen_at) \
         VALUES ('s1', $1, $1, 'active', now(), now())",
    )
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();

    let result = store
        .check_subscribe_permission("s1", owner, "o@b.c")
        .await
        .unwrap();

    assert!(
        matches!(result, SubscribePermission::Owner),
        "expected Owner, got {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 2: email grant matches case-insensitively
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn email_invitation_matches_case_insensitive(pool: PgPool) {
    let store = InvitationStore::new(pool.clone());

    // Insert owner
    let owner: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, email, password_hash) \
         VALUES (gen_random_uuid(), 'owner2@b.c', 'x') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO sessions (id, host_id, owner_id, state, started_at, last_seen_at) \
         VALUES ('s2', $1, $1, 'active', now(), now())",
    )
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();

    // Insert stranger with lowercase email
    let stranger: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, email, password_hash) \
         VALUES (gen_random_uuid(), 'alice@example.com', 'x') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // Grant by mixed-case email; grantee_user_id must be NULL (one_grant_target)
    sqlx::query(
        "INSERT INTO session_grants \
         (session_id, target_email, can_request_control, can_write) \
         VALUES ('s2', 'Alice@Example.Com', true, false)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let result = store
        .check_subscribe_permission("s2", stranger, "alice@example.com")
        .await
        .unwrap();

    assert!(
        matches!(result, SubscribePermission::Granted { can_drive: true }),
        "expected Granted {{ can_drive: true }}, got {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 3: revoked grant is rejected
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn revoked_invitation_rejects(pool: PgPool) {
    let store = InvitationStore::new(pool.clone());

    let owner: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, email, password_hash) \
         VALUES (gen_random_uuid(), 'owner3@b.c', 'x') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO sessions (id, host_id, owner_id, state, started_at, last_seen_at) \
         VALUES ('s3', $1, $1, 'active', now(), now())",
    )
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();

    let stranger: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, email, password_hash) \
         VALUES (gen_random_uuid(), 'stranger3@b.c', 'x') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // Revoked grant — revoked_at is set so it must not match
    sqlx::query(
        "INSERT INTO session_grants \
         (session_id, grantee_user_id, can_request_control, can_write, revoked_at) \
         VALUES ('s3', $1, true, false, NOW())",
    )
    .bind(stranger)
    .execute(&pool)
    .await
    .unwrap();

    let result = store
        .check_subscribe_permission("s3", stranger, "stranger3@b.c")
        .await
        .unwrap();

    assert!(
        matches!(result, SubscribePermission::Denied),
        "expected Denied for revoked grant, got {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 4: team grant matched via team membership
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn team_invitation_via_membership(pool: PgPool) {
    let store = InvitationStore::new(pool.clone());

    let owner: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, email, password_hash) \
         VALUES (gen_random_uuid(), 'owner4@b.c', 'x') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO sessions (id, host_id, owner_id, state, started_at, last_seen_at) \
         VALUES ('s4', $1, $1, 'active', now(), now())",
    )
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();

    let stranger: Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, email, password_hash) \
         VALUES (gen_random_uuid(), 'stranger4@b.c', 'x') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // Create a team owned by the session owner
    let team_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO teams (id, name, owner_id) VALUES ($1, 'Test Team', $2)",
    )
    .bind(team_id)
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();

    // Make stranger a member of that team
    sqlx::query(
        "INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(team_id)
    .bind(stranger)
    .execute(&pool)
    .await
    .unwrap();

    // Grant via team only — grantee_user_id and target_email must be NULL
    // (one_grant_target CHECK requires exactly one non-null target)
    sqlx::query(
        "INSERT INTO session_grants \
         (session_id, target_team_id, can_request_control, can_write) \
         VALUES ('s4', $1, false, false)",
    )
    .bind(team_id)
    .execute(&pool)
    .await
    .unwrap();

    let result = store
        .check_subscribe_permission("s4", stranger, "stranger4@b.c")
        .await
        .unwrap();

    assert!(
        matches!(result, SubscribePermission::Granted { can_drive: false }),
        "expected Granted {{ can_drive: false }}, got {result:?}"
    );
}
