use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::auth::{jwt, password};
use crate::models::user;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub user_id: String,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/auth/register
///
/// Creates a new user account and returns a fresh token pair.
pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<AuthResponse>), (StatusCode, String)> {
    // Hash password
    let password_hash = password::hash_password(&req.password).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to hash password: {e}"),
        )
    })?;

    // Create user
    let new_user = user::create_user(&state.pool, &req.email, &password_hash, req.name.as_deref())
        .await
        .map_err(|e| {
            // Surface duplicate-email as 409 Conflict, everything else as 500.
            let status = if e.to_string().contains("duplicate key")
                || e.to_string().contains("unique constraint")
            {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, format!("Failed to create user: {e}"))
        })?;

    // Generate token pair
    let pair = jwt::generate_token_pair(
        new_user.id,
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
            &new_user.id.to_string(),
            "user.register",
            &format!("user/{}", new_user.id),
            &req.email,
        )
        .await;

    Ok((
        StatusCode::CREATED,
        Json(AuthResponse {
            user_id: new_user.id.to_string(),
            access_token: pair.access_token,
            refresh_token: pair.refresh_token,
        }),
    ))
}

/// POST /api/auth/login
///
/// Authenticates an existing user and returns a fresh token pair.
pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, (StatusCode, String)> {
    // Find user by email
    let found = user::find_user_by_email(&state.pool, &req.email)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database error: {e}"),
            )
        })?;

    let existing = found.ok_or_else(|| {
        // Return 401 (not 404) to avoid leaking whether the email exists.
        (
            StatusCode::UNAUTHORIZED,
            "Invalid email or password".to_string(),
        )
    })?;

    // Verify password
    let hash = existing.password_hash.as_deref().ok_or_else(|| {
        (StatusCode::UNAUTHORIZED, "This account uses OAuth login".to_string())
    })?;
    let valid = password::verify_password(&req.password, hash).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Password verification error: {e}"),
        )
    })?;

    if !valid {
        state
            .audit
            .record(
                &existing.id.to_string(),
                "user.login.failed",
                &format!("user/{}", existing.id),
                &req.email,
            )
            .await;
        return Err((
            StatusCode::UNAUTHORIZED,
            "Invalid email or password".to_string(),
        ));
    }

    // Generate token pair
    let pair = jwt::generate_token_pair(
        existing.id,
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
            &existing.id.to_string(),
            "user.login",
            &format!("user/{}", existing.id),
            &req.email,
        )
        .await;

    Ok(Json(AuthResponse {
        user_id: existing.id.to_string(),
        access_token: pair.access_token,
        refresh_token: pair.refresh_token,
    }))
}

/// POST /api/auth/refresh
///
/// Exchanges a valid refresh token for a new token pair.
pub async fn refresh(
    State(state): State<AppState>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<AuthResponse>, (StatusCode, String)> {
    // Decode the supplied token (validates signature + expiry)
    let claims = jwt::decode_token(&req.refresh_token, &state.config.jwt_secret).map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            format!("Invalid or expired refresh token: {e}"),
        )
    })?;

    // Verify it's a refresh token (not an access token)
    if claims.kind != jwt::TokenKind::Refresh {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Provided token is not a refresh token".to_string(),
        ));
    }

    // Parse the user ID from the subject claim
    let user_id = claims.sub.parse::<uuid::Uuid>().map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            format!("Invalid user ID in token: {e}"),
        )
    })?;

    // Issue a new token pair
    let pair = jwt::generate_token_pair(
        user_id,
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

    Ok(Json(AuthResponse {
        user_id: user_id.to_string(),
        access_token: pair.access_token,
        refresh_token: pair.refresh_token,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── serde round-trip tests (no DB needed) ────────────────────────────────

    #[test]
    fn register_request_deserializes_with_name() {
        let json = r#"{"email":"alice@example.com","password":"hunter2","name":"Alice"}"#;
        let req: RegisterRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.email, "alice@example.com");
        assert_eq!(req.password, "hunter2");
        assert_eq!(req.name.as_deref(), Some("Alice"));
    }

    #[test]
    fn register_request_deserializes_without_name() {
        let json = r#"{"email":"bob@example.com","password":"secret"}"#;
        let req: RegisterRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.email, "bob@example.com");
        assert!(req.name.is_none());
    }

    #[test]
    fn login_request_deserializes() {
        let json = r#"{"email":"charlie@example.com","password":"pass123"}"#;
        let req: LoginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.email, "charlie@example.com");
        assert_eq!(req.password, "pass123");
    }

    #[test]
    fn refresh_request_deserializes() {
        let json = r#"{"refresh_token":"some.jwt.token"}"#;
        let req: RefreshRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.refresh_token, "some.jwt.token");
    }

    #[test]
    fn auth_response_serializes() {
        let resp = AuthResponse {
            user_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            access_token: "access.jwt".to_string(),
            refresh_token: "refresh.jwt".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["user_id"], "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(json["access_token"], "access.jwt");
        assert_eq!(json["refresh_token"], "refresh.jwt");
    }

    // ── DB-backed integration tests (require DATABASE_URL) ───────────────────

    #[tokio::test]
    #[ignore]
    async fn register_creates_user_and_returns_201() {
        // Requires: DATABASE_URL set + running Postgres with migrations applied.
        let url = match std::env::var("DATABASE_URL") {
            Ok(u) => u,
            Err(_) => return,
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let config = std::sync::Arc::new(crate::config::ArcConfig::default());
        let relay = std::sync::Arc::new(crate::relay::router::RelayRouter::new());
        let state = AppState {
            pool: pool.clone(),
            config,
            relay,
            audit: crate::audit::AuditLog::new(),
            mirror_registry: crate::mirror::MirrorRegistry::default(),
            invitations: crate::mirror::invitation::InvitationStore::new(pool),
        };

        let email = format!("register_{}@test.invalid", uuid::Uuid::new_v4());
        let req = RegisterRequest {
            email: email.clone(),
            password: "test-password-123".to_string(),
            name: Some("Test User".to_string()),
        };

        let result = register(State(state), Json(req)).await;
        let (status, Json(resp)) = result.expect("register should succeed");
        assert_eq!(status, StatusCode::CREATED);
        assert!(!resp.access_token.is_empty());
        assert!(!resp.refresh_token.is_empty());
        assert!(!resp.user_id.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn login_with_valid_credentials_returns_tokens() {
        let url = match std::env::var("DATABASE_URL") {
            Ok(u) => u,
            Err(_) => return,
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let config = std::sync::Arc::new(crate::config::ArcConfig::default());
        let relay = std::sync::Arc::new(crate::relay::router::RelayRouter::new());
        let state = AppState {
            pool: pool.clone(),
            config: config.clone(),
            relay,
            audit: crate::audit::AuditLog::new(),
            mirror_registry: crate::mirror::MirrorRegistry::default(),
            invitations: crate::mirror::invitation::InvitationStore::new(pool.clone()),
        };

        // First register a user
        let email = format!("login_{}@test.invalid", uuid::Uuid::new_v4());
        let pw = "login-test-password";
        let reg = RegisterRequest {
            email: email.clone(),
            password: pw.to_string(),
            name: None,
        };
        let _ = register(State(state.clone()), Json(reg))
            .await
            .expect("register");

        // Now login
        let login_req = LoginRequest {
            email: email.clone(),
            password: pw.to_string(),
        };
        let Json(resp) = login(State(state), Json(login_req))
            .await
            .expect("login should succeed");
        assert!(!resp.access_token.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn login_with_wrong_password_returns_401() {
        let url = match std::env::var("DATABASE_URL") {
            Ok(u) => u,
            Err(_) => return,
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect");
        let config = std::sync::Arc::new(crate::config::ArcConfig::default());
        let relay = std::sync::Arc::new(crate::relay::router::RelayRouter::new());
        let state = AppState {
            pool: pool.clone(),
            config: config.clone(),
            relay,
            audit: crate::audit::AuditLog::new(),
            mirror_registry: crate::mirror::MirrorRegistry::default(),
            invitations: crate::mirror::invitation::InvitationStore::new(pool.clone()),
        };

        let email = format!("badpw_{}@test.invalid", uuid::Uuid::new_v4());
        let reg = RegisterRequest {
            email: email.clone(),
            password: "correct-password".to_string(),
            name: None,
        };
        let _ = register(State(state.clone()), Json(reg))
            .await
            .expect("register");

        let login_req = LoginRequest {
            email,
            password: "wrong-password".to_string(),
        };
        let err = login(State(state), Json(login_req))
            .await
            .expect_err("should fail with wrong password");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }
}
