use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    Json,
};
use serde::Serialize;
use tracing::error;

use crate::api::AppState;
use crate::auth::middleware::AuthUser;
use crate::models::marketplace::{self, MarketplaceQuery, PluginListItem};

/// F3: log full sqlx error server-side, return generic 500 to the client.
///
/// sqlx errors include table names, column names, and constraint names;
/// returning them over HTTP leaks schema details. This wrapper is used
/// across the marketplace public-read handlers.
fn db_500(op: &'static str) -> impl FnOnce(sqlx::Error) -> (StatusCode, String) {
    move |e| {
        error!(operation = op, error = %e, "marketplace public DB error");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    }
}

#[derive(Debug, Serialize)]
pub struct PluginDetailResponse {
    #[serde(flatten)]
    pub plugin: marketplace::MarketplacePlugin,
    pub author_name: Option<String>,
    pub download_count: i64,
    pub latest_version: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CategoriesResponse {
    pub categories: Vec<String>,
}

/// GET /api/marketplace/plugins
pub async fn list_plugins(
    State(state): State<AppState>,
    Query(query): Query<MarketplaceQuery>,
) -> Result<Json<Vec<PluginListItem>>, (StatusCode, String)> {
    let plugins = marketplace::list_approved_plugins(&state.pool, &query)
        .await
        .map_err(db_500("list_approved_plugins"))?;
    Ok(Json(plugins))
}

/// GET /api/marketplace/plugins/:slug
pub async fn get_plugin(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<PluginDetailResponse>, (StatusCode, String)> {
    let plugin = marketplace::get_plugin_by_slug(&state.pool, &slug)
        .await
        .map_err(db_500("get_plugin_by_slug"))?
        .ok_or((StatusCode::NOT_FOUND, "Plugin not found".to_string()))?;

    let author = crate::models::user::find_user_by_id(&state.pool, plugin.author_id)
        .await
        .map_err(db_500("find_user_by_id"))?;

    let versions = marketplace::get_plugin_versions(&state.pool, plugin.id)
        .await
        .map_err(db_500("get_plugin_versions"))?;

    let download_count: i64 = versions.iter().map(|v| v.download_count).sum();
    let latest_version = versions.first().map(|v| v.version.clone());

    Ok(Json(PluginDetailResponse {
        plugin,
        author_name: author.and_then(|u| u.name),
        download_count,
        latest_version,
    }))
}

/// GET /api/marketplace/plugins/:slug/versions
pub async fn get_plugin_versions(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<Vec<marketplace::MarketplaceVersion>>, (StatusCode, String)> {
    let plugin = marketplace::get_plugin_by_slug(&state.pool, &slug)
        .await
        .map_err(db_500("get_plugin_by_slug"))?
        .ok_or((StatusCode::NOT_FOUND, "Plugin not found".to_string()))?;

    let versions = marketplace::get_plugin_versions(&state.pool, plugin.id)
        .await
        .map_err(db_500("get_plugin_versions"))?;

    Ok(Json(versions))
}

/// GET /api/marketplace/categories
pub async fn list_categories(
    State(state): State<AppState>,
) -> Result<Json<CategoriesResponse>, (StatusCode, String)> {
    let categories = marketplace::list_categories(&state.pool)
        .await
        .map_err(db_500("list_categories"))?;
    Ok(Json(CategoriesResponse { categories }))
}

/// GET /api/marketplace/plugins/:slug/versions/:version/download
///
/// M3: redirect-style bundle download endpoint. The marketplace stores
/// plugin archives in R2 and exposes only a URL on the version row; clients
/// previously had to construct that URL themselves. This handler:
///
/// 1. Resolves the plugin by slug → plugin_id (404 if not approved/missing).
/// 2. Atomically increments `download_count` for the matching approved
///    version and reads back the `archive_url` in a single UPDATE-RETURNING
///    statement (so concurrent downloads can't double-count or race-skip).
///    Returns 404 if the version row doesn't exist or isn't approved.
/// 3. Issues a 302 Found pointing at the R2 URL — we deliberately do NOT
///    proxy bytes through the relay because that would 6x our bandwidth
///    bill for every plugin install.
///
/// Auth: any authenticated user (free or pro tier). Marketplace browsing
/// is public reads, but the download counter exists to feed
/// per-plugin/per-user analytics, so we require a JWT here.
pub async fn download_version(
    State(state): State<AppState>,
    AuthUser(_user_id): AuthUser,
    Path((slug, version)): Path<(String, String)>,
) -> Result<(StatusCode, HeaderMap), (StatusCode, String)> {
    let plugin = marketplace::get_plugin_by_slug(&state.pool, &slug)
        .await
        .map_err(db_500("get_plugin_by_slug"))?
        .ok_or((StatusCode::NOT_FOUND, "Plugin not found".to_string()))?;

    let archive_url =
        marketplace::increment_download_count_and_get_url(&state.pool, plugin.id, &version)
            .await
            .map_err(db_500("increment_download_count_and_get_url"))?
            .ok_or((
                StatusCode::NOT_FOUND,
                "Version not found or not approved".to_string(),
            ))?;

    // 302 Found — axum's `Redirect` only exposes 303/307/308 constructors,
    // and we explicitly want 302 per the M3 audit spec. Build the response
    // manually with a Location header.
    let location = HeaderValue::from_str(&archive_url).map_err(|e| {
        error!(
            operation = "download_version",
            error = %e,
            archive_url = %archive_url,
            "stored archive_url is not a valid Location header value"
        );
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    })?;
    let mut headers = HeaderMap::new();
    headers.insert(header::LOCATION, location);
    Ok((StatusCode::FOUND, headers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{build_router, tests::test_state};
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[test]
    fn categories_response_serializes() {
        let resp = CategoriesResponse {
            categories: vec!["ai".to_string(), "devops".to_string()],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["categories"], serde_json::json!(["ai", "devops"]));
    }

    #[test]
    fn marketplace_query_deserializes_empty() {
        let q: MarketplaceQuery = serde_urlencoded::from_str("").unwrap();
        assert!(q.search.is_none());
        assert!(q.category.is_none());
    }

    #[test]
    fn marketplace_query_deserializes_full() {
        let q: MarketplaceQuery =
            serde_urlencoded::from_str("search=ai&category=devops&sort=recent&limit=10").unwrap();
        assert_eq!(q.search.as_deref(), Some("ai"));
        assert_eq!(q.category.as_deref(), Some("devops"));
        assert_eq!(q.sort.as_deref(), Some("recent"));
        assert_eq!(q.limit, Some(10));
    }

    // ── M3: GET /api/marketplace/plugins/:slug/versions/:version/download ──

    /// Without a Bearer token the route must reject with 401, not 404. A
    /// 404 here would mean the download route was never wired into the
    /// marketplace router.
    #[tokio::test]
    async fn download_version_requires_auth() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("GET")
            .uri("/api/marketplace/plugins/some-plugin/versions/1.0.0/download")
            .body(Body::empty())
            .unwrap();
        let s = app.oneshot(req).await.unwrap().status();
        assert_eq!(
            s,
            StatusCode::UNAUTHORIZED,
            "GET .../versions/:version/download must require auth (got {s}); \
             404 means the route was never wired"
        );
    }

    /// A valid JWT must reach the handler. With the lazy pool (no DB
    /// connection) the slug→plugin lookup fails with a sqlx error, which
    /// `db_500` redacts into a 500. We assert NOT 404 (route wired AND
    /// auth passed) and NOT 401 (token accepted).
    #[tokio::test]
    async fn download_version_with_valid_jwt_reaches_handler() {
        use crate::auth::jwt::generate_token_pair;

        let state = test_state();
        let app = build_router(state.clone());

        let user_id = uuid::Uuid::new_v4();
        let pair =
            generate_token_pair(user_id, &state.config.jwt_secret, 900, 86_400).unwrap();

        let req = Request::builder()
            .method("GET")
            .uri("/api/marketplace/plugins/some-plugin/versions/1.0.0/download")
            .header("authorization", format!("Bearer {}", pair.access_token))
            .body(Body::empty())
            .unwrap();
        let s = app.oneshot(req).await.unwrap().status();
        // Auth must be accepted: no 401.
        assert_ne!(
            s,
            StatusCode::UNAUTHORIZED,
            "valid JWT must not get 401 (got {s})"
        );
        // Route must exist: no 404 BEFORE we hit the DB. With the lazy
        // pool, the slug lookup raises a sqlx error → 500 via db_500.
        assert_eq!(
            s,
            StatusCode::INTERNAL_SERVER_ERROR,
            "with auth passed but DB unavailable, handler must reach the \
             slug lookup and 500 (got {s}); a 404 here would mean auth ran \
             before the DB call OR the route was never wired"
        );
    }

    /// Integration test: with a real DB, seed an approved plugin + version,
    /// hit the endpoint, assert 302 + Location header points at the
    /// stored `archive_url`, and verify `download_count` was incremented.
    ///
    /// Marked `#[ignore]` because the unit-test suite uses a lazy pool with
    /// no live Postgres. Run via `cargo test -- --ignored` against a DB
    /// with `004_marketplace.sql` applied.
    #[tokio::test]
    #[ignore = "requires a live Postgres with marketplace migrations applied"]
    async fn download_version_increments_count_and_redirects() {
        // Intentionally a stub — the route-shape tests above cover the
        // wiring, auth, and 500-on-DB-error paths. The full
        // seed-and-assert flow is out of scope for the unit-test harness;
        // it lives in `tests/marketplace_download_integration.rs` once
        // we have the integration-test scaffold for the marketplace.
    }
}
