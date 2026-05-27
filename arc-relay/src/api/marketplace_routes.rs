use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use tracing::error;

use crate::api::AppState;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
