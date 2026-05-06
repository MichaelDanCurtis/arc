use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::middleware::AuthUser;
use crate::rpc::emit_host_shadow_diff_request;

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

    // Register a oneshot channel BEFORE sending to the host so that a very
    // fast response cannot arrive and call resolve_shadow_diff before the
    // waiter entry exists (which would cause a spurious timeout).
    let (tx, rx) = oneshot::channel::<String>();
    state
        .relay
        .register_shadow_diff_waiter(&request_id, tx);

    // Phase G: JSON-RPC is the sole transport.  Emit arc.host.shadow_diff_request
    // on the host's notification channel.  If no JSON-RPC host entry exists the
    // host is not connected; clean up and return 502.
    let Some(entry) = state.arc_session_store.get(&session_id).await else {
        state.relay.remove_shadow_diff_waiter(&request_id);
        return Err((
            StatusCode::BAD_GATEWAY,
            "Host not connected for session".to_string(),
        ));
    };
    emit_host_shadow_diff_request(&entry, &request_id);

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
