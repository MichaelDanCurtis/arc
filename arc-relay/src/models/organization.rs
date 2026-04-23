use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub domain: Option<String>,
    pub allow_public: bool,
    pub idp_provider: Option<String>,
    pub idp_config: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OrgMembership {
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub role: String,
}

pub async fn find_org_by_domain(pool: &PgPool, domain: &str) -> Result<Option<Organization>, sqlx::Error> {
    sqlx::query_as::<_, Organization>("SELECT * FROM organizations WHERE domain = $1")
        .bind(domain)
        .fetch_optional(pool)
        .await
}

pub async fn is_org_admin(pool: &PgPool, user_id: Uuid, org_id: Uuid) -> Result<bool, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM org_memberships WHERE user_id = $1 AND org_id = $2",
    )
    .bind(user_id)
    .bind(org_id)
    .fetch_optional(pool)
    .await?;
    Ok(matches!(row.as_ref().map(|r| r.0.as_str()), Some("admin" | "owner")))
}

pub async fn add_org_member(
    pool: &PgPool,
    user_id: Uuid,
    org_id: Uuid,
    role: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO org_memberships (user_id, org_id, role) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, org_id) DO NOTHING",
    )
    .bind(user_id)
    .bind(org_id)
    .bind(role)
    .execute(pool)
    .await?;
    Ok(())
}
