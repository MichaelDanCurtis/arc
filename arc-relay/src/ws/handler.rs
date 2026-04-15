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
    use beacon_proto::codec::{decode, encode};
    use beacon_proto::envelope::ClientToArc;
    use tokio::time::{interval, MissedTickBehavior};

    let jwt_secret = state.config.jwt_secret.clone();

    // Check once immediately in case the token expired between upgrade and dispatch.
    match token_decode_status(&token, &jwt_secret) {
        TokenStatus::Valid => {}
        TokenStatus::Expired => {
            tracing::info!(user_id = %user_id, "WS closing before first message: access token expired");
            close_with_code(&mut socket, CLOSE_TOKEN_EXPIRED, "access token expired").await;
            return;
        }
        TokenStatus::Invalid => {
            tracing::warn!(user_id = %user_id, "WS closing before first message: token decode failed (signature/malformed)");
            close_with_code(&mut socket, CLOSE_TOKEN_INVALID, "token rejected").await;
            return;
        }
    }

    // Wait for the first binary frame, with a periodic expiry re-check.
    let mut expiry_tick = interval(TOKEN_REVALIDATE_INTERVAL);
    expiry_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
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
                match token_decode_status(&token, &jwt_secret) {
                    TokenStatus::Valid => {}
                    TokenStatus::Expired => {
                        tracing::info!(user_id = %user_id, "WS closing: access token expired");
                        close_with_code(&mut socket, CLOSE_TOKEN_EXPIRED, "access token expired").await;
                        return;
                    }
                    TokenStatus::Invalid => {
                        tracing::warn!(user_id = %user_id, "WS closing: token decode failed (signature/malformed)");
                        close_with_code(&mut socket, CLOSE_TOKEN_INVALID, "token rejected").await;
                        return;
                    }
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
                state.mirror_registry.clone(),
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
                state.clone(),
            )
            .await;
        }
        ClientToArc::SubscribeSession { session_id, since_seq } => {
            tracing::info!(user_id = %user_id, session_id = %session_id, "WebSocket dispatching to mirror-subscribe handler");

            // Look up the user's email from DB (not in JWT claims).
            let user_email: String = match sqlx::query_scalar::<_, String>(
                "SELECT email FROM users WHERE id = $1",
            )
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await
            {
                Ok(Some(e)) => e,
                Ok(None) => {
                    tracing::warn!(user_id = %user_id, "SubscribeSession: user not found in DB");
                    let _ = socket.close().await;
                    return;
                }
                Err(e) => {
                    tracing::error!(user_id = %user_id, "SubscribeSession: DB error looking up email: {e}");
                    let _ = socket.close().await;
                    return;
                }
            };

            // Create the watcher channel. The tx goes into Fanout; we drive the rx here.
            let (sink_tx, mut sink_rx) = crate::mirror::fanout::Fanout::make_channel();

            // Sanctioned deviation: device_label hardcoded as "Watcher" for Task 2.8.
            // Task 2.10+ will add it to the SubscribeSession wire variant.
            let device_label = "Watcher";
            let session_id_str = session_id.to_string();

            match crate::mirror::handlers::handle_subscribe(
                &state,
                user_id,
                &user_email,
                device_label,
                &session_id_str,
                since_seq,
                sink_tx,
            )
            .await
            {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(user_id = %user_id, "SubscribeSession denied/failed: {e}");
                    let _ = socket.close().await;
                    return;
                }
            }

            // Forward frames from the fanout channel to the WebSocket.
            // Task 2.10: dispatch inbound control frames from the watcher.
            // Sanctioned deviation: device_label still hardcoded as "Watcher"
            // (envelope.rs RequestControl has no device_label field yet).
            loop {
                tokio::select! {
                    frame = sink_rx.recv() => {
                        match frame {
                            Some(msg) => {
                                match encode(&msg) {
                                    Ok(bytes) => {
                                        if socket.send(Message::Binary(bytes)).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!("SubscribeSession: encode error: {e}");
                                        break;
                                    }
                                }
                            }
                            None => {
                                // Channel closed (watcher evicted by fanout).
                                let _ = socket.close().await;
                                break;
                            }
                        }
                    }
                    inbound = socket.recv() => {
                        match inbound {
                            Some(Ok(Message::Binary(bytes))) => {
                                match decode::<ClientToArc>(&bytes) {
                                    Err(e) => {
                                        tracing::warn!(
                                            user_id = %user_id,
                                            session_id = %session_id_str,
                                            "SubscribeSession: decode error on inbound frame: {e}"
                                        );
                                    }
                                    Ok(ClientToArc::RequestControl { session_id: req_sid }) => {
                                        let sid_str = req_sid.to_string();
                                        let _ = crate::mirror::handlers::handle_request_control(
                                            &state,
                                            user_id,
                                            device_label,
                                            &sid_str,
                                        ).await;
                                    }
                                    Ok(ClientToArc::ApproveControl { session_id: req_sid, request_id }) => {
                                        let sid_str = req_sid.to_string();
                                        let _ = crate::mirror::handlers::handle_approve_control(
                                            &state,
                                            user_id,
                                            &request_id,
                                            &sid_str,
                                        ).await;
                                    }
                                    Ok(ClientToArc::DenyControl { session_id: req_sid, request_id }) => {
                                        let sid_str = req_sid.to_string();
                                        let _ = crate::mirror::handlers::handle_deny_control(
                                            &state,
                                            user_id,
                                            &request_id,
                                            &sid_str,
                                        ).await;
                                    }
                                    Ok(ClientToArc::RevokeControl { session_id: req_sid }) => {
                                        let sid_str = req_sid.to_string();
                                        let _ = crate::mirror::handlers::handle_revoke_control(
                                            &state,
                                            user_id,
                                            &sid_str,
                                        ).await;
                                    }
                                    Ok(ClientToArc::SessionInput { session_id: req_sid, bytes: input_bytes }) => {
                                        let sid_str = req_sid.to_string();
                                        let _ = crate::mirror::handlers::handle_session_input(
                                            &state,
                                            user_id,
                                            device_label,
                                            &sid_str,
                                            input_bytes,
                                        ).await;
                                    }
                                    Ok(ClientToArc::UnsubscribeSession { .. }) => {
                                        // Clean unsubscribe: exit the pump loop without
                                        // closing the socket (caller's outer handler does that).
                                        tracing::info!(
                                            user_id = %user_id,
                                            session_id = %session_id_str,
                                            "watcher sent UnsubscribeSession; exiting pump"
                                        );
                                        break;
                                    }
                                    Ok(ClientToArc::Pong) => {
                                        tracing::trace!(user_id = %user_id, "watcher pong");
                                    }
                                    Ok(other) => {
                                        tracing::warn!(
                                            user_id = %user_id,
                                            session_id = %session_id_str,
                                            "SubscribeSession: unexpected inbound variant: {other:?}"
                                        );
                                    }
                                }
                            }
                            Some(Ok(Message::Close(_))) => {
                                tracing::info!(
                                    user_id = %user_id,
                                    session_id = %session_id_str,
                                    "watcher sent Close frame"
                                );
                                break;
                            }
                            Some(Ok(_)) => {
                                // Text / ping / pong frames — ignore.
                            }
                            Some(Err(e)) => {
                                tracing::warn!(
                                    user_id = %user_id,
                                    session_id = %session_id_str,
                                    "SubscribeSession: socket recv error: {e}"
                                );
                                break;
                            }
                            None => {
                                tracing::debug!(
                                    user_id = %user_id,
                                    session_id = %session_id_str,
                                    "SubscribeSession: socket closed by watcher"
                                );
                                break;
                            }
                        }
                    }
                }
            }
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
