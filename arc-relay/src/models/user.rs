use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub password_hash: Option<String>,
    pub avatar_url: Option<String>,
    pub oauth_provider: Option<String>,
    pub oauth_provider_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub async fn create_user(
    pool: &PgPool,
    email: &str,
    password_hash: &str,
    name: Option<&str>,
) -> Result<User, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query_as::<_, User>(
        "INSERT INTO users (id, email, password_hash, name) VALUES ($1, $2, $3, $4) RETURNING *",
    )
    .bind(id)
    .bind(email)
    .bind(password_hash)
    .bind(name)
    .fetch_one(pool)
    .await
}

pub async fn find_user_by_email(pool: &PgPool, email: &str) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE email = $1")
        .bind(email)
        .fetch_optional(pool)
        .await
}

pub async fn find_user_by_id(pool: &PgPool, id: Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn upsert_oauth_user(
    pool: &PgPool,
    email: &str,
    name: Option<&str>,
    avatar_url: Option<&str>,
    oauth_provider: &str,
    oauth_provider_id: &str,
) -> Result<User, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query_as::<_, User>(
        "INSERT INTO users (id, email, name, avatar_url, oauth_provider, oauth_provider_id) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (email) DO UPDATE SET \
             name = COALESCE(EXCLUDED.name, users.name), \
             avatar_url = COALESCE(EXCLUDED.avatar_url, users.avatar_url), \
             oauth_provider = EXCLUDED.oauth_provider, \
             oauth_provider_id = EXCLUDED.oauth_provider_id \
         RETURNING *",
    )
    .bind(id)
    .bind(email)
    .bind(name)
    .bind(avatar_url)
    .bind(oauth_provider)
    .bind(oauth_provider_id)
    .fetch_one(pool)
    .await
}

pub async fn get_signing_key(pool: &PgPool, user_id: Uuid) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT signing_public_key FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(k,)| k))
}

pub async fn set_signing_key(pool: &PgPool, user_id: Uuid, public_key_pem: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET signing_public_key = $2 WHERE id = $1")
        .bind(user_id)
        .bind(public_key_pem)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn is_admin(pool: &PgPool, user_id: Uuid) -> Result<bool, sqlx::Error> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 AS n FROM org_memberships WHERE user_id = $1 AND role = 'admin' LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_pool_url() -> Option<String> {
        std::env::var("DATABASE_URL").ok()
    }

    #[tokio::test]
    #[ignore]
    async fn test_create_user() {
        let url = match get_pool_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        let user = create_user(&pool, "test@example.com", "hashed_pw", Some("Test User"))
            .await
            .expect("create_user");

        assert_eq!(user.email, "test@example.com");
        assert_eq!(user.name.as_deref(), Some("Test User"));
    }

    #[tokio::test]
    #[ignore]
    async fn test_find_user_by_email() {
        let url = match get_pool_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        let email = format!("findbyemail_{}@example.com", Uuid::new_v4());
        create_user(&pool, &email, "pw_hash", None)
            .await
            .expect("create");

        let found = find_user_by_email(&pool, &email)
            .await
            .expect("find_by_email");
        assert!(found.is_some());
        assert_eq!(found.unwrap().email, email);
    }

    #[tokio::test]
    #[ignore]
    async fn test_find_user_by_email_missing() {
        let url = match get_pool_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        let found = find_user_by_email(&pool, "nobody@nowhere.invalid")
            .await
            .expect("find_by_email");
        assert!(found.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_find_user_by_id() {
        let url = match get_pool_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        let email = format!("findbyid_{}@example.com", Uuid::new_v4());
        let created = create_user(&pool, &email, "pw_hash", None)
            .await
            .expect("create");

        let found = find_user_by_id(&pool, created.id)
            .await
            .expect("find_by_id");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, created.id);
    }
}
