use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use axum_extra::extract::Multipart;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::middleware::AuthUser;
use crate::models::marketplace::{
    self, CreatePluginRequest, MarketplacePlugin, MarketplaceVersion, UpdatePluginRequest,
};

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/marketplace/plugins — create a new plugin (status = draft)
pub async fn create_plugin(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Json(req): Json<CreatePluginRequest>,
) -> Result<(StatusCode, Json<MarketplacePlugin>), (StatusCode, String)> {
    match marketplace::create_plugin(&state.pool, user_id, &req).await {
        Ok(plugin) => Ok((StatusCode::CREATED, Json(plugin))),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("duplicate key") || msg.contains("unique") {
                Err((StatusCode::CONFLICT, "A plugin with that slug already exists".to_string()))
            } else {
                Err((StatusCode::INTERNAL_SERVER_ERROR, msg))
            }
        }
    }
}

/// PATCH /api/marketplace/plugins/:id — update own plugin
pub async fn update_plugin(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(plugin_id): Path<Uuid>,
    Json(req): Json<UpdatePluginRequest>,
) -> Result<Json<MarketplacePlugin>, (StatusCode, String)> {
    match marketplace::update_plugin(&state.pool, plugin_id, user_id, &req).await {
        Ok(Some(plugin)) => Ok(Json(plugin)),
        Ok(None) => Err((StatusCode::NOT_FOUND, "Plugin not found or not yours".to_string())),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// DELETE /api/marketplace/plugins/:id — delete own plugin
pub async fn delete_plugin(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(plugin_id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, String)> {
    match marketplace::delete_plugin(&state.pool, plugin_id, user_id).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err((StatusCode::NOT_FOUND, "Plugin not found or not yours".to_string())),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// GET /api/marketplace/my-plugins — list own plugins
pub async fn my_plugins(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
) -> Result<Json<Vec<MarketplacePlugin>>, (StatusCode, String)> {
    match marketplace::list_plugins_by_author(&state.pool, user_id).await {
        Ok(plugins) => Ok(Json(plugins)),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// POST /api/marketplace/plugins/:id/versions — upload a new plugin version
pub async fn upload_version(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(plugin_id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<MarketplaceVersion>), (StatusCode, String)> {
    // Check R2 is configured
    let r2 = state
        .r2
        .as_ref()
        .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "R2 storage not configured".to_string()))?;

    // Verify plugin exists and belongs to this user
    let plugin = marketplace::get_plugin_by_id(&state.pool, plugin_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Plugin not found".to_string()))?;

    if plugin.author_id != user_id {
        return Err((StatusCode::FORBIDDEN, "Not your plugin".to_string()));
    }

    // Parse multipart fields
    let mut version_str: Option<String> = None;
    let mut platform_str: Option<String> = None;
    let mut signature_str: Option<String> = None;
    let mut file_bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e: axum_extra::extract::multipart::MultipartError| {
            (StatusCode::BAD_REQUEST, e.to_string())
        })?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "version" => {
                version_str = Some(
                    field
                        .text()
                        .await
                        .map_err(|e: axum_extra::extract::multipart::MultipartError| {
                            (StatusCode::BAD_REQUEST, e.to_string())
                        })?,
                );
            }
            "platform" => {
                platform_str = Some(
                    field
                        .text()
                        .await
                        .map_err(|e: axum_extra::extract::multipart::MultipartError| {
                            (StatusCode::BAD_REQUEST, e.to_string())
                        })?,
                );
            }
            "signature" => {
                signature_str = Some(
                    field
                        .text()
                        .await
                        .map_err(|e: axum_extra::extract::multipart::MultipartError| {
                            (StatusCode::BAD_REQUEST, e.to_string())
                        })?,
                );
            }
            "file" => {
                file_bytes = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e: axum_extra::extract::multipart::MultipartError| {
                            (StatusCode::BAD_REQUEST, e.to_string())
                        })?
                        .to_vec(),
                );
            }
            _ => {}
        }
    }

    let version = version_str
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing 'version' field".to_string()))?;
    let platform = platform_str
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing 'platform' field".to_string()))?;
    let bytes = file_bytes
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing 'file' field".to_string()))?;

    // Compute SHA-256
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let archive_sha256 = hex::encode(hasher.finalize());

    let size_bytes = bytes.len() as i64;

    // Upload to R2
    let key = crate::storage::R2Client::archive_key(&plugin.slug, &version, &platform);
    let archive_url = r2
        .upload_plugin_archive(&key, bytes, "application/octet-stream")
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Create version record
    let ver = marketplace::create_version(
        &state.pool,
        plugin_id,
        &version,
        &platform,
        &archive_url,
        &archive_sha256,
        Some(size_bytes),
        signature_str.as_deref(),
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Transition plugin from draft → pending_review
    marketplace::set_plugin_pending_if_draft(&state.pool, plugin_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok((StatusCode::CREATED, Json(ver)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{build_router, tests::test_state};
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn marketplace_create_plugin_requires_auth() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/api/marketplace/plugins")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"Test","slug":"test"}"#))
            .unwrap();
        let s = app.oneshot(req).await.unwrap().status();
        assert_eq!(s, StatusCode::UNAUTHORIZED, "POST /api/marketplace/plugins must require auth");
    }

    #[tokio::test]
    async fn marketplace_my_plugins_requires_auth() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("GET")
            .uri("/api/marketplace/my-plugins")
            .body(Body::empty())
            .unwrap();
        let s = app.oneshot(req).await.unwrap().status();
        assert_eq!(s, StatusCode::UNAUTHORIZED, "GET /api/marketplace/my-plugins must require auth");
    }
}
