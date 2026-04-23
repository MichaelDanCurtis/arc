use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MarketplacePlugin {
    pub id: Uuid,
    pub author_id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub icon_url: Option<String>,
    pub homepage_url: Option<String>,
    pub repository_url: Option<String>,
    pub license: Option<String>,
    pub status: String,
    pub price_cents: i32,
    pub payment_type: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MarketplaceVersion {
    pub id: Uuid,
    pub plugin_id: Uuid,
    pub version: String,
    pub release_notes: Option<String>,
    pub archive_url: String,
    pub archive_sha256: String,
    pub signature: Option<String>,
    pub countersig: Option<String>,
    pub platform: String,
    pub size_bytes: Option<i64>,
    pub download_count: i64,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct MarketplaceQuery {
    pub search: Option<String>,
    pub category: Option<String>,
    pub payment_type: Option<String>,
    pub platform: Option<String>,
    pub sort: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PluginListItem {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub icon_url: Option<String>,
    pub license: Option<String>,
    pub payment_type: String,
    pub price_cents: i32,
    pub author_name: Option<String>,
    pub download_count: i64,
    pub latest_version: Option<String>,
    pub updated_at: DateTime<Utc>,
}

pub async fn list_approved_plugins(
    pool: &PgPool,
    query: &MarketplaceQuery,
) -> Result<Vec<PluginListItem>, sqlx::Error> {
    let sort_clause = match query.sort.as_deref() {
        Some("recent") => "mp.updated_at DESC",
        Some("name") => "mp.name ASC",
        _ => "COALESCE(dl.total_downloads, 0) DESC",
    };

    let limit = query.limit.unwrap_or(50).min(100);
    let offset = query.offset.unwrap_or(0);

    let sql = format!(
        "SELECT \
            mp.id, mp.name, mp.slug, mp.description, mp.category, \
            mp.icon_url, mp.license, mp.payment_type, mp.price_cents, \
            u.name AS author_name, \
            COALESCE(dl.total_downloads, 0) AS download_count, \
            lv.version AS latest_version, \
            mp.updated_at \
        FROM marketplace_plugins mp \
        JOIN users u ON u.id = mp.author_id \
        LEFT JOIN LATERAL ( \
            SELECT SUM(download_count) AS total_downloads \
            FROM marketplace_versions WHERE plugin_id = mp.id \
        ) dl ON true \
        LEFT JOIN LATERAL ( \
            SELECT version FROM marketplace_versions \
            WHERE plugin_id = mp.id AND status = 'approved' \
            ORDER BY created_at DESC LIMIT 1 \
        ) lv ON true \
        WHERE mp.status = 'approved' \
            AND ($1::text IS NULL OR mp.category = $1) \
            AND ($2::text IS NULL OR mp.payment_type = $2) \
            AND ($3::text IS NULL OR EXISTS ( \
                SELECT 1 FROM marketplace_versions mv \
                WHERE mv.plugin_id = mp.id AND mv.platform = $3 \
            )) \
            AND ($4::text IS NULL OR to_tsvector('english', mp.name || ' ' || COALESCE(mp.description, '')) @@ plainto_tsquery('english', $4)) \
        ORDER BY {sort_clause} \
        LIMIT $5 OFFSET $6"
    );

    sqlx::query_as::<_, PluginListItem>(&sql)
        .bind(query.category.as_deref())
        .bind(query.payment_type.as_deref())
        .bind(query.platform.as_deref())
        .bind(query.search.as_deref())
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
}

pub async fn get_plugin_by_slug(
    pool: &PgPool,
    slug: &str,
) -> Result<Option<MarketplacePlugin>, sqlx::Error> {
    sqlx::query_as::<_, MarketplacePlugin>(
        "SELECT * FROM marketplace_plugins WHERE slug = $1 AND status = 'approved'",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await
}

pub async fn get_plugin_versions(
    pool: &PgPool,
    plugin_id: Uuid,
) -> Result<Vec<MarketplaceVersion>, sqlx::Error> {
    sqlx::query_as::<_, MarketplaceVersion>(
        "SELECT * FROM marketplace_versions \
         WHERE plugin_id = $1 AND status = 'approved' \
         ORDER BY created_at DESC",
    )
    .bind(plugin_id)
    .fetch_all(pool)
    .await
}

pub async fn list_categories(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT category FROM marketplace_plugins \
         WHERE status = 'approved' AND category IS NOT NULL \
         ORDER BY category",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(c,)| c).collect())
}

// ---------------------------------------------------------------------------
// CRUD request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreatePluginRequest {
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub homepage_url: Option<String>,
    pub repository_url: Option<String>,
    pub license: Option<String>,
    pub payment_type: Option<String>,
    pub price_cents: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePluginRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub homepage_url: Option<String>,
    pub repository_url: Option<String>,
    pub license: Option<String>,
}

// ---------------------------------------------------------------------------
// CRUD + status query functions
// ---------------------------------------------------------------------------

pub async fn create_plugin(
    pool: &PgPool,
    author_id: Uuid,
    req: &CreatePluginRequest,
) -> Result<MarketplacePlugin, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query_as::<_, MarketplacePlugin>(
        "INSERT INTO marketplace_plugins \
         (id, author_id, name, slug, description, category, homepage_url, repository_url, \
          license, status, payment_type, price_cents) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'draft', \
                 COALESCE($10, 'free'), COALESCE($11, 0)) \
         RETURNING *",
    )
    .bind(id)
    .bind(author_id)
    .bind(&req.name)
    .bind(&req.slug)
    .bind(req.description.as_deref())
    .bind(req.category.as_deref())
    .bind(req.homepage_url.as_deref())
    .bind(req.repository_url.as_deref())
    .bind(req.license.as_deref())
    .bind(req.payment_type.as_deref())
    .bind(req.price_cents)
    .fetch_one(pool)
    .await
}

pub async fn update_plugin(
    pool: &PgPool,
    plugin_id: Uuid,
    author_id: Uuid,
    req: &UpdatePluginRequest,
) -> Result<Option<MarketplacePlugin>, sqlx::Error> {
    sqlx::query_as::<_, MarketplacePlugin>(
        "UPDATE marketplace_plugins SET \
         name = COALESCE($3, name), \
         description = COALESCE($4, description), \
         category = COALESCE($5, category), \
         homepage_url = COALESCE($6, homepage_url), \
         repository_url = COALESCE($7, repository_url), \
         license = COALESCE($8, license), \
         updated_at = NOW() \
         WHERE id = $1 AND author_id = $2 \
         RETURNING *",
    )
    .bind(plugin_id)
    .bind(author_id)
    .bind(req.name.as_deref())
    .bind(req.description.as_deref())
    .bind(req.category.as_deref())
    .bind(req.homepage_url.as_deref())
    .bind(req.repository_url.as_deref())
    .bind(req.license.as_deref())
    .fetch_optional(pool)
    .await
}

pub async fn delete_plugin(
    pool: &PgPool,
    plugin_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM marketplace_plugins WHERE id = $1 AND author_id = $2",
    )
    .bind(plugin_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn create_version(
    pool: &PgPool,
    plugin_id: Uuid,
    version: &str,
    platform: &str,
    archive_url: &str,
    archive_sha256: &str,
    size_bytes: Option<i64>,
    signature: Option<&str>,
) -> Result<MarketplaceVersion, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query_as::<_, MarketplaceVersion>(
        "INSERT INTO marketplace_versions \
         (id, plugin_id, version, platform, archive_url, archive_sha256, size_bytes, signature, status) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'pending_review') \
         RETURNING *",
    )
    .bind(id)
    .bind(plugin_id)
    .bind(version)
    .bind(platform)
    .bind(archive_url)
    .bind(archive_sha256)
    .bind(size_bytes)
    .bind(signature)
    .fetch_one(pool)
    .await
}

pub async fn get_plugin_by_id(
    pool: &PgPool,
    plugin_id: Uuid,
) -> Result<Option<MarketplacePlugin>, sqlx::Error> {
    sqlx::query_as::<_, MarketplacePlugin>(
        "SELECT * FROM marketplace_plugins WHERE id = $1",
    )
    .bind(plugin_id)
    .fetch_optional(pool)
    .await
}

pub async fn list_plugins_by_author(
    pool: &PgPool,
    author_id: Uuid,
) -> Result<Vec<MarketplacePlugin>, sqlx::Error> {
    sqlx::query_as::<_, MarketplacePlugin>(
        "SELECT * FROM marketplace_plugins WHERE author_id = $1 ORDER BY updated_at DESC",
    )
    .bind(author_id)
    .fetch_all(pool)
    .await
}

pub async fn set_plugin_pending_if_draft(
    pool: &PgPool,
    plugin_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE marketplace_plugins SET status = 'pending_review' \
         WHERE id = $1 AND status = 'draft'",
    )
    .bind(plugin_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_pending_review(pool: &PgPool) -> Result<Vec<MarketplaceVersion>, sqlx::Error> {
    sqlx::query_as::<_, MarketplaceVersion>(
        "SELECT * FROM marketplace_versions WHERE status = 'pending_review' ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await
}

pub async fn set_version_status(
    pool: &PgPool,
    version_id: Uuid,
    status: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE marketplace_versions SET status = $2 WHERE id = $1",
    )
    .bind(version_id)
    .bind(status)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn approve_plugin_if_first(
    pool: &PgPool,
    plugin_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE marketplace_plugins SET status = 'approved' \
         WHERE id = $1 AND status != 'approved'",
    )
    .bind(plugin_id)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Org allowlist types and functions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OrgAllowlistEntry {
    pub org_id: Uuid,
    pub plugin_id: Uuid,
    pub added_by: Uuid,
    pub added_at: DateTime<Utc>,
}

pub async fn get_org_allowlist(
    pool: &PgPool,
    org_id: Uuid,
) -> Result<Vec<PluginListItem>, sqlx::Error> {
    sqlx::query_as::<_, PluginListItem>(
        "SELECT \
            mp.id, mp.name, mp.slug, mp.description, mp.category, \
            mp.icon_url, mp.license, mp.payment_type, mp.price_cents, \
            u.name AS author_name, \
            COALESCE(dl.total_downloads, 0) AS download_count, \
            lv.version AS latest_version, \
            mp.updated_at \
        FROM org_plugin_allowlist opa \
        JOIN marketplace_plugins mp ON mp.id = opa.plugin_id \
        JOIN users u ON u.id = mp.author_id \
        LEFT JOIN LATERAL ( \
            SELECT SUM(download_count) AS total_downloads \
            FROM marketplace_versions WHERE plugin_id = mp.id \
        ) dl ON true \
        LEFT JOIN LATERAL ( \
            SELECT version FROM marketplace_versions \
            WHERE plugin_id = mp.id AND status = 'approved' \
            ORDER BY created_at DESC LIMIT 1 \
        ) lv ON true \
        WHERE opa.org_id = $1 \
        ORDER BY mp.updated_at DESC",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await
}

pub async fn add_to_org_allowlist(
    pool: &PgPool,
    org_id: Uuid,
    plugin_id: Uuid,
    added_by: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO org_plugin_allowlist (org_id, plugin_id, added_by) \
         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(org_id)
    .bind(plugin_id)
    .bind(added_by)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove_from_org_allowlist(
    pool: &PgPool,
    org_id: Uuid,
    plugin_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM org_plugin_allowlist WHERE org_id = $1 AND plugin_id = $2",
    )
    .bind(org_id)
    .bind(plugin_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Install to team
// ---------------------------------------------------------------------------

pub async fn install_to_team(
    pool: &PgPool,
    plugin_id: Uuid,
    version: &str,
    team_id: Uuid,
    installed_by: Uuid,
) -> Result<(), sqlx::Error> {
    // Look up plugin (approved only)
    let plugin = sqlx::query_as::<_, MarketplacePlugin>(
        "SELECT * FROM marketplace_plugins WHERE id = $1 AND status = 'approved'",
    )
    .bind(plugin_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| sqlx::Error::RowNotFound)?;

    // Look up version (approved only)
    let ver = sqlx::query_as::<_, MarketplaceVersion>(
        "SELECT * FROM marketplace_versions \
         WHERE plugin_id = $1 AND version = $2 AND status = 'approved'",
    )
    .bind(plugin_id)
    .bind(version)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| sqlx::Error::RowNotFound)?;

    // Build manifest JSON
    let manifest = serde_json::json!({
        "plugin_id": plugin.id,
        "slug": plugin.slug,
        "name": plugin.name,
        "version": ver.version,
        "platform": ver.platform,
        "archive_url": ver.archive_url,
        "archive_sha256": ver.archive_sha256,
        "payment_type": plugin.payment_type,
    });

    // INSERT INTO plugin_registry ON CONFLICT UPDATE
    sqlx::query(
        "INSERT INTO plugin_registry \
         (id, team_id, plugin_id, version, manifest, installed_by) \
         VALUES (gen_random_uuid(), $1, $2, $3, $4, $5) \
         ON CONFLICT (team_id, plugin_id) DO UPDATE SET \
             version = EXCLUDED.version, \
             manifest = EXCLUDED.manifest, \
             installed_by = EXCLUDED.installed_by, \
             updated_at = NOW()",
    )
    .bind(team_id)
    .bind(plugin_id)
    .bind(&ver.version)
    .bind(manifest)
    .bind(installed_by)
    .execute(pool)
    .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_plugin_request_deserializes_minimal() {
        let json = r#"{"name":"Test","slug":"test"}"#;
        let req: CreatePluginRequest = serde_json::from_str(json).expect("deserialize");
        assert_eq!(req.name, "Test");
        assert_eq!(req.slug, "test");
        assert!(req.description.is_none());
        assert!(req.category.is_none());
        assert!(req.payment_type.is_none());
        assert!(req.price_cents.is_none());
    }

    #[test]
    fn create_plugin_request_deserializes_full() {
        let json = r#"{
            "name": "Full Plugin",
            "slug": "full-plugin",
            "description": "A full plugin",
            "category": "productivity",
            "homepage_url": "https://example.com",
            "repository_url": "https://github.com/example/plugin",
            "license": "MIT",
            "payment_type": "paid",
            "price_cents": 999
        }"#;
        let req: CreatePluginRequest = serde_json::from_str(json).expect("deserialize");
        assert_eq!(req.name, "Full Plugin");
        assert_eq!(req.slug, "full-plugin");
        assert_eq!(req.description.as_deref(), Some("A full plugin"));
        assert_eq!(req.category.as_deref(), Some("productivity"));
        assert_eq!(req.payment_type.as_deref(), Some("paid"));
        assert_eq!(req.price_cents, Some(999));
    }

    #[test]
    fn update_plugin_request_deserializes_partial() {
        let json = r#"{"name":"New Name"}"#;
        let req: UpdatePluginRequest = serde_json::from_str(json).expect("deserialize");
        assert_eq!(req.name.as_deref(), Some("New Name"));
        assert!(req.description.is_none());
        assert!(req.category.is_none());
    }

    #[test]
    fn org_allowlist_entry_serializes() {
        let entry = OrgAllowlistEntry {
            org_id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            plugin_id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap(),
            added_by: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440002").unwrap(),
            added_at: DateTime::from_timestamp(0, 0).unwrap(),
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        assert!(json.contains("550e8400-e29b-41d4-a716-446655440000"));
        assert!(json.contains("550e8400-e29b-41d4-a716-446655440001"));
    }
}
