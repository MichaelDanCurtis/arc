use axum::{
    extract::{ws::WebSocketUpgrade, Query, State},
    http::StatusCode,
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
/// [`handle_connection`].
pub async fn ws_handler(
    ws: WebSocketUpgrade,
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

    let token = params.token.clone();
    tracing::info!(user_id = %user_id, "WebSocket connection upgrading");
    ws.on_upgrade(move |socket| handle_connection(socket, user_id, token, state))
}

/// Close code sent when the access token has expired mid-connection.
pub const CLOSE_TOKEN_EXPIRED: u16 = 4001;

/// Send a WebSocket close frame with code 4001 (token expired).
pub async fn close_with_expired(socket: &mut axum::extract::ws::WebSocket) {
    use axum::extract::ws::{CloseFrame, Message};
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: CLOSE_TOKEN_EXPIRED.into(),
            reason: "access token expired".into(),
        })))
        .await;
}

/// Return `true` if the given token is expired (or otherwise invalid).
///
/// Uses `decode_token` which validates the signature and expiry with leeway=0.
/// Any error — including `ExpiredSignature` — returns `true`.
pub fn token_is_expired(token: &str, jwt_secret: &str) -> bool {
    decode_token(token, jwt_secret).is_err()
}

/// Perform the first read on a freshly-upgraded WebSocket to determine
/// whether the remote end is a host (HostHello) or a client (AttachSession),
/// then dispatch to the appropriate handler.
async fn handle_connection(
    mut socket: axum::extract::ws::WebSocket,
    user_id: Uuid,
    token: String,
    state: AppState,
) {
    use axum::extract::ws::Message;
    use beacon_proto::codec::decode;
    use beacon_proto::envelope::ClientToArc;
    use tokio::time::{interval, Duration};

    let jwt_secret = state.config.jwt_secret.clone();

    // Check once immediately in case the token expired between upgrade and dispatch.
    if token_is_expired(&token, &jwt_secret) {
        tracing::warn!(user_id = %user_id, "token expired before first message; closing 4001");
        close_with_expired(&mut socket).await;
        return;
    }

    // Wait for the first binary frame, with a periodic expiry re-check.
    let mut expiry_tick = interval(Duration::from_secs(1));
    expiry_tick.tick().await; // consume the immediate first tick

    let first_msg = loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Binary(bytes))) => break bytes,
                    Some(Ok(_)) => {
                        tracing::warn!(user_id = %user_id, "WebSocket first message was not binary; closing");
                        let _ = socket.close().await;
                        return;
                    }
                    Some(Err(e)) => {
                        tracing::warn!(user_id = %user_id, "WebSocket first message error: {e}");
                        return;
                    }
                    None => {
                        tracing::debug!(user_id = %user_id, "WebSocket closed before first message");
                        return;
                    }
                }
            }
            _ = expiry_tick.tick() => {
                if token_is_expired(&token, &jwt_secret) {
                    tracing::warn!(user_id = %user_id, "token expired while waiting for first message; closing 4001");
                    close_with_expired(&mut socket).await;
                    return;
                }
            }
        }
    };

    let msg: ClientToArc = match decode(&first_msg) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(user_id = %user_id, "WebSocket first message decode error: {e}");
            let _ = socket.close().await;
            return;
        }
    };

    match msg {
        hello @ ClientToArc::HostHello { .. } => {
            tracing::info!(user_id = %user_id, "WebSocket dispatching to host handler");
            crate::ws::host::handle_host(
                socket,
                user_id,
                hello,
                state.relay.clone(),
                state.pool.clone(),
                token,
                jwt_secret,
            )
            .await;
        }
        attach @ ClientToArc::AttachSession { .. } => {
            tracing::info!(user_id = %user_id, "WebSocket dispatching to client handler");
            crate::ws::client::handle_client(
                socket,
                user_id,
                attach,
                state.relay.clone(),
                state.pool.clone(),
                token,
                jwt_secret,
            )
            .await;
        }
        other => {
            tracing::warn!(
                user_id = %user_id,
                "WebSocket first message was neither HostHello nor AttachSession: {other:?}"
            );
            let _ = socket.close().await;
        }
    }
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
