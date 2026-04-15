//! Shared integration-test helpers for arc-relay.
//!
//! Usage in an integration test file:
//! ```ignore
//! mod common;
//! use common::*;
//! ```
//!
//! All helpers that require a live Postgres instance skip gracefully when
//! `DATABASE_URL` is not set.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message, WebSocketStream};
use uuid::Uuid;

use arc_relay::api::{build_router, AppState};
use arc_relay::audit::AuditLog;
use arc_relay::config::ArcConfig;
use arc_relay::mirror::invitation::InvitationStore;
use arc_relay::mirror::MirrorRegistry;
use arc_relay::relay::router::RelayRouter;
use beacon_proto::codec::{decode, encode};
use beacon_proto::envelope::{ArcToClient, ClientToArc};

// ---------------------------------------------------------------------------
// Relay handle
// ---------------------------------------------------------------------------

/// A running in-process relay bound to a random port.
pub struct Relay {
    pub addr: std::net::SocketAddr,
    #[allow(dead_code)]
    pub jwt_secret: String,
}

impl Relay {
    pub fn ws_url(&self, token: &str) -> String {
        format!("ws://127.0.0.1:{}/ws?token={}", self.addr.port(), token)
    }

    #[allow(dead_code)]
    pub fn http_url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.addr.port(), path)
    }
}

/// Spin up a full axum relay on a random port backed by the `DATABASE_URL`
/// environment variable.  Returns `None` if `DATABASE_URL` is not set.
pub async fn try_spawn_relay() -> Option<Relay> {
    let db_url = std::env::var("DATABASE_URL").ok()?;
    let pool = sqlx::PgPool::connect(&db_url).await.ok()?;

    // Run migrations so the schema is up to date.
    sqlx::migrate!("./migrations").run(&pool).await.ok()?;

    let jwt_secret = "test-secret-that-is-long-enough-32x".to_string();
    let mut config = ArcConfig::default();
    config.jwt_secret = jwt_secret.clone();

    let state = AppState {
        pool: pool.clone(),
        config: Arc::new(config),
        relay: Arc::new(RelayRouter::new()),
        audit: AuditLog::new(),
        mirror_registry: MirrorRegistry::default(),
        invitations: InvitationStore::new(pool),
    };

    let router = build_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.ok()?;
    let addr = listener.local_addr().ok()?;

    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("axum serve");
    });

    Some(Relay { addr, jwt_secret })
}

// ---------------------------------------------------------------------------
// User credentials
// ---------------------------------------------------------------------------

/// Credentials returned by `create_user`.
pub struct UserCreds {
    pub user_id: Uuid,
    #[allow(dead_code)]
    pub email: String,
    pub access_token: String,
}

/// Register a user via the HTTP API and return credentials.
/// Uses a raw TCP connection so we don't need `reqwest` as a dev-dep.
pub async fn create_user(relay: &Relay, email: &str) -> UserCreds {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let body = serde_json::json!({
        "email": email,
        "password": "test-password-123",
        "name": "Test User"
    })
    .to_string();

    let request = format!(
        "POST /api/auth/register HTTP/1.1\r\n\
         Host: 127.0.0.1:{}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        relay.addr.port(),
        body.len(),
        body
    );

    let mut stream = tokio::net::TcpStream::connect(relay.addr)
        .await
        .expect("connect to relay");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("send register request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read response");
    let response_str = String::from_utf8_lossy(&response);

    // Parse the JSON body after the blank line.
    let json_start = response_str.find("\r\n\r\n").expect("HTTP header end") + 4;
    let json_body = &response_str[json_start..];
    let json: serde_json::Value = serde_json::from_str(json_body).expect("parse register json");

    let user_id: Uuid = json["user_id"]
        .as_str()
        .expect("user_id field")
        .parse()
        .expect("parse uuid");
    let access_token = json["access_token"]
        .as_str()
        .expect("access_token field")
        .to_string();

    UserCreds {
        user_id,
        email: email.to_string(),
        access_token,
    }
}

// ---------------------------------------------------------------------------
// WebSocket watcher connection
// ---------------------------------------------------------------------------

type WsStream = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A connected watcher WebSocket with typed send/recv helpers.
pub struct WatcherConn {
    ws: WsStream,
}

impl WatcherConn {
    /// Send a `ClientToArc` frame as a binary WebSocket message.
    pub async fn send(&mut self, msg: &ClientToArc) {
        let bytes = encode(msg).expect("encode ClientToArc");
        self.ws
            .send(Message::Binary(bytes))
            .await
            .expect("ws send");
    }

    /// Receive the next binary frame and decode it as `ArcToClient`.
    /// Skips non-binary frames (ping/pong/text).
    pub async fn recv(&mut self) -> ArcToClient {
        loop {
            match self.ws.next().await.expect("ws stream ended") {
                Ok(Message::Binary(bytes)) => {
                    return decode(&bytes).expect("decode ArcToClient");
                }
                Ok(Message::Close(_)) => panic!("WebSocket closed unexpectedly"),
                Ok(_) => continue, // skip ping/pong/text
                Err(e) => panic!("ws recv error: {e}"),
            }
        }
    }
}

/// Open a WebSocket connection authenticated as `user` and return the wrapper.
pub async fn connect_watcher(relay: &Relay, user: &UserCreds, _device_label: &str) -> WatcherConn {
    let url = relay.ws_url(&user.access_token);
    let (ws, _resp) = connect_async(&url).await.expect("ws connect");
    WatcherConn { ws }
}

// ---------------------------------------------------------------------------
// Host session helpers
// ---------------------------------------------------------------------------

/// Open a host WebSocket, send `HostHello` + `SessionRegister`, and return
/// the assigned session_id string.
///
/// The host connection is kept alive in a background task for the duration of
/// the test process.
pub async fn host_register_session(relay: &Relay, user: &UserCreds, label: &str) -> String {
    use beacon_model::SessionId;

    let url = relay.ws_url(&user.access_token);
    let (mut ws, _) = connect_async(&url).await.expect("host ws connect");

    // Send HostHello
    ws.send(Message::Binary(
        encode(&ClientToArc::HostHello {
            host_id: user.user_id.to_string(),
            auth_token: user.access_token.clone(),
            protocol_version: 1,
        })
        .expect("encode HostHello"),
    ))
    .await
    .expect("send HostHello");

    // Wait for HelloAck
    loop {
        match ws.next().await.expect("stream") {
            Ok(Message::Binary(b)) => {
                let msg: ArcToClient = decode(&b).expect("decode");
                if matches!(msg, ArcToClient::HelloAck { .. }) {
                    break;
                }
            }
            Ok(_) => continue,
            Err(e) => panic!("host ws error waiting for HelloAck: {e}"),
        }
    }

    // Register a session
    let session_id = SessionId::new();
    ws.send(Message::Binary(
        encode(&ClientToArc::SessionRegister {
            session_id,
            name: label.to_string(),
        })
        .expect("encode SessionRegister"),
    ))
    .await
    .expect("send SessionRegister");

    // Keep the host WS alive in the background.
    tokio::spawn(async move {
        while let Some(Ok(_)) = ws.next().await {}
    });

    session_id.to_string()
}

/// Placeholder for Task 2.9. For Task 2.8 tests we do not push output;
/// scrollback is empty and the test verifies the subscribe + PresenceChanged path.
///
/// Sanctioned deviation: real host push output requires Task 2.9 (ClientToArc::PushOutput).
#[allow(unused_variables, dead_code)]
pub async fn host_push_output(
    _relay: &Relay,
    _user: &UserCreds,
    _session_id: &str,
    _bytes: &[u8],
) {
    // No-op for Task 2.8. Task 2.9 will implement ClientToArc::PushOutput.
}
