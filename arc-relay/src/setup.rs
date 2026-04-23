use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::auth::{jwt, password};
use crate::models::{team, user};

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SetupRequest {
    pub owner_name: String,
    pub owner_email: String,
    pub team_name: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct SetupResponse {
    pub owner_id: String,
    pub team_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct SetupStatus {
    /// True once at least one owner account exists — setup can no longer be run.
    pub is_configured: bool,
    pub owner_exists: bool,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/setup/status — check if first-run setup has been completed.
pub async fn get_setup_status(State(state): State<AppState>) -> Json<SetupStatus> {
    let owner_exists = any_user_exists(&state.pool).await;
    Json(SetupStatus {
        is_configured: owner_exists,
        owner_exists,
    })
}

/// POST /api/setup — complete first-run setup. Refuses to run if any user exists.
pub async fn complete_setup(
    State(state): State<AppState>,
    Json(req): Json<SetupRequest>,
) -> Result<Json<SetupResponse>, (StatusCode, String)> {
    // Validate inputs
    if req.owner_name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "owner_name is required".to_string(),
        ));
    }
    if req.team_name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "team_name is required".to_string(),
        ));
    }
    if !req.owner_email.contains('@') {
        return Err((
            StatusCode::BAD_REQUEST,
            "owner_email must be a valid email".to_string(),
        ));
    }
    if req.password.len() < 8 {
        return Err((
            StatusCode::BAD_REQUEST,
            "password must be at least 8 characters".to_string(),
        ));
    }

    // Gate: setup is a one-time operation. If any user exists, refuse.
    if any_user_exists(&state.pool).await {
        return Err((
            StatusCode::CONFLICT,
            "setup has already been completed".to_string(),
        ));
    }

    // Hash password, create user, create team
    let password_hash = password::hash_password(&req.password).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to hash password: {e}"),
        )
    })?;

    let owner = user::create_user(
        &state.pool,
        &req.owner_email,
        &password_hash,
        Some(req.owner_name.as_str()),
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create owner: {e}"),
        )
    })?;

    let new_team = team::create_team(&state.pool, &req.team_name, owner.id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create team: {e}"),
            )
        })?;

    // Issue a token pair so the web client can immediately sign in
    let pair = jwt::generate_token_pair(
        owner.id,
        &state.config.jwt_secret,
        state.config.access_token_ttl_secs,
        state.config.refresh_token_ttl_secs,
    )
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to generate tokens: {e}"),
        )
    })?;

    state
        .audit
        .record(
            &owner.id.to_string(),
            "setup.complete",
            &format!("team/{}", new_team.id),
            &req.owner_email,
        )
        .await;

    Ok(Json(SetupResponse {
        owner_id: owner.id.to_string(),
        team_id: new_team.id.to_string(),
        access_token: pair.access_token,
        refresh_token: pair.refresh_token,
        message: format!(
            "Setup complete. Owner '{}' created with team '{}'.",
            req.owner_name, req.team_name
        ),
    }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn any_user_exists(pool: &sqlx::PgPool) -> bool {
    match sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await
    {
        Ok(n) => n > 0,
        Err(e) => {
            tracing::error!(error = %e, "failed to count users");
            // Fail-closed: pretend setup is done, so we don't accidentally
            // allow a second setup run on transient DB errors.
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_status_serializes_correctly() {
        let status = SetupStatus {
            is_configured: false,
            owner_exists: false,
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["is_configured"], false);
        assert_eq!(json["owner_exists"], false);
    }

    #[test]
    fn setup_response_serializes_correctly() {
        let resp = SetupResponse {
            owner_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            team_id: "550e8400-e29b-41d4-a716-446655440001".to_string(),
            access_token: "access.jwt".to_string(),
            refresh_token: "refresh.jwt".to_string(),
            message: "Setup complete.".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["access_token"], "access.jwt");
        assert_eq!(json["refresh_token"], "refresh.jwt");
    }

    #[test]
    fn setup_request_deserializes() {
        let json = r#"{"owner_name":"Alice","owner_email":"alice@example.com","team_name":"Acme","password":"hunter2x"}"#;
        let req: SetupRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.owner_name, "Alice");
        assert_eq!(req.team_name, "Acme");
    }

    // DB-backed tests require DATABASE_URL + a clean users table.

    #[tokio::test]
    #[ignore]
    async fn complete_setup_rejects_short_password() {
        // (Validation runs before DB so this works without a live DB, but
        // we still need AppState, which requires a pool. Construct lazy pool.)
        let url = match std::env::var("DATABASE_URL") {
            Ok(u) => u,
            Err(_) => return,
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let state = AppState {
            pool: pool.clone(),
            config: std::sync::Arc::new(crate::config::ArcConfig::default()),
            relay: std::sync::Arc::new(crate::relay::router::RelayRouter::new()),
            audit: crate::audit::AuditLog::new(),
            mirror_registry: crate::mirror::MirrorRegistry::default(),
            invitations: crate::mirror::invitation::InvitationStore::new(pool),
            r2: None,
        };
        let req = SetupRequest {
            owner_name: "Alice".to_string(),
            owner_email: "alice@example.com".to_string(),
            team_name: "Acme".to_string(),
            password: "short".to_string(),
        };
        let err = complete_setup(State(state), Json(req))
            .await
            .expect_err("short password");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }
}
