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

/// Atomically increment `download_count` for the matching approved version
/// and return its `archive_url`.
///
/// Used by the public bundle-download endpoint
/// (`GET /api/marketplace/plugins/:slug/versions/:version/download`). The
/// increment and the URL lookup happen in a single `UPDATE ... RETURNING`
/// statement so concurrent downloads can't double-count or race-skip a
/// counter bump.
///
/// Returns `Ok(None)` when no row matches (plugin/version not found OR the
/// version is not approved). The handler maps this to 404.
///
/// `(plugin_id, version, platform)` is the table's uniqueness constraint —
/// multiple platforms can share a version string. Today we increment ALL
/// matching approved rows (one per platform) and return the first
/// `archive_url`. This keeps the per-plugin "total downloads" sum accurate
/// when summed across platforms in `list_approved_plugins`. If the route
/// later grows a `?platform=` selector, swap to a platform-scoped WHERE.
pub async fn increment_download_count_and_get_url(
    pool: &PgPool,
    plugin_id: Uuid,
    version: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "UPDATE marketplace_versions \
         SET download_count = download_count + 1 \
         WHERE plugin_id = $1 AND version = $2 AND status = 'approved' \
         RETURNING archive_url",
    )
    .bind(plugin_id)
    .bind(version)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(url,)| url))
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

/// Errors that can occur when installing an approved marketplace version
/// onto a team.
///
/// M1 introduces `SignatureInvalid` as a defense-in-depth gate: even though
/// the upload path already verifies the developer signature and admin review
/// only countersigns after manual approval, we re-verify both signatures at
/// install time against the value in the version row (which lives across
/// process boundaries — R2, DB, anything that touched the row in between).
/// If either the developer signature or the server countersig is missing
/// or fails to verify against the stored `archive_sha256`, the install is
/// refused.
#[derive(thiserror::Error, Debug)]
pub enum InstallError {
    /// Plugin or version not found, or version is not in `approved` state.
    #[error("plugin or version not found")]
    NotFound,

    /// The version row's developer signature or server countersig either is
    /// absent or fails to verify against the stored archive hash + the
    /// matching public key. Defense-in-depth — should be unreachable on a
    /// correctly-administered server, but the audit explicitly asked for
    /// this check to be installed at the install boundary too.
    #[error("version signature invalid; cannot install")]
    SignatureInvalid,

    /// Internal database error.
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

pub async fn install_to_team(
    pool: &PgPool,
    plugin_id: Uuid,
    version: &str,
    team_id: Uuid,
    installed_by: Uuid,
) -> Result<(), InstallError> {
    // Look up plugin (approved only)
    let plugin = sqlx::query_as::<_, MarketplacePlugin>(
        "SELECT * FROM marketplace_plugins WHERE id = $1 AND status = 'approved'",
    )
    .bind(plugin_id)
    .fetch_optional(pool)
    .await?
    .ok_or(InstallError::NotFound)?;

    // Look up version (approved only)
    let ver = sqlx::query_as::<_, MarketplaceVersion>(
        "SELECT * FROM marketplace_versions \
         WHERE plugin_id = $1 AND version = $2 AND status = 'approved'",
    )
    .bind(plugin_id)
    .bind(version)
    .fetch_optional(pool)
    .await?
    .ok_or(InstallError::NotFound)?;

    // M1 defense-in-depth: re-verify BOTH signatures against the stored
    // archive_sha256 right before completing the install. The trust chain
    // is "developer signs the bundle → admin reviews + server countersigns
    // → install requires both" — every step that uses the version row
    // should re-check it, not assume earlier steps were sound.
    //
    // The audit explicitly called out that install handlers were trusting
    // the `status='approved'` row state and skipping the verify. That's
    // tolerable on a closed system but fails open the moment anything in
    // the trust chain (DB write path, review tooling, R2 swap) is wrong.
    let dev_sig = ver.signature.as_deref().ok_or(InstallError::SignatureInvalid)?;
    let countersig = ver.countersig.as_deref().ok_or(InstallError::SignatureInvalid)?;

    // Developer public key (lives on the users row, set via PUT
    // /api/marketplace/signing-key).
    let dev_pub_pem: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT signing_public_key FROM users WHERE id = $1",
    )
    .bind(plugin.author_id)
    .fetch_optional(pool)
    .await?;
    let dev_pub_pem = dev_pub_pem
        .and_then(|(k,)| k)
        .ok_or(InstallError::SignatureInvalid)?;

    let dev_ok = crate::signing::verify_developer_signature(
        &dev_pub_pem,
        &ver.archive_sha256,
        dev_sig,
    )
    .map_err(|_| InstallError::SignatureInvalid)?;
    if !dev_ok {
        return Err(InstallError::SignatureInvalid);
    }

    // Server public key (lives in server_config; populated by
    // signing::get_or_create_server_key on first countersign).
    let server_pub_pem: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM server_config WHERE key = 'signing_public_key'",
    )
    .fetch_optional(pool)
    .await?;
    let server_pub_pem = server_pub_pem
        .map(|(p,)| p)
        .ok_or(InstallError::SignatureInvalid)?;

    let server_ok = crate::signing::verify_developer_signature(
        &server_pub_pem,
        &ver.archive_sha256,
        countersig,
    )
    .map_err(|_| InstallError::SignatureInvalid)?;
    if !server_ok {
        return Err(InstallError::SignatureInvalid);
    }

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

    // ── M1: InstallError surface tests ─────────────────────────────────────

    #[test]
    fn install_error_distinguishes_not_found_from_signature_invalid() {
        // The handler relies on being able to pattern-match on variants —
        // pin that the enum has the three variants the handler expects.
        // A future refactor that conflates NotFound and SignatureInvalid
        // would silently downgrade 403 (signature) to 404 (not found),
        // breaking the audit-required UX.
        let nf = InstallError::NotFound;
        let si = InstallError::SignatureInvalid;
        let db = InstallError::Db(sqlx::Error::RowNotFound);

        assert!(matches!(nf, InstallError::NotFound));
        assert!(matches!(si, InstallError::SignatureInvalid));
        assert!(matches!(db, InstallError::Db(_)));

        // Display impls are user-visible (via thiserror) — pin the exact
        // string the M1 handler returns to the client.
        assert_eq!(
            format!("{si}"),
            "version signature invalid; cannot install"
        );
    }

    #[test]
    fn install_error_from_sqlx_via_question_mark() {
        // The `#[from] sqlx::Error` derive on the Db variant is what lets
        // install_to_team use `?` on sqlx calls. Verify the conversion
        // path stays wired up — if a future refactor strips `#[from]`,
        // this fails to compile, which is the warning we want.
        fn convert(e: sqlx::Error) -> InstallError {
            e.into()
        }
        assert!(matches!(
            convert(sqlx::Error::RowNotFound),
            InstallError::Db(_)
        ));
    }
}
