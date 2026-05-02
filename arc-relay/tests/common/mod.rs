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

/// Spin up a relay, skipping the test if `DATABASE_URL` is not set.
///
/// Convenience wrapper around [`try_spawn_relay`] for tests that use the
/// `#[ignore]` attribute but call a `spawn_relay` that returns `Relay`
/// directly.  Returns `None` (caller should early-return) when the DB is
/// unavailable.
#[allow(dead_code)]
pub async fn spawn_relay() -> Option<Relay> {
    try_spawn_relay().await
}

/// Drain frames from `conn` until the predicate `pred` returns `true` for a
/// received frame.  Panics if the stream closes before the predicate matches,
/// or if the predicate has not matched within 10 seconds (guards against
/// test-deadlock when the awaited frame never arrives — e.g. a relay bug that
/// silently drops PresenceChanged).
#[allow(dead_code)]
pub async fn drain_until<F>(conn: &mut WatcherConn, pred: F)
where
    F: Fn(&ArcToClient) -> bool,
{
    let fut = async {
        loop {
            let frame = conn.recv().await;
            if pred(&frame) {
                return;
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), fut)
        .await
        .expect("drain_until: timed out after 10s waiting for predicate to match");
}

/// Spin up a full axum relay on a random port backed by the `DATABASE_URL`
/// environment variable.  Returns `None` if `DATABASE_URL` is not set.
pub async fn try_spawn_relay() -> Option<Relay> {
    let db_url = std::env::var("DATABASE_URL").ok()?;
    let pool = sqlx::PgPool::connect(&db_url).await.ok()?;

    // Run migrations so the schema is up to date.
    sqlx::migrate!("./migrations").run(&pool).await.ok()?;

    let jwt_secret = "test-secret-that-is-long-enough-32x".to_string();
    let config = ArcConfig { jwt_secret: jwt_secret.clone(), ..Default::default() };

    let state = AppState {
        pool: pool.clone(),
        config: Arc::new(config),
        relay: Arc::new(RelayRouter::new()),
        audit: AuditLog::new(),
        mirror_registry: MirrorRegistry::default(),
        invitations: InvitationStore::new(pool),
        r2: None,
        arc_session_store: arc_relay::rpc::ArcSessionStore::new(64),
        arc_ai_session_store: arc_relay::ai_rpc::ArcAiSessionStore::new(64),
        arc_ai_backend: None,
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

    // NOTE: assumes Content-Length response (Axum JSON does this). If a future
    // endpoint returns chunked, rewrite with reqwest as a dev-dep.
    //
    // Fail fast if the server ever sends chunked transfer encoding — the naive
    // "\r\n\r\n" split will produce garbled hex-length prefixes as "JSON" and
    // produce confusing downstream panics rather than a clear error here.
    debug_assert!(
        !response_str.to_ascii_lowercase().contains("transfer-encoding: chunked"),
        "raw HTTP parser cannot handle chunked responses; rewrite with reqwest if this fires"
    );

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

// ---------------------------------------------------------------------------
// JSON-RPC test helpers (Task 1+)
// ---------------------------------------------------------------------------

/// Spin up a minimal in-process relay on a random port.
///
/// Does NOT require `DATABASE_URL` — uses a lazy (never-connecting) pool.
/// The JWT secret is pinned to `"test-secret"` so that [`mint_access_token`]
/// works without a real config.
pub async fn spawn_test_server() -> (std::net::SocketAddr, AppState) {
    // Ensure JsonRpcSpine is enabled for all JSON-RPC integration tests.
    // (Also in DOGFOOD_FLAGS for debug builds, but env var is authoritative.)
    // TODO(beacon-features test setter): replace with a proper test-mode setter on
    // `beacon_features` once added; `set_var` is unsafe under concurrent reads and
    // leaks process-globally to other tests in this binary.
    std::env::set_var("BEACON_FF_JSON_RPC_SPINE", "1");

    // Lazy pool — DB is not exercised in these tests.
    let pool = sqlx::PgPool::connect_lazy("postgres://arc:arc@localhost:5432/arc")
        .expect("lazy pool");
    let mut config = ArcConfig::default();
    config.jwt_secret = "test-secret".into();
    let arc_session_store = arc_relay::rpc::ArcSessionStore::new(64);
    let mirror_registry = MirrorRegistry::default();
    mirror_registry.add_emitter(std::sync::Arc::new(
        arc_relay::rpc::ArcMirrorEmitter::new(arc_session_store.clone()),
    ));
    let state = AppState {
        pool: pool.clone(),
        config: std::sync::Arc::new(config),
        relay: std::sync::Arc::new(RelayRouter::new()),
        audit: AuditLog::new(),
        mirror_registry,
        invitations: InvitationStore::new(pool),
        r2: None,
        arc_session_store,
        arc_ai_session_store: arc_relay::ai_rpc::ArcAiSessionStore::new(64),
        arc_ai_backend: None,
    };
    let app = build_router(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, state)
}

/// Spin up a minimal in-process relay on a random port with a custom AI backend.
///
/// Identical to [`spawn_test_server`] but sets `arc_ai_backend` to `Some(backend)` so
/// that the JSON-RPC WS handler registers the `arc.ai.*` handlers.  Call this from AI
/// integration tests; use [`spawn_test_server`] for tests that don't need AI.
pub async fn spawn_test_server_with_ai(
    backend: std::sync::Arc<dyn arc_relay::ai_rpc::ArcAiBackend>,
) -> (std::net::SocketAddr, AppState) {
    std::env::set_var("BEACON_FF_JSON_RPC_SPINE", "1");

    let pool = sqlx::PgPool::connect_lazy("postgres://arc:arc@localhost:5432/arc")
        .expect("lazy pool");
    let mut config = ArcConfig::default();
    config.jwt_secret = "test-secret".into();
    let arc_session_store = arc_relay::rpc::ArcSessionStore::new(64);
    let mirror_registry = MirrorRegistry::default();
    mirror_registry.add_emitter(std::sync::Arc::new(
        arc_relay::rpc::ArcMirrorEmitter::new(arc_session_store.clone()),
    ));
    let state = AppState {
        pool: pool.clone(),
        config: std::sync::Arc::new(config),
        relay: std::sync::Arc::new(RelayRouter::new()),
        audit: AuditLog::new(),
        mirror_registry,
        invitations: InvitationStore::new(pool),
        r2: None,
        arc_session_store,
        arc_ai_session_store: arc_relay::ai_rpc::ArcAiSessionStore::new(64),
        arc_ai_backend: Some(backend),
    };
    let app = build_router(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, state)
}

/// Mint a short-lived access token using the test JWT secret.
pub fn mint_access_token() -> String {
    use arc_relay::auth::jwt::generate_token_pair;
    let pair =
        generate_token_pair(Uuid::new_v4(), "test-secret", 900, 86_400).unwrap();
    pair.access_token
}

// ---------------------------------------------------------------------------
// JSON-RPC WebSocket client (Task 3+)
// ---------------------------------------------------------------------------

use tokio_tungstenite::{
    tungstenite::{client::IntoClientRequest, http::HeaderValue},
    MaybeTlsStream, WebSocketStream as TungsteniteWsStream,
};

/// A connected JSON-RPC WebSocket client for integration tests.
pub struct JsonRpcWs {
    pub inner: TungsteniteWsStream<MaybeTlsStream<tokio::net::TcpStream>>,
}

impl JsonRpcWs {
    /// Send a JSON-RPC request and wait for the first text response frame.
    /// Ping/Pong frames are skipped transparently.
    pub async fn request(&mut self, req: serde_json::Value) -> serde_json::Value {
        use futures_util::{SinkExt, StreamExt};
        self.inner
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&req).unwrap(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                match self.inner.next().await.unwrap().unwrap() {
                    tokio_tungstenite::tungstenite::Message::Text(s) => {
                        return serde_json::from_str(&s).unwrap()
                    }
                    tokio_tungstenite::tungstenite::Message::Ping(_)
                    | tokio_tungstenite::tungstenite::Message::Pong(_) => continue,
                    m => panic!("unexpected frame: {m:?}"),
                }
            }
        })
        .await
        .expect("JsonRpcWs::request: timed out after 10s")
    }

    /// Wait for the next JSON-RPC notification (text frame without an `id`
    /// that has a `method` field). Ping/Pong and response frames are skipped.
    pub async fn next_notification(&mut self) -> serde_json::Value {
        use futures_util::StreamExt;
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                match self.inner.next().await.unwrap().unwrap() {
                    tokio_tungstenite::tungstenite::Message::Text(s) => {
                        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
                        if v.get("method").is_some() && v.get("id").is_none() {
                            return v;
                        }
                        // response frame — skip
                    }
                    tokio_tungstenite::tungstenite::Message::Ping(_)
                    | tokio_tungstenite::tungstenite::Message::Pong(_) => continue,
                    m => panic!("unexpected frame: {m:?}"),
                }
            }
        })
        .await
        .expect("JsonRpcWs::next_notification: timed out after 10s")
    }
}

/// Connect to the relay at `addr` with `Sec-WebSocket-Protocol: arc.json-rpc`.
pub async fn connect_jsonrpc(addr: &std::net::SocketAddr) -> JsonRpcWs {
    let token = mint_access_token();
    let mut req = format!("ws://{addr}/ws?token={token}")
        .into_client_request()
        .unwrap();
    req.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("arc.json-rpc"),
    );
    let (inner, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    JsonRpcWs { inner }
}

/// Send a `PushOutput` frame from the host for `session_id`.
///
/// Opens a fresh host WebSocket, authenticates with `HostHello`, sends the
/// `PushOutput` frame, and closes the connection. The session must already be
/// registered (via `host_register_session`) before calling this.
#[allow(dead_code)]
pub async fn host_push_output(
    relay: &Relay,
    user: &UserCreds,
    session_id: &str,
    bytes: &[u8],
) {
    use beacon_model::SessionId;

    let session_id_parsed: SessionId = session_id.parse().expect("parse session_id");

    let url = relay.ws_url(&user.access_token);
    let (mut ws, _) = connect_async(&url).await.expect("host_push_output: ws connect");

    // Authenticate with HostHello.
    ws.send(Message::Binary(
        encode(&ClientToArc::HostHello {
            host_id: user.user_id.to_string(),
            auth_token: user.access_token.clone(),
            protocol_version: 1,
        })
        .expect("encode HostHello"),
    ))
    .await
    .expect("host_push_output: send HostHello");

    // Wait for HelloAck.
    loop {
        match ws.next().await.expect("host_push_output: stream ended") {
            Ok(Message::Binary(b)) => {
                let msg: ArcToClient = decode(&b).expect("decode");
                if matches!(msg, ArcToClient::HelloAck { .. }) {
                    break;
                }
            }
            Ok(_) => continue,
            Err(e) => panic!("host_push_output: ws error waiting for HelloAck: {e}"),
        }
    }

    // Send PushOutput. `seq` is always 0 here — the relay discards the
    // client-sent seq and assigns its own canonical sequence from the
    // scrollback ring, so any value is equivalent for test purposes.
    ws.send(Message::Binary(
        encode(&ClientToArc::PushOutput {
            session_id: session_id_parsed,
            bytes: bytes.to_vec(),
            seq: 0,
        })
        .expect("encode PushOutput"),
    ))
    .await
    .expect("host_push_output: send PushOutput");

    // Give the relay time to process the frame, then close cleanly.
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    let _ = ws.close(None).await;
}
