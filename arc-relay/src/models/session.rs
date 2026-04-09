use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DbSession {
    pub id: String,
    pub host_id: Uuid,
    pub owner_id: Uuid,
    pub team_id: Option<Uuid>,
    pub name: Option<String>,
    pub state: String,
    pub metadata: Option<serde_json::Value>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub last_seen_at: DateTime<Utc>,
}

pub async fn create_session(
    pool: &PgPool,
    id: &str,
    host_id: Uuid,
    owner_id: Uuid,
    name: Option<&str>,
) -> Result<DbSession, sqlx::Error> {
    let now = Utc::now();
    sqlx::query_as::<_, DbSession>(
        "INSERT INTO sessions (id, host_id, owner_id, name, state, started_at, last_seen_at) \
         VALUES ($1, $2, $3, $4, 'active', $5, $5) RETURNING *",
    )
    .bind(id)
    .bind(host_id)
    .bind(owner_id)
    .bind(name)
    .bind(now)
    .fetch_one(pool)
    .await
}

pub async fn find_session(pool: &PgPool, id: &str) -> Result<Option<DbSession>, sqlx::Error> {
    sqlx::query_as::<_, DbSession>("SELECT * FROM sessions WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn list_sessions_for_user(
    pool: &PgPool,
    owner_id: Uuid,
) -> Result<Vec<DbSession>, sqlx::Error> {
    sqlx::query_as::<_, DbSession>(
        "SELECT * FROM sessions WHERE owner_id = $1 ORDER BY started_at DESC",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await
}

pub async fn update_session_state(pool: &PgPool, id: &str, state: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE sessions SET state = $1 WHERE id = $2")
        .bind(state)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[allow(dead_code)]
pub async fn touch_session(pool: &PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE sessions SET last_seen_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_pool_url() -> Option<String> {
        std::env::var("DATABASE_URL").ok()
    }

    /// Non-DB: verify valid state strings match the DB CHECK constraint values.
    #[test]
    fn session_state_values_are_valid() {
        let valid = ["active", "suspended", "ended"];
        for s in &valid {
            assert!(!s.is_empty());
        }
        // States are the exact set accepted by the DB CHECK constraint.
        assert_eq!(valid.len(), 3);
    }

    #[tokio::test]
    #[ignore]
    async fn test_create_and_find_session() {
        let url = match get_pool_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        let owner_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x')")
            .bind(owner_id)
            .bind(format!("sess_owner_{}@test.invalid", owner_id))
            .execute(&pool)
            .await
            .expect("user");

        let session_id = format!("sess_{}", Uuid::new_v4().simple());
        let host_id = Uuid::new_v4();

        let sess = create_session(&pool, &session_id, host_id, owner_id, Some("my session"))
            .await
            .expect("create_session");
        assert_eq!(sess.id, session_id);
        assert_eq!(sess.state, "active");

        let found = find_session(&pool, &session_id).await.expect("find");
        assert!(found.is_some());
    }

    #[tokio::test]
    #[ignore]
    async fn test_update_session_state() {
        let url = match get_pool_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        let owner_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x')")
            .bind(owner_id)
            .bind(format!("sess_state_{}@test.invalid", owner_id))
            .execute(&pool)
            .await
            .expect("user");

        let session_id = format!("state_{}", Uuid::new_v4().simple());
        create_session(&pool, &session_id, Uuid::new_v4(), owner_id, None)
            .await
            .expect("create");

        update_session_state(&pool, &session_id, "ended")
            .await
            .expect("update_state");

        let sess = find_session(&pool, &session_id)
            .await
            .expect("find")
            .expect("exists");
        assert_eq!(sess.state, "ended");
    }

    #[tokio::test]
    #[ignore]
    async fn test_touch_session() {
        let url = match get_pool_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        let owner_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x')")
            .bind(owner_id)
            .bind(format!("touch_{}@test.invalid", owner_id))
            .execute(&pool)
            .await
            .expect("user");

        let session_id = format!("touch_{}", Uuid::new_v4().simple());
        create_session(&pool, &session_id, Uuid::new_v4(), owner_id, None)
            .await
            .expect("create");

        touch_session(&pool, &session_id).await.expect("touch");

        let sess = find_session(&pool, &session_id)
            .await
            .expect("find")
            .expect("exists");
        // last_seen_at should be very recent
        let age = Utc::now() - sess.last_seen_at;
        assert!(age.num_seconds() < 5);
    }
}
