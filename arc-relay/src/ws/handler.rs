use axum::{
    extract::{ws::WebSocketUpgrade, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::jwt::{decode_token, TokenKind};

/// Query parameters expected on the WebSocket upgrade request.
///
/// WebSocket connections cannot carry custom request headers in most browsers,
/// so the JWT is delivered as a plain query parameter instead.
#[derive(Debug, Deserialize)]
pub struct WsParams {
    pub token: String,
}

/// GET /ws — WebSocket upgrade handler.
///
/// Decodes and validates the bearer token from the `token` query parameter,
/// then upgrades the HTTP connection to a WebSocket and dispatches to
/// [`crate::ws::jsonrpc_handler::handle_jsonrpc_connection`].
///
/// The `Sec-WebSocket-Protocol: arc.json-rpc` header is **mandatory** since
/// Phase G (2026-05-05). Connections without it are rejected with `400
/// Bad Request`. All production clients (Mac hostd + arc-web) send this
/// header since Phase B / Phase E respectively.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(params): Query<WsParams>,
) -> Response {
    // Validate JWT before accepting the upgrade.
    let claims = match decode_token(&params.token, &state.config.jwt_secret) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("WebSocket auth rejected: {e}");
            return (StatusCode::UNAUTHORIZED, "Invalid or expired token").into_response();
        }
    };

    // Only access tokens are valid here; refresh tokens must never reach WS.
    if claims.kind != TokenKind::Access {
        tracing::warn!("WebSocket auth rejected: refresh token used");
        return (
            StatusCode::UNAUTHORIZED,
            "Refresh tokens cannot be used for WebSocket access",
        )
            .into_response();
    }

    let user_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("WebSocket auth: invalid sub UUID in token: {e}");
            return (StatusCode::UNAUTHORIZED, "Invalid user ID in token").into_response();
        }
    };

    let wants_jsonrpc = headers
        .get("Sec-WebSocket-Protocol")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').any(|p| p.trim() == "arc.json-rpc"))
        .unwrap_or(false);

    if !wants_jsonrpc {
        tracing::warn!(user_id = %user_id, "WebSocket rejected: arc.json-rpc subprotocol required");
        return (
            StatusCode::BAD_REQUEST,
            "arc.json-rpc subprotocol required",
        )
            .into_response();
    }

    let token = params.token.clone();
    tracing::info!(user_id = %user_id, "WebSocket arc.json-rpc connection upgrading");

    ws.protocols(["arc.json-rpc"])
        .on_upgrade(move |socket| {
            crate::ws::jsonrpc_handler::handle_jsonrpc_connection(
                socket, user_id, token, state,
            )
        })
}

/// Close code sent when the access token has expired mid-connection.
/// Client should refresh and reconnect.
pub const CLOSE_TOKEN_EXPIRED: u16 = 4001;

/// Close code sent when the token is rejected for a non-expiry reason
/// (bad signature, malformed, secret rotated, etc.).
/// Client should re-authenticate from scratch (login flow), not refresh.
pub const CLOSE_TOKEN_INVALID: u16 = 4003;

/// How often the connection loops re-decode the bearer token to detect expiry.
///
/// Production: 30 s is well under the 15-min access-token lifetime, so an
/// expiring token is detected promptly without burning CPU.
///
/// The `fast-revalidate` Cargo feature drops this to 1 s so that integration
/// tests (which mint 2-second tokens and sleep 4 s) finish quickly.
#[cfg(not(feature = "fast-revalidate"))]
pub const TOKEN_REVALIDATE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

#[cfg(feature = "fast-revalidate")]
pub const TOKEN_REVALIDATE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Result of decoding a bearer token during a live connection re-check.
#[derive(Debug, PartialEq, Eq)]
pub enum TokenStatus {
    /// Token is valid and not yet expired.
    Valid,
    /// Token signature and structure are fine but the expiry has passed.
    /// The client should refresh its access token and reconnect.
    Expired,
    /// Token is structurally invalid or has a bad signature.
    /// A refresh will not help; the client must re-authenticate from scratch.
    Invalid,
}

/// Decode `token` and classify the outcome into a [`TokenStatus`].
pub fn token_decode_status(token: &str, jwt_secret: &str) -> TokenStatus {
    use jsonwebtoken::errors::ErrorKind;
    match decode_token(token, jwt_secret) {
        Ok(_) => TokenStatus::Valid,
        Err(e) => match e.kind() {
            ErrorKind::ExpiredSignature => TokenStatus::Expired,
            _ => TokenStatus::Invalid,
        },
    }
}

/// Send a WebSocket close frame with an arbitrary code and reason string.
pub async fn close_with_code(
    socket: &mut axum::extract::ws::WebSocket,
    code: u16,
    reason: &str,
) {
    use axum::extract::ws::{CloseFrame, Message};
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.to_owned().into(),
        })))
        .await;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// serde_json is available; verify WsParams deserializes from a JSON object
    /// (same field names as the query-string struct).
    #[test]
    fn ws_params_deserializes_token() {
        let params: WsParams = serde_json::from_str(r#"{"token":"abc123"}"#).expect("parse");
        assert_eq!(params.token, "abc123");
    }

    #[test]
    fn ws_params_empty_string_token() {
        let params: WsParams = serde_json::from_str(r#"{"token":""}"#).expect("parse");
        assert_eq!(params.token, "");
    }

    #[test]
    fn ws_params_missing_token_fails() {
        let result: Result<WsParams, _> = serde_json::from_str(r#"{"other":"value"}"#);
        assert!(result.is_err(), "should fail when token field is absent");
    }
}
