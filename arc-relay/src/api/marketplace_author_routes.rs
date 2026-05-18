use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use axum_extra::extract::Multipart;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::middleware::{AuthUser, RequirePro};
use crate::models::marketplace::{
    self, CreatePluginRequest, MarketplacePlugin, MarketplaceVersion, UpdatePluginRequest,
};

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/marketplace/plugins — create a new plugin (status = draft)
///
/// Pro-tier-gated per REQUIREMENTS.md §7 + §1 Tiers row: marketplace
/// publish is a `pro`-only surface. Free users get 403.
pub async fn create_plugin(
    State(state): State<AppState>,
    RequirePro(user_id): RequirePro,
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

/// PATCH /api/marketplace/plugins/:slug — update own plugin (slug may be a UUID or slug string)
pub async fn update_plugin(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(slug): Path<String>,
    Json(req): Json<UpdatePluginRequest>,
) -> Result<Json<MarketplacePlugin>, (StatusCode, String)> {
    let plugin_id = Uuid::parse_str(&slug)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid plugin ID".to_string()))?;
    match marketplace::update_plugin(&state.pool, plugin_id, user_id, &req).await {
        Ok(Some(plugin)) => Ok(Json(plugin)),
        Ok(None) => Err((StatusCode::NOT_FOUND, "Plugin not found or not yours".to_string())),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// DELETE /api/marketplace/plugins/:slug — delete own plugin
pub async fn delete_plugin(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(slug): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let plugin_id = Uuid::parse_str(&slug)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid plugin ID".to_string()))?;
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

/// POST /api/marketplace/plugins/:slug/versions — upload a new plugin version
pub async fn upload_version(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(slug): Path<String>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<MarketplaceVersion>), (StatusCode, String)> {
    let plugin_id = Uuid::parse_str(&slug)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid plugin ID".to_string()))?;

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

    // Verify developer signature if provided
    if let Some(ref sig) = signature_str {
        let user_key = crate::models::user::get_signing_key(&state.pool, user_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?;

        if let Some(pub_pem) = user_key {
            let valid = crate::signing::verify_developer_signature(&pub_pem, &archive_sha256, sig)
                .map_err(|e| (StatusCode::BAD_REQUEST, format!("Signature error: {e}")))?;
            if !valid {
                return Err((StatusCode::BAD_REQUEST, "Invalid signature".to_string()));
            }
        }
    }

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

/// PUT /api/marketplace/signing-key — upload developer's public key
pub async fn set_signing_key(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    body: String,
) -> Result<StatusCode, (StatusCode, String)> {
    if !body.contains("BEGIN PUBLIC KEY") {
        return Err((StatusCode::BAD_REQUEST, "Expected PEM-encoded public key".to_string()));
    }

    crate::models::user::set_signing_key(&state.pool, user_id, &body)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/marketplace/signing-key/server — get marketplace server public key
pub async fn get_server_public_key(
    State(state): State<AppState>,
) -> Result<String, (StatusCode, String)> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM server_config WHERE key = 'signing_public_key'",
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?;

    match row {
        Some((pem,)) => Ok(pem),
        None => Err((StatusCode::NOT_FOUND, "Server key not yet generated".to_string())),
    }
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
