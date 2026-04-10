use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use beacon_proto::{
    codec::{decode, encode},
    envelope::{ArcToClient, ClientToArc},
};
use serde::Serialize;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::middleware::AuthUser;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ShadowDiffResponse {
    pub session_id: String,
    pub diff: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/sessions/:id/shadow-diff
///
/// Requests the shadow diff for a session by forwarding to the host via the
/// relay router and waiting for the host's response.
pub async fn get_shadow_diff(
    State(state): State<AppState>,
    AuthUser(_user_id): AuthUser,
    Path(session_id): Path<String>,
) -> Result<Json<ShadowDiffResponse>, (StatusCode, String)> {
    // Verify the session exists in the router
    let info = state.relay.get_session_info(&session_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("Session {session_id} not found or not connected"),
        )
    })?;
    let _ = info; // used for existence check

    // Create a unique request ID for correlation
    let request_id = Uuid::new_v4().to_string();

    // Build the ShadowDiffRequest envelope
    let sid = session_id
        .parse()
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid session ID".to_string()))?;

    let request_msg = ClientToArc::ShadowDiffRequest {
        session_id: sid,
        request_id: request_id.clone(),
    };

    let encoded = encode(&request_msg).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Encode error: {e}"),
        )
    })?;

    // Send the request to the host via the relay router
    if !state.relay.send_to_host(&session_id, encoded).await {
        return Err((
            StatusCode::BAD_GATEWAY,
            "Host not connected for session".to_string(),
        ));
    }

    // Register a oneshot channel to receive the response
    let (tx, rx) = oneshot::channel::<String>();
    state
        .relay
        .register_shadow_diff_waiter(&request_id, tx);

    // Wait for the response with a timeout
    let diff = tokio::time::timeout(std::time::Duration::from_secs(10), rx)
        .await
        .map_err(|_| {
            state.relay.remove_shadow_diff_waiter(&request_id);
            (StatusCode::GATEWAY_TIMEOUT, "Host did not respond in time".to_string())
        })?
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Response channel closed".to_string(),
            )
        })?;

    Ok(Json(ShadowDiffResponse {
        session_id,
        diff,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_diff_response_serializes_correctly() {
        let resp = ShadowDiffResponse {
            session_id: "sess_123".to_string(),
            diff: "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+new".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["session_id"], "sess_123");
        assert!(json["diff"].as_str().unwrap().contains("@@"));
    }

    #[tokio::test]
    async fn shadow_diff_route_requires_auth() {
        use axum::{
            body::Body,
            http::{Request, StatusCode},
        };
        use tower::ServiceExt;

        let state = crate::api::tests::test_state();
        let app = crate::api::build_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/api/sessions/some-session/shadow-diff")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "shadow-diff endpoint must require auth"
        );
    }
}
