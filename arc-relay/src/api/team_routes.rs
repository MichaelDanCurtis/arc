use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::auth::middleware::AuthUser;
use crate::models::team;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct TeamResponse {
    pub id: String,
    pub name: String,
    pub owner_id: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateTeamRequest {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct MemberResponse {
    pub user_id: String,
    pub role: String,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

impl From<team::Team> for TeamResponse {
    fn from(t: team::Team) -> Self {
        TeamResponse {
            id: t.id.to_string(),
            name: t.name,
            owner_id: t.owner_id.to_string(),
        }
    }
}

impl From<team::TeamMember> for MemberResponse {
    fn from(m: team::TeamMember) -> Self {
        MemberResponse {
            user_id: m.user_id.to_string(),
            role: m.role,
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/teams
///
/// Returns all teams the authenticated user belongs to.
pub async fn list_teams(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
) -> Result<Json<Vec<TeamResponse>>, (StatusCode, String)> {
    let teams = team::list_teams_for_user(&state.pool, user_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database error: {e}"),
            )
        })?;

    Ok(Json(teams.into_iter().map(TeamResponse::from).collect()))
}

/// POST /api/teams
///
/// Creates a new team owned by the authenticated user.
/// The owner is automatically added as a member with the "owner" role.
pub async fn create_team(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Json(req): Json<CreateTeamRequest>,
) -> Result<(StatusCode, Json<TeamResponse>), (StatusCode, String)> {
    let new_team = team::create_team(&state.pool, &req.name, user_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create team: {e}"),
            )
        })?;

    state
        .audit
        .record(
            &user_id.to_string(),
            "team.create",
            &format!("team/{}", new_team.id),
            &req.name,
        )
        .await;

    Ok((StatusCode::CREATED, Json(TeamResponse::from(new_team))))
}

/// GET /api/teams/:id/members
///
/// Returns the member list for the given team.
pub async fn list_members(
    State(state): State<AppState>,
    AuthUser(_user_id): AuthUser,
    Path(team_id): Path<String>,
) -> Result<Json<Vec<MemberResponse>>, (StatusCode, String)> {
    let team_uuid = team_id.parse::<uuid::Uuid>().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "Invalid team ID format".to_string(),
        )
    })?;

    let members = team::list_team_members(&state.pool, team_uuid)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database error: {e}"),
            )
        })?;

    Ok(Json(
        members.into_iter().map(MemberResponse::from).collect(),
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_response_serializes_correctly() {
        let resp = TeamResponse {
            id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Engineering".to_string(),
            owner_id: "550e8400-e29b-41d4-a716-446655440001".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["id"], "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(json["name"], "Engineering");
        assert_eq!(json["owner_id"], "550e8400-e29b-41d4-a716-446655440001");
        // created_at must not be exposed
        assert!(json.get("created_at").is_none());
    }

    #[test]
    fn member_response_serializes_correctly() {
        let resp = MemberResponse {
            user_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            role: "owner".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["user_id"], "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(json["role"], "owner");
        assert!(json.get("team_id").is_none(), "team_id must not be exposed");
    }

    #[test]
    fn create_team_request_deserializes() {
        let json = r#"{"name":"Platform Team"}"#;
        let req: CreateTeamRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "Platform Team");
    }

    #[test]
    fn list_members_rejects_invalid_uuid() {
        // Verify the parse step will catch malformed UUIDs before hitting the DB.
        let invalid = "not-a-uuid";
        let result = invalid.parse::<uuid::Uuid>();
        assert!(result.is_err(), "non-UUID team ID should fail to parse");
    }

    #[tokio::test]
    #[ignore]
    async fn create_team_returns_201_and_owner_is_member() {
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
            invitations: crate::mirror::invitation::InvitationStore::new(pool.clone()),
            r2: None,
            arc_session_store: crate::rpc::ArcSessionStore::new(64),
            arc_ai_session_store: crate::ai_rpc::ArcAiSessionStore::new(64),
            arc_ai_backend: None,
        };

        let owner_id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x')")
            .bind(owner_id)
            .bind(format!("team_create_{}@test.invalid", owner_id))
            .execute(&pool)
            .await
            .expect("user");

        let req = CreateTeamRequest {
            name: "My Team".to_string(),
        };
        let (status, Json(resp)) = create_team(State(state.clone()), AuthUser(owner_id), Json(req))
            .await
            .expect("create_team");
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(resp.name, "My Team");
        assert_eq!(resp.owner_id, owner_id.to_string());

        // Verify owner appears in member list
        let Json(members) = list_members(State(state), AuthUser(owner_id), Path(resp.id.clone()))
            .await
            .expect("list_members");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].user_id, owner_id.to_string());
        assert_eq!(members[0].role, "owner");
    }

    #[tokio::test]
    #[ignore]
    async fn list_teams_returns_teams_for_user() {
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
            invitations: crate::mirror::invitation::InvitationStore::new(pool.clone()),
            r2: None,
            arc_session_store: crate::rpc::ArcSessionStore::new(64),
            arc_ai_session_store: crate::ai_rpc::ArcAiSessionStore::new(64),
            arc_ai_backend: None,
        };

        let owner_id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x')")
            .bind(owner_id)
            .bind(format!("team_list_{}@test.invalid", owner_id))
            .execute(&pool)
            .await
            .expect("user");

        // Create two teams
        let req_a = CreateTeamRequest {
            name: "Alpha".to_string(),
        };
        let req_b = CreateTeamRequest {
            name: "Beta".to_string(),
        };
        let _ = create_team(State(state.clone()), AuthUser(owner_id), Json(req_a))
            .await
            .expect("team a");
        let _ = create_team(State(state.clone()), AuthUser(owner_id), Json(req_b))
            .await
            .expect("team b");

        let Json(teams) = list_teams(State(state), AuthUser(owner_id))
            .await
            .expect("list_teams");
        assert!(teams.len() >= 2);
    }
}
