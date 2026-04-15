/// Integration test: migration 002 applies cleanly and the `one_grant_target`
/// CHECK constraint enforces exactly one of (grantee_user_id, target_email,
/// target_team_id) on session_grants.
///
/// # Requirements
/// - Dev-stack Postgres must be running at postgres://arc:arc@localhost:5432/arc
/// - If the DB is unavailable the test skips gracefully (no CI failure).
///
/// # What is tested
/// 1. Insert with exactly ONE target set succeeds.
/// 2. Insert with ZERO targets fails with a CHECK violation.
/// 3. Insert with TWO targets set fails with a CHECK violation.
///
/// All inserted rows are cleaned up (DELETE after assertions).
///
/// Note: we use `sqlx::query` (runtime-only, no `!` macro) so that
/// `cargo check` passes without a live DATABASE_URL at compile time.
use sqlx::PgPool;
use uuid::Uuid;

const DB_URL: &str = "postgres://arc:arc@localhost:5432/arc";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns true when the sqlx error is a Postgres CHECK-constraint violation
/// (SQLSTATE 23514).
fn is_check_violation(e: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = e {
        // Postgres SQLSTATE 23514 = check_violation
        return db_err.code().as_deref() == Some("23514");
    }
    false
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_002_schema_and_check_constraint() {
    // -----------------------------------------------------------------------
    // Connect — skip gracefully if the dev DB is not running.
    // -----------------------------------------------------------------------
    let pool = match PgPool::connect(DB_URL).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "[migration_002] SKIP — could not connect to dev DB ({}): {e}",
                DB_URL
            );
            return; // test passes (skipped)
        }
    };

    // -----------------------------------------------------------------------
    // Pre-test cleanup — guarantees re-runs start clean even if a prior
    // run panicked mid-test. Must run in FK order: grants → sessions → users.
    // -----------------------------------------------------------------------
    sqlx::query("DELETE FROM session_grants WHERE session_id LIKE 'test-mig002-%'")
        .execute(&pool).await.ok();
    sqlx::query("DELETE FROM sessions WHERE id LIKE 'test-mig002-%'")
        .execute(&pool).await.ok();
    sqlx::query("DELETE FROM users WHERE email LIKE 'test-%@example.com' AND email SIMILAR TO 'test-[0-9a-f]%'")
        .execute(&pool).await.ok();

    // -----------------------------------------------------------------------
    // Apply all migrations (idempotent via sqlx versioned migrations).
    // -----------------------------------------------------------------------
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrations should apply cleanly");

    // -----------------------------------------------------------------------
    // Seed: one user + one session so FK checks pass.
    // We use fresh UUIDs / ULIDs to avoid collisions with existing data.
    // -----------------------------------------------------------------------
    let user_id = Uuid::new_v4();
    let session_id = format!("test-mig002-{}", user_id.simple());
    let email = format!("test-{}@example.com", user_id.simple());

    sqlx::query(
        "INSERT INTO users (id, email, name, password_hash) \
         VALUES ($1, $2, 'Test User', 'x')",
    )
    .bind(user_id)
    .bind(&email)
    .execute(&pool)
    .await
    .expect("insert seed user");

    sqlx::query(
        "INSERT INTO sessions \
         (id, host_id, owner_id, state, started_at, last_seen_at) \
         VALUES ($1, $2, $2, 'active', NOW(), NOW())",
    )
    .bind(&session_id)
    .bind(user_id)
    .execute(&pool)
    .await
    .expect("insert seed session");

    // -----------------------------------------------------------------------
    // Assertion 1: exactly ONE target (grantee_user_id) → should succeed.
    // -----------------------------------------------------------------------
    let r1 = sqlx::query(
        "INSERT INTO session_grants \
         (session_id, grantee_user_id, can_write) \
         VALUES ($1, $2, false)",
    )
    .bind(&session_id)
    .bind(user_id)
    .execute(&pool)
    .await;

    assert!(
        r1.is_ok(),
        "Insert with one target (grantee_user_id) should succeed; got: {:?}",
        r1
    );

    // -----------------------------------------------------------------------
    // Assertion 2: ZERO targets → should fail with CHECK violation.
    // -----------------------------------------------------------------------
    let r2 = sqlx::query(
        "INSERT INTO session_grants \
         (session_id, can_write) \
         VALUES ($1, false)",
    )
    .bind(&session_id)
    .execute(&pool)
    .await;

    assert!(
        r2.is_err(),
        "Insert with zero targets should fail (one_grant_target CHECK)"
    );
    assert!(
        is_check_violation(r2.as_ref().unwrap_err()),
        "Expected CHECK violation (SQLSTATE 23514) for zero targets; got: {:?}",
        r2
    );

    // -----------------------------------------------------------------------
    // Assertion 3: TWO targets (grantee_user_id + target_email) → should fail.
    // -----------------------------------------------------------------------
    let r3 = sqlx::query(
        "INSERT INTO session_grants \
         (session_id, grantee_user_id, target_email, can_write) \
         VALUES ($1, $2, 'other@example.com', false)",
    )
    .bind(&session_id)
    .bind(user_id)
    .execute(&pool)
    .await;

    assert!(
        r3.is_err(),
        "Insert with two targets should fail (one_grant_target CHECK)"
    );
    assert!(
        is_check_violation(r3.as_ref().unwrap_err()),
        "Expected CHECK violation (SQLSTATE 23514) for two targets; got: {:?}",
        r3
    );

    // -----------------------------------------------------------------------
    // Cleanup: remove seed data (reverse FK order).
    // -----------------------------------------------------------------------
    sqlx::query("DELETE FROM session_grants WHERE session_id = $1")
        .bind(&session_id)
        .execute(&pool)
        .await
        .expect("cleanup session_grants");

    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(&session_id)
        .execute(&pool)
        .await
        .expect("cleanup sessions");

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("cleanup users");

    pool.close().await;
    println!("[migration_002] all 3 assertions passed");
}
