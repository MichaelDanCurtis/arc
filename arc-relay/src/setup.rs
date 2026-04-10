use axum::{http::StatusCode, Json};
use serde::{Deserialize, Serialize};

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
    pub token: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct SetupStatus {
    pub is_configured: bool,
    pub owner_exists: bool,
}

/// GET /api/setup/status — check if first-run setup has been completed
pub async fn get_setup_status() -> Json<SetupStatus> {
    // TODO: Check DB for existing owner
    Json(SetupStatus {
        is_configured: false,
        owner_exists: false,
    })
}

/// POST /api/setup — complete first-run setup
pub async fn complete_setup(
    Json(req): Json<SetupRequest>,
) -> Result<Json<SetupResponse>, StatusCode> {
    // Validate
    if req.owner_name.is_empty() || req.password.len() < 8 {
        return Err(StatusCode::BAD_REQUEST);
    }

    let owner_id = format!("owner-{}", uuid_stub());
    let team_id = format!("team-{}", uuid_stub());
    let token = format!("tok-{}", uuid_stub());

    // TODO: Create owner in DB, create team, generate real token

    Ok(Json(SetupResponse {
        owner_id,
        team_id,
        token,
        message: format!(
            "Setup complete. Owner '{}' created with team '{}'.",
            req.owner_name, req.team_name
        ),
    }))
}

fn uuid_stub() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:016x}", nanos)
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
            owner_id: "owner-abc".to_string(),
            team_id: "team-xyz".to_string(),
            token: "tok-123".to_string(),
            message: "Setup complete.".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["owner_id"], "owner-abc");
        assert_eq!(json["team_id"], "team-xyz");
        assert_eq!(json["token"], "tok-123");
    }

    #[tokio::test]
    async fn complete_setup_rejects_short_password() {
        let req = SetupRequest {
            owner_name: "Alice".to_string(),
            owner_email: "alice@example.com".to_string(),
            team_name: "Acme".to_string(),
            password: "short".to_string(), // < 8 chars
        };
        let result = complete_setup(Json(req)).await;
        assert_eq!(result.unwrap_err(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn complete_setup_rejects_empty_owner_name() {
        let req = SetupRequest {
            owner_name: "".to_string(),
            owner_email: "alice@example.com".to_string(),
            team_name: "Acme".to_string(),
            password: "long_enough_password".to_string(),
        };
        let result = complete_setup(Json(req)).await;
        assert_eq!(result.unwrap_err(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn complete_setup_succeeds_with_valid_input() {
        let req = SetupRequest {
            owner_name: "Alice".to_string(),
            owner_email: "alice@example.com".to_string(),
            team_name: "Acme".to_string(),
            password: "securepassword".to_string(),
        };
        let Json(resp) = complete_setup(Json(req)).await.unwrap();
        assert!(resp.owner_id.starts_with("owner-"));
        assert!(resp.team_id.starts_with("team-"));
        assert!(resp.token.starts_with("tok-"));
        assert!(resp.message.contains("Alice"));
        assert!(resp.message.contains("Acme"));
    }

    #[tokio::test]
    async fn get_setup_status_returns_unconfigured() {
        let Json(status) = get_setup_status().await;
        assert!(!status.is_configured);
        assert!(!status.owner_exists);
    }
}
