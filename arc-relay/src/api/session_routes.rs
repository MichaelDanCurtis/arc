use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;

use crate::api::AppState;
use crate::auth::middleware::AuthUser;
use crate::models::session;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub id: String,
    pub name: Option<String>,
    pub state: String,
    pub started_at: String,
    pub last_seen_at: String,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

impl From<session::DbSession> for SessionResponse {
    fn from(s: session::DbSession) -> Self {
        SessionResponse {
            id: s.id,
            name: s.name,
            state: s.state,
            started_at: s.started_at.to_rfc3339(),
            last_seen_at: s.last_seen_at.to_rfc3339(),
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/sessions
///
/// Returns all sessions owned by the authenticated user.
pub async fn list_sessions(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
) -> Result<Json<Vec<SessionResponse>>, (StatusCode, String)> {
    let sessions = session::list_sessions_for_user(&state.pool, user_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database error: {e}"),
            )
        })?;

    Ok(Json(
        sessions.into_iter().map(SessionResponse::from).collect(),
    ))
}

/// GET /api/sessions/:id
///
/// Returns a single session by ID.
/// Note: does not currently enforce that the session belongs to the requesting
/// user; callers must supply a valid `AuthUser` to reach this handler.
pub async fn get_session(
    State(state): State<AppState>,
    AuthUser(_user_id): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<SessionResponse>, (StatusCode, String)> {
    let found = session::find_session(&state.pool, &id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Database error: {e}"),
        )
    })?;

    let sess = found.ok_or_else(|| (StatusCode::NOT_FOUND, "Session not found".to_string()))?;

    Ok(Json(SessionResponse::from(sess)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_response_serializes_correctly() {
        let resp = SessionResponse {
            id: "sess_abc123".to_string(),
            name: Some("My session".to_string()),
            state: "active".to_string(),
            started_at: "2026-01-01T00:00:00+00:00".to_string(),
            last_seen_at: "2026-01-01T01:00:00+00:00".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["id"], "sess_abc123");
        assert_eq!(json["name"], "My session");
        assert_eq!(json["state"], "active");
        assert!(json.get("host_id").is_none(), "host_id must not be exposed");
        assert!(
            json.get("owner_id").is_none(),
            "owner_id must not be exposed"
        );
    }

    #[test]
    fn session_response_without_name_serializes_null() {
        let resp = SessionResponse {
            id: "sess_xyz".to_string(),
            name: None,
            state: "ended".to_string(),
            started_at: "2026-01-01T00:00:00+00:00".to_string(),
            last_seen_at: "2026-01-01T00:30:00+00:00".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["name"].is_null());
    }

    #[tokio::test]
    #[ignore]
    async fn list_sessions_returns_owned_sessions() {
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
        };

        // Create a user
        let owner_id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x')")
            .bind(owner_id)
            .bind(format!("sess_list_{}@test.invalid", owner_id))
            .execute(&state.pool)
            .await
            .expect("user");

        // Create two sessions
        let sid1 = format!("list_a_{}", uuid::Uuid::new_v4().simple());
        let sid2 = format!("list_b_{}", uuid::Uuid::new_v4().simple());
        session::create_session(&state.pool, &sid1, uuid::Uuid::new_v4(), owner_id, None)
            .await
            .expect("sess1");
        session::create_session(
            &state.pool,
            &sid2,
            uuid::Uuid::new_v4(),
            owner_id,
            Some("Named"),
        )
        .await
        .expect("sess2");

        let Json(sessions) = list_sessions(State(state), AuthUser(owner_id))
            .await
            .expect("list_sessions");
        assert!(sessions.len() >= 2);
    }

    #[tokio::test]
    #[ignore]
    async fn get_session_returns_404_for_missing_session() {
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
        };

        let err = get_session(
            State(state),
            AuthUser(uuid::Uuid::new_v4()),
            Path("nonexistent_session_id".to_string()),
        )
        .await
        .expect_err("should be 404");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }
}
