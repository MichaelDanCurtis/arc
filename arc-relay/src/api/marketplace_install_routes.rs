use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use tracing::error;
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::middleware::AuthUser;
use crate::models::marketplace;

#[derive(Debug, Deserialize)]
pub struct InstallRequest {
    pub plugin_id: Uuid,
    pub version: String,
    pub team_id: Uuid,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/marketplace/install — install a plugin version to a team
pub async fn install(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Json(body): Json<InstallRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    match marketplace::install_to_team(
        &state.pool,
        body.plugin_id,
        &body.version,
        body.team_id,
        user_id,
    )
    .await
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(sqlx::Error::RowNotFound) => Err((
            StatusCode::NOT_FOUND,
            "Plugin or version not found (must be approved)".to_string(),
        )),
        Err(e) => {
            // F3: log full error server-side, return generic 500 to client
            error!(operation = "install_to_team", error = %e, "marketplace DB error");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ))
        }
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
    async fn marketplace_install_requires_auth() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/api/marketplace/install")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"plugin_id":"00000000-0000-0000-0000-000000000000","version":"1.0.0","team_id":"00000000-0000-0000-0000-000000000000"}"#))
            .unwrap();
        let s = app.oneshot(req).await.unwrap().status();
        assert_eq!(
            s,
            StatusCode::UNAUTHORIZED,
            "POST /api/marketplace/install must require auth"
        );
    }

    #[tokio::test]
    async fn marketplace_org_allowlist_requires_auth() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("GET")
            .uri("/api/marketplace/org/00000000-0000-0000-0000-000000000000/allowlist")
            .body(Body::empty())
            .unwrap();
        let s = app.oneshot(req).await.unwrap().status();
        assert_eq!(
            s,
            StatusCode::UNAUTHORIZED,
            "GET /api/marketplace/org/:org_id/allowlist must require auth"
        );
    }
}
