use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use tracing::error;
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::middleware::AuthUser;
use crate::models::{marketplace, organization};

#[derive(Debug, Deserialize)]
pub struct AddToAllowlistRequest {
    pub plugin_id: Uuid,
}

/// F3: log full sqlx error server-side, return generic 500 to the client.
fn db_500(op: &'static str) -> impl FnOnce(sqlx::Error) -> (StatusCode, String) {
    move |e| {
        error!(operation = op, error = %e, "marketplace org DB error");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/marketplace/org/:org_id/allowlist
pub async fn get_allowlist(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(org_id): Path<Uuid>,
) -> Result<Json<Vec<marketplace::PluginListItem>>, (StatusCode, String)> {
    let is_admin = organization::is_org_admin(&state.pool, user_id, org_id)
        .await
        .map_err(db_500("is_org_admin"))?;
    if !is_admin {
        return Err((StatusCode::FORBIDDEN, "Org admin access required".to_string()));
    }

    match marketplace::get_org_allowlist(&state.pool, org_id).await {
        Ok(items) => Ok(Json(items)),
        Err(e) => Err(db_500("get_org_allowlist")(e)),
    }
}

/// POST /api/marketplace/org/:org_id/allowlist
pub async fn add_to_allowlist(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(org_id): Path<Uuid>,
    Json(body): Json<AddToAllowlistRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let is_admin = organization::is_org_admin(&state.pool, user_id, org_id)
        .await
        .map_err(db_500("is_org_admin"))?;
    if !is_admin {
        return Err((StatusCode::FORBIDDEN, "Org admin access required".to_string()));
    }

    marketplace::add_to_org_allowlist(&state.pool, org_id, body.plugin_id, user_id)
        .await
        .map_err(db_500("add_to_org_allowlist"))?;

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/marketplace/org/:org_id/allowlist/:plugin_id
pub async fn remove_from_allowlist(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path((org_id, plugin_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, (StatusCode, String)> {
    let is_admin = organization::is_org_admin(&state.pool, user_id, org_id)
        .await
        .map_err(db_500("is_org_admin"))?;
    if !is_admin {
        return Err((StatusCode::FORBIDDEN, "Org admin access required".to_string()));
    }

    match marketplace::remove_from_org_allowlist(&state.pool, org_id, plugin_id).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err((StatusCode::NOT_FOUND, "Entry not found".to_string())),
        Err(e) => Err(db_500("remove_from_org_allowlist")(e)),
    }
}
