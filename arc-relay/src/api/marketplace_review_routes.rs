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
use crate::models::{marketplace, user};

#[derive(Debug, Deserialize)]
pub struct ReviewAction {
    pub reason: Option<String>,
}

/// F3: log full sqlx error server-side, return generic 500 to the client.
fn db_500(op: &'static str) -> impl FnOnce(sqlx::Error) -> (StatusCode, String) {
    move |e| {
        error!(operation = op, error = %e, "marketplace review DB error");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    }
}

/// F3: log internal signing-layer error string, return generic 500.
fn signing_500(op: &'static str) -> impl FnOnce(String) -> (StatusCode, String) {
    move |e| {
        error!(operation = op, error = %e, "marketplace review signing error");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/marketplace/review — list all versions pending review (admin only)
pub async fn list_pending(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
) -> Result<Json<Vec<marketplace::MarketplaceVersion>>, (StatusCode, String)> {
    let admin = user::is_admin(&state.pool, user_id)
        .await
        .map_err(db_500("is_admin"))?;
    if !admin {
        return Err((StatusCode::FORBIDDEN, "Admin access required".to_string()));
    }

    match marketplace::list_pending_review(&state.pool).await {
        Ok(versions) => Ok(Json(versions)),
        Err(e) => Err(db_500("list_pending_review")(e)),
    }
}

/// POST /api/marketplace/review/:id/approve — approve a version (admin only)
pub async fn approve(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(version_id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, String)> {
    let admin = user::is_admin(&state.pool, user_id)
        .await
        .map_err(db_500("is_admin"))?;
    if !admin {
        return Err((StatusCode::FORBIDDEN, "Admin access required".to_string()));
    }

    let updated = marketplace::set_version_status(&state.pool, version_id, "approved")
        .await
        .map_err(db_500("set_version_status_approved"))?;

    if !updated {
        return Err((StatusCode::NOT_FOUND, "Version not found".to_string()));
    }

    // Look up the parent plugin and approve it if this is the first approval
    let version = sqlx::query_as::<_, marketplace::MarketplaceVersion>(
        "SELECT * FROM marketplace_versions WHERE id = $1",
    )
    .bind(version_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(db_500("fetch_version"))?;

    if let Some(ver) = &version {
        marketplace::approve_plugin_if_first(&state.pool, ver.plugin_id)
            .await
            .map_err(db_500("approve_plugin_if_first"))?;
    }

    // Countersign the archive SHA-256
    if let Some(ref ver) = version {
        let server_key = crate::signing::get_or_create_server_key(&state.pool)
            .await
            .map_err(signing_500("get_or_create_server_key"))?;

        let countersig_hex = crate::signing::countersign(&server_key, &ver.archive_sha256)
            .map_err(signing_500("countersign"))?;

        sqlx::query("UPDATE marketplace_versions SET countersig = $2 WHERE id = $1")
            .bind(version_id)
            .bind(&countersig_hex)
            .execute(&state.pool)
            .await
            .map_err(db_500("update_countersig"))?;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/marketplace/review/:id/reject — reject a version (admin only)
pub async fn reject(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(version_id): Path<Uuid>,
    Json(_body): Json<ReviewAction>,
) -> Result<StatusCode, (StatusCode, String)> {
    let admin = user::is_admin(&state.pool, user_id)
        .await
        .map_err(db_500("is_admin"))?;
    if !admin {
        return Err((StatusCode::FORBIDDEN, "Admin access required".to_string()));
    }

    let updated = marketplace::set_version_status(&state.pool, version_id, "rejected")
        .await
        .map_err(db_500("set_version_status_rejected"))?;

    if !updated {
        return Err((StatusCode::NOT_FOUND, "Version not found".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
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
    async fn marketplace_review_requires_auth() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("GET")
            .uri("/api/marketplace/review")
            .body(Body::empty())
            .unwrap();
        let s = app.oneshot(req).await.unwrap().status();
        assert_eq!(
            s,
            StatusCode::UNAUTHORIZED,
            "GET /api/marketplace/review must require auth"
        );
    }
}
