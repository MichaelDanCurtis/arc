use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;

use crate::api::AppState;
use crate::auth::middleware::AuthUser;
use crate::models::user;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct UserProfile {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/users/me
///
/// Returns the authenticated user's profile (no password hash exposed).
pub async fn get_me(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
) -> Result<Json<UserProfile>, (StatusCode, String)> {
    let found = user::find_user_by_id(&state.pool, user_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database error: {e}"),
            )
        })?;

    let u = found.ok_or_else(|| (StatusCode::NOT_FOUND, "User not found".to_string()))?;

    Ok(Json(UserProfile {
        id: u.id.to_string(),
        email: u.email,
        name: u.name,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_profile_serializes_with_name() {
        let profile = UserProfile {
            id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            email: "alice@example.com".to_string(),
            name: Some("Alice".to_string()),
        };
        let json = serde_json::to_value(&profile).unwrap();
        assert_eq!(json["id"], "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(json["email"], "alice@example.com");
        assert_eq!(json["name"], "Alice");
        // Verify password_hash is NOT present
        assert!(json.get("password_hash").is_none());
    }

    #[test]
    fn user_profile_serializes_without_name() {
        let profile = UserProfile {
            id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            email: "bob@example.com".to_string(),
            name: None,
        };
        let json = serde_json::to_value(&profile).unwrap();
        assert!(json["name"].is_null());
    }

    #[test]
    fn user_profile_has_no_password_hash_field() {
        // Compile-time check: UserProfile struct must not contain password_hash.
        // This test documents the invariant and will fail to compile if violated.
        let profile = UserProfile {
            id: String::new(),
            email: String::new(),
            name: None,
        };
        let json = serde_json::to_string(&profile).unwrap();
        assert!(!json.contains("password_hash"));
        assert!(!json.contains("password"));
    }

    #[tokio::test]
    #[ignore]
    async fn get_me_returns_profile_for_authenticated_user() {
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
            r2: None,
            arc_session_store: crate::rpc::ArcSessionStore::new(64),
            arc_ai_session_store: crate::ai_rpc::ArcAiSessionStore::new(64),
            arc_ai_backend: None,
        };

        let email = format!("getme_{}@test.invalid", uuid::Uuid::new_v4());
        let created =
            crate::models::user::create_user(&state.pool, &email, "some_hash", Some("GetMe User"))
                .await
                .expect("create user");

        let result = get_me(State(state), AuthUser(created.id)).await;
        let Json(profile) = result.expect("get_me should succeed");
        assert_eq!(profile.id, created.id.to_string());
        assert_eq!(profile.email, email);
        assert_eq!(profile.name.as_deref(), Some("GetMe User"));
    }
}
