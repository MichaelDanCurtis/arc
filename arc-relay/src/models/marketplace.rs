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

/// Three-valued allowlist decision for `(org_id, plugin_id)`.
///
/// Used by `install_to_team` (G2) and any future install-time gate that
/// needs to honour an org's curated catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowlistDecision {
    /// Org maintains a curated allowlist and the plugin is on it → allow.
    Allowed,
    /// Org maintains a curated allowlist and the plugin is NOT on it → reject.
    Blocked,
    /// Org has zero allowlist rows at all → no curation, treat as open.
    /// This matches the open-source posture: an org that has never added
    /// anything to its allowlist hasn't opted into curation yet, so we
    /// don't want to block every install for them. The moment an admin
    /// adds the first row, the allowlist becomes enforced.
    NoCuration,
}

/// Decide whether `(org_id, plugin_id)` may be installed under the org's
/// curated allowlist policy.
///
/// See [`AllowlistDecision`] for the three outcomes. One query checks both
/// "is this plugin on the list?" and "does the list have ANY entries?" so
/// we avoid a TOCTOU between the existence check and the membership check.
pub async fn check_org_allowlist(
    pool: &PgPool,
    org_id: Uuid,
    plugin_id: Uuid,
) -> Result<AllowlistDecision, sqlx::Error> {
    // Single query — count total rows for the org, and whether any of them
    // matches the requested plugin. EXISTS is cheap and indexed on the
    // UNIQUE(org_id, plugin_id) constraint; COUNT(*) scans the org's
    // rows but org allowlists are small (curated by humans).
    let row: (i64, bool) = sqlx::query_as(
        "SELECT \
            (SELECT COUNT(*) FROM org_plugin_allowlist WHERE org_id = $1) AS total, \
            EXISTS (SELECT 1 FROM org_plugin_allowlist WHERE org_id = $1 AND plugin_id = $2) AS present",
    )
    .bind(org_id)
    .bind(plugin_id)
    .fetch_one(pool)
    .await?;

    let (total, present) = row;
    Ok(match (total, present) {
        (0, _) => AllowlistDecision::NoCuration,
        (_, true) => AllowlistDecision::Allowed,
        (_, false) => AllowlistDecision::Blocked,
    })
}

// ---------------------------------------------------------------------------
// Install to team
// ---------------------------------------------------------------------------

/// Failure modes for [`install_to_team`].
///
/// The variants map onto distinct HTTP status codes in the handler:
/// - [`NotMember`] → `403 Forbidden`  (G1 — caller doesn't belong to team)
/// - [`NotAllowlisted`] → `403 Forbidden`  (G2 — org curates and plugin isn't on the list)
/// - [`NotFound`] → `404 Not Found`  (plugin / version missing or not approved — G3)
/// - [`SignatureInvalid`] → `403 Forbidden`  (M1 — defense-in-depth signature re-verify failed)
/// - [`Db`] → `500 Internal Server Error`  (logged server-side, generic message client-side)
///
/// M1 introduces `SignatureInvalid` as defense-in-depth: even though the
/// upload path already verifies the developer signature and admin review
/// only countersigns after manual approval, we re-verify both signatures
/// at install time against the stored `archive_sha256`. If either is
/// missing or fails to verify against the matching public key, the
/// install is refused.
#[derive(thiserror::Error, Debug)]
pub enum InstallError {
    /// G1: the authenticated caller is not a member of `team_id`.
    #[error("not a member of team")]
    NotMember,
    /// G2: the team's org operates a curated allowlist and this plugin
    /// is not on it.
    #[error("plugin not on org allowlist")]
    NotAllowlisted,
    /// Plugin not found, version not found, or status != 'approved' (G3).
    /// Collapsed into a single variant so the handler returns a stable
    /// 404 regardless of which row was missing — avoids leaking whether
    /// the plugin exists at all to unauthorised probers.
    #[error("plugin or version not found")]
    NotFound,
    /// M1: the version row's developer signature or server countersig is
    /// absent or fails to verify against the stored archive hash + the
    /// matching public key. Defense-in-depth — should be unreachable on
    /// a correctly-administered server, but the audit explicitly asked
    /// for this check at the install boundary too.
    #[error("version signature invalid; cannot install")]
    SignatureInvalid,
    /// Any other sqlx error — wrapped for server-side logging.
    #[error("database error: {0}")]
    Db(sqlx::Error),
}

/// Manual `From` impl (instead of `#[from]` on `Db`) so that
/// `sqlx::Error::RowNotFound` collapses onto `InstallError::NotFound`
/// automatically through `?`. Without this mapping, every missing-row
/// case would have to be unwrapped manually inside `install_to_team`.
impl From<sqlx::Error> for InstallError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::NotFound,
            other => Self::Db(other),
        }
    }
}


pub async fn install_to_team(
    pool: &PgPool,
    plugin_id: Uuid,
    version: &str,
    team_id: Uuid,
    installed_by: Uuid,
) -> Result<(), InstallError> {
    // ── G1: membership check ────────────────────────────────────────────
    // Verify the caller actually belongs to `team_id` before doing
    // anything else. Skipping this would let any authenticated user
    // install plugins into arbitrary teams just by guessing UUIDs.
    if !crate::models::team::is_team_member(pool, team_id, installed_by).await? {
        return Err(InstallError::NotMember);
    }

    // ── G2: org-curated allowlist enforcement ───────────────────────────
    // Look up the team's parent org (if any). When the org is curating
    // its catalog (i.e. the allowlist has at least one row), the plugin
    // MUST be on the list. If the team has no org, or the org has zero
    // allowlist rows, this guard is a no-op. See [`AllowlistDecision`]
    // for the "no curation → open" rationale.
    let org_id = crate::models::team::get_team_org_id(pool, team_id).await?;
    if let Some(org_id) = org_id {
        match check_org_allowlist(pool, org_id, plugin_id).await? {
            AllowlistDecision::Allowed | AllowlistDecision::NoCuration => {}
            AllowlistDecision::Blocked => return Err(InstallError::NotAllowlisted),
        }
    }

    // ── Plugin lookup (G3 — approved only) ──────────────────────────────
    let plugin = sqlx::query_as::<_, MarketplacePlugin>(
        "SELECT * FROM marketplace_plugins WHERE id = $1 AND status = 'approved'",
    )
    .bind(plugin_id)
    .fetch_optional(pool)
    .await?
    .ok_or(InstallError::NotFound)?;

    // ── G3: only install approved versions ──────────────────────────────
    // The `status = 'approved'` filter is intentional and critical: a
    // version whose status is `'rejected'` (or still `'pending_review'`)
    // must NEVER be installable. Without this clause a previously-good
    // version_string could be rejected upstream yet remain installable
    // for anyone who knew the (plugin_id, version) pair, defeating the
    // review workflow. The "version not found" path below collapses
    // missing / rejected / pending into a single 404 so the API doesn't
    // disclose which state a non-approved version is in.
    let ver = sqlx::query_as::<_, MarketplaceVersion>(
        "SELECT * FROM marketplace_versions \
         WHERE plugin_id = $1 AND version = $2 AND status = 'approved'",
    )
    .bind(plugin_id)
    .bind(version)
    .fetch_optional(pool)
    .await?
    .ok_or(InstallError::NotFound)?;

    // ── M1 defense-in-depth: re-verify BOTH signatures ─────────────────
    // Re-verify the developer signature AND the server countersig
    // against the stored archive_sha256 right before completing the
    // install. Trust chain: developer signs the bundle → admin reviews
    // + server countersigns → install requires both. Every step that
    // uses the version row re-checks it; we never assume earlier steps
    // were sound. Audit called this out as the third leg of the trust
    // chain that was previously skipped.
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

    // ── M1 + M2: InstallError surface tests ─────────────────────────────

    #[test]
    fn install_error_distinguishes_not_found_from_signature_invalid() {
        // The handler relies on being able to pattern-match on variants.
        // A future refactor that conflates NotFound and SignatureInvalid
        // would silently downgrade 403 (signature) to 404 (not found),
        // breaking the audit-required UX.
        let nf = InstallError::NotFound;
        let si = InstallError::SignatureInvalid;
        let nm = InstallError::NotMember;
        let na = InstallError::NotAllowlisted;
        let db = InstallError::Db(sqlx::Error::PoolClosed);

        assert!(matches!(nf, InstallError::NotFound));
        assert!(matches!(si, InstallError::SignatureInvalid));
        assert!(matches!(nm, InstallError::NotMember));
        assert!(matches!(na, InstallError::NotAllowlisted));
        assert!(matches!(db, InstallError::Db(_)));

        // Pin the exact Display string the M1 handler returns to the
        // client (via thiserror).
        assert_eq!(
            format!("{si}"),
            "version signature invalid; cannot install"
        );
    }

    #[test]
    fn install_error_maps_row_not_found_to_not_found() {
        // The handler relies on `From<sqlx::Error> for InstallError`
        // collapsing the row-missing case onto a single variant so a
        // missing plugin OR a missing version OR a non-approved version
        // all surface as 404 to the client. Any other sqlx error must
        // remain wrapped in `Db(_)` so the handler can log + 500.
        let mapped: InstallError = sqlx::Error::RowNotFound.into();
        assert!(matches!(mapped, InstallError::NotFound));

        let other: InstallError = sqlx::Error::PoolClosed.into();
        assert!(matches!(other, InstallError::Db(_)));
    }

    #[test]
    fn install_error_display_messages_are_stable() {
        // The handler reuses these strings verbatim for client-facing
        // error responses. If these strings change, the route-shape
        // tests in marketplace_install_routes.rs#tests (and any
        // API consumers) need to be updated.
        assert_eq!(
            format!("{}", InstallError::NotMember),
            "not a member of team"
        );
        assert_eq!(
            format!("{}", InstallError::NotAllowlisted),
            "plugin not on org allowlist"
        );
        assert_eq!(
            format!("{}", InstallError::NotFound),
            "plugin or version not found"
        );
        assert_eq!(
            format!("{}", InstallError::SignatureInvalid),
            "version signature invalid; cannot install"
        );
    }

    #[test]
    fn allowlist_decision_is_eq() {
        // Sanity: the enum is `PartialEq` so handlers can pattern-match
        // or compare directly without `matches!` boilerplate.
        assert_eq!(AllowlistDecision::Allowed, AllowlistDecision::Allowed);
        assert_ne!(AllowlistDecision::Allowed, AllowlistDecision::Blocked);
        assert_ne!(AllowlistDecision::NoCuration, AllowlistDecision::Blocked);
    }

    fn db_url() -> Option<String> {
        std::env::var("DATABASE_URL").ok()
    }

    /// G2 (boundary): the helper that powers G2.
    ///
    /// `#[ignore]` because it needs a live Postgres connection. Run with
    /// `cargo test -p arc-relay --tests -- --ignored check_org_allowlist`
    /// against a database that has applied all migrations.
    #[tokio::test]
    #[ignore]
    async fn check_org_allowlist_three_states() {
        let url = match db_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        // Set up: one org with no allowlist rows (→ NoCuration),
        // one org with a row matching plugin_a (→ Allowed for plugin_a,
        // Blocked for plugin_b).
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x')")
            .bind(user_id)
            .bind(format!("alw_{user_id}@test.invalid"))
            .execute(&pool)
            .await
            .expect("insert user");

        let org_uncurated = Uuid::new_v4();
        let org_curated = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, $2, $3), ($4, $5, $6)")
            .bind(org_uncurated)
            .bind("Uncurated")
            .bind(format!("uncurated-{org_uncurated}"))
            .bind(org_curated)
            .bind("Curated")
            .bind(format!("curated-{org_curated}"))
            .execute(&pool)
            .await
            .expect("insert orgs");

        // Two distinct plugins.
        let req_a = CreatePluginRequest {
            name: format!("Plugin A {}", Uuid::new_v4()),
            slug: format!("plugin-a-{}", Uuid::new_v4()),
            description: None,
            category: None,
            homepage_url: None,
            repository_url: None,
            license: None,
            payment_type: None,
            price_cents: None,
        };
        let plugin_a = create_plugin(&pool, user_id, &req_a)
            .await
            .expect("create plugin_a");
        let req_b = CreatePluginRequest {
            name: format!("Plugin B {}", Uuid::new_v4()),
            slug: format!("plugin-b-{}", Uuid::new_v4()),
            description: None,
            category: None,
            homepage_url: None,
            repository_url: None,
            license: None,
            payment_type: None,
            price_cents: None,
        };
        let plugin_b = create_plugin(&pool, user_id, &req_b)
            .await
            .expect("create plugin_b");

        // Curated org only allowlists plugin_a.
        add_to_org_allowlist(&pool, org_curated, plugin_a.id, user_id)
            .await
            .expect("add allowlist");

        // (1) Uncurated org → NoCuration regardless of plugin.
        assert_eq!(
            check_org_allowlist(&pool, org_uncurated, plugin_a.id)
                .await
                .expect("uncurated lookup"),
            AllowlistDecision::NoCuration,
        );

        // (2) Curated org + on-list plugin → Allowed.
        assert_eq!(
            check_org_allowlist(&pool, org_curated, plugin_a.id)
                .await
                .expect("curated on-list lookup"),
            AllowlistDecision::Allowed,
        );

        // (3) Curated org + off-list plugin → Blocked.
        assert_eq!(
            check_org_allowlist(&pool, org_curated, plugin_b.id)
                .await
                .expect("curated off-list lookup"),
            AllowlistDecision::Blocked,
        );
    }

    /// G1 + G3 in one shot.
    ///
    /// Verifies that:
    ///   - install_to_team rejects a non-member with `InstallError::NotMember`
    ///   - install_to_team rejects a rejected-status version with `InstallError::NotFound`
    ///
    /// `#[ignore]` because it needs a live Postgres connection. Run with
    /// `cargo test -p arc-relay --tests -- --ignored install_to_team`.
    #[tokio::test]
    #[ignore]
    async fn install_to_team_rejects_non_member_and_rejected_version() {
        let url = match db_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        // Insert two users: `owner` will own the team, `outsider` will
        // not. `outsider` attempting install must hit G1.
        let owner = Uuid::new_v4();
        let outsider = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x'), ($3, $4, 'x')",
        )
        .bind(owner)
        .bind(format!("inst_owner_{owner}@test.invalid"))
        .bind(outsider)
        .bind(format!("inst_outsider_{outsider}@test.invalid"))
        .execute(&pool)
        .await
        .expect("insert users");

        let team = crate::models::team::create_team(&pool, "InstallTest", owner)
            .await
            .expect("create team");

        // Author a plugin and an approved version.
        let plugin_req = CreatePluginRequest {
            name: format!("Install Plugin {}", Uuid::new_v4()),
            slug: format!("install-plugin-{}", Uuid::new_v4()),
            description: None,
            category: None,
            homepage_url: None,
            repository_url: None,
            license: None,
            payment_type: None,
            price_cents: None,
        };
        let plugin = create_plugin(&pool, owner, &plugin_req)
            .await
            .expect("create plugin");
        approve_plugin_if_first(&pool, plugin.id)
            .await
            .expect("approve plugin");
        let v_approved = create_version(
            &pool,
            plugin.id,
            "1.0.0",
            "darwin-aarch64",
            "https://example.invalid/v1.tar.gz",
            "0".repeat(64).as_str(),
            None,
            None,
        )
        .await
        .expect("create approved version");
        set_version_status(&pool, v_approved.id, "approved")
            .await
            .expect("set approved");

        // ── G1: outsider (not in team_members) → NotMember ──────────────
        let g1 = install_to_team(&pool, plugin.id, "1.0.0", team.id, outsider).await;
        assert!(
            matches!(g1, Err(InstallError::NotMember)),
            "expected NotMember, got {g1:?}",
        );

        // Sanity: owner CAN install the approved version.
        install_to_team(&pool, plugin.id, "1.0.0", team.id, owner)
            .await
            .expect("owner install");

        // ── G3: rejected version → NotFound ─────────────────────────────
        let v_rejected = create_version(
            &pool,
            plugin.id,
            "9.9.9",
            "darwin-aarch64",
            "https://example.invalid/v9.tar.gz",
            "f".repeat(64).as_str(),
            None,
            None,
        )
        .await
        .expect("create rejected version");
        set_version_status(&pool, v_rejected.id, "rejected")
            .await
            .expect("set rejected");

        let g3 = install_to_team(&pool, plugin.id, "9.9.9", team.id, owner).await;
        assert!(
            matches!(g3, Err(InstallError::NotFound)),
            "rejected version must surface as NotFound (got {g3:?})",
        );
    }

    /// G2: install_to_team enforces the org allowlist when the org has
    /// any allowlist rows at all.
    #[tokio::test]
    #[ignore]
    async fn install_to_team_blocks_when_org_curates_and_plugin_off_list() {
        let url = match db_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        let owner = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x')")
            .bind(owner)
            .bind(format!("g2_owner_{owner}@test.invalid"))
            .execute(&pool)
            .await
            .expect("insert user");

        let org_id = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, $2, $3)")
            .bind(org_id)
            .bind("G2 Org")
            .bind(format!("g2-org-{org_id}"))
            .execute(&pool)
            .await
            .expect("insert org");

        let team = crate::models::team::create_team(&pool, "G2Team", owner)
            .await
            .expect("create team");
        // Attach team to the curated org.
        sqlx::query("UPDATE teams SET org_id = $1 WHERE id = $2")
            .bind(org_id)
            .bind(team.id)
            .execute(&pool)
            .await
            .expect("attach team to org");

        // Author two approved plugins. The org allowlists only `plugin_on`.
        let p_on_req = CreatePluginRequest {
            name: format!("On List {}", Uuid::new_v4()),
            slug: format!("on-list-{}", Uuid::new_v4()),
            description: None,
            category: None,
            homepage_url: None,
            repository_url: None,
            license: None,
            payment_type: None,
            price_cents: None,
        };
        let plugin_on = create_plugin(&pool, owner, &p_on_req)
            .await
            .expect("create on-list plugin");
        approve_plugin_if_first(&pool, plugin_on.id)
            .await
            .expect("approve on-list");
        let v_on = create_version(
            &pool,
            plugin_on.id,
            "1.0.0",
            "darwin-aarch64",
            "https://example.invalid/on.tar.gz",
            "a".repeat(64).as_str(),
            None,
            None,
        )
        .await
        .expect("create on-list version");
        set_version_status(&pool, v_on.id, "approved")
            .await
            .expect("approve version");

        let p_off_req = CreatePluginRequest {
            name: format!("Off List {}", Uuid::new_v4()),
            slug: format!("off-list-{}", Uuid::new_v4()),
            description: None,
            category: None,
            homepage_url: None,
            repository_url: None,
            license: None,
            payment_type: None,
            price_cents: None,
        };
        let plugin_off = create_plugin(&pool, owner, &p_off_req)
            .await
            .expect("create off-list plugin");
        approve_plugin_if_first(&pool, plugin_off.id)
            .await
            .expect("approve off-list");
        let v_off = create_version(
            &pool,
            plugin_off.id,
            "1.0.0",
            "darwin-aarch64",
            "https://example.invalid/off.tar.gz",
            "b".repeat(64).as_str(),
            None,
            None,
        )
        .await
        .expect("create off-list version");
        set_version_status(&pool, v_off.id, "approved")
            .await
            .expect("approve version");

        add_to_org_allowlist(&pool, org_id, plugin_on.id, owner)
            .await
            .expect("allowlist on plugin");

        // (a) Pre-allowlist: org has zero rows → install would succeed.
        //     We can't really assert this here without rolling back, but
        //     the previous test (`check_org_allowlist_three_states`) covers
        //     the NoCuration branch directly.

        // (b) Curated org + on-list plugin → install succeeds.
        install_to_team(&pool, plugin_on.id, "1.0.0", team.id, owner)
            .await
            .expect("on-list install should succeed");

        // (c) Curated org + off-list plugin → NotAllowlisted.
        let r = install_to_team(&pool, plugin_off.id, "1.0.0", team.id, owner).await;
        assert!(
            matches!(r, Err(InstallError::NotAllowlisted)),
            "expected NotAllowlisted, got {r:?}",
        );
    }
}
