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
