/// Integration test: JSON-RPC handler closes with code 4001 when an access
/// token expires mid-connection.
///
/// Gated on the `fast-revalidate` Cargo feature which drops
/// `TOKEN_REVALIDATE_INTERVAL` from 30 s to 1 s so the test finishes quickly.
/// Run with: `cargo test -p arc-relay --features fast-revalidate`
///
/// # Test approach
///
/// We spin up an in-process axum server on a random port (no Docker, no DB).
/// We mint a 2-second access token, connect via the `arc.json-rpc` subprotocol,
/// sleep 4 seconds, then drain frames until we see Close(4001).
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{connect_async, tungstenite::Message};

const JWT_SECRET: &str = "test-secret-that-is-long-enough-32x";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn spawn_relay_no_db() -> std::net::SocketAddr {
    use arc_relay::ai_rpc::ArcAiSessionStore;
    use arc_relay::api::{build_router, AppState};
    use arc_relay::audit::AuditLog;
    use arc_relay::config::ArcConfig;
    use arc_relay::relay::router::RelayRouter;
    use arc_relay::rpc::ArcSessionStore;

    let pool =
        sqlx::PgPool::connect_lazy("postgres://arc:arc@localhost:5432/arc").expect("lazy pool");

    let config = ArcConfig { jwt_secret: JWT_SECRET.to_string(), ..Default::default() };

    let state = AppState {
        pool: pool.clone(),
        config: Arc::new(config),
        relay: Arc::new(RelayRouter::new()),
        audit: AuditLog::new(),
        mirror_registry: arc_relay::mirror::MirrorRegistry::default(),
        invitations: arc_relay::mirror::invitation::InvitationStore::new(pool),
        r2: None,
        arc_session_store: ArcSessionStore::new(64),
        arc_ai_session_store: ArcAiSessionStore::new(64),
        arc_ai_backend: None,
    };

    let router = build_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("axum serve");
    });

    addr
}

fn mint_short_token(ttl_secs: u64) -> String {
    use arc_relay::auth::jwt::generate_token_pair;
    let user_id = uuid::Uuid::new_v4();
    generate_token_pair(user_id, JWT_SECRET, ttl_secs, 600)
        .expect("generate_token_pair")
        .access_token
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The JSON-RPC handler must close the WebSocket with code 4001 once the
/// access token expires, even if the client never sends a message.
///
/// Requires `--features fast-revalidate` (1 s interval instead of 30 s).
#[cfg(feature = "fast-revalidate")]
#[tokio::test]
async fn jsonrpc_ws_closes_with_4001_when_access_token_expires() {
    let access_token = mint_short_token(2);

    let addr = spawn_relay_no_db().await;
    let url = format!("ws://127.0.0.1:{}/ws?token={}", addr.port(), access_token);

    let mut req = url.into_client_request().unwrap();
    req.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("arc.json-rpc"),
    );

    let (mut ws, _resp) = connect_async(req).await.expect("WS connect failed");

    // Wait 4 seconds — well past the 2-second token TTL.
    // The relay's 1-second periodic tick will detect expiry and send Close(4001).
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;

    // Nudge the relay by sending a ping so the select! wakes up if it hasn't.
    let _ = ws.send(Message::Ping(vec![])).await;

    let close_code = loop {
        match ws.next().await {
            Some(Ok(Message::Close(Some(cf)))) => {
                break u16::from(cf.code);
            }
            Some(Ok(Message::Close(None))) => {
                panic!("received Close frame with no close frame data (expected code 4001)");
            }
            Some(Ok(Message::Ping(_)))
            | Some(Ok(Message::Pong(_)))
            | Some(Ok(Message::Text(_))) => continue,
            Some(Ok(Message::Binary(_))) => continue,
            Some(Ok(Message::Frame(_))) => continue,
            Some(Err(e)) => panic!("WS error while waiting for close frame: {e}"),
            None => panic!("WebSocket stream ended without a Close frame"),
        }
    };

    assert_eq!(
        close_code, 4001,
        "expected close code 4001 (token expired), got {close_code}"
    );
}

/// Verify that a non-expired token does NOT get closed within 2 seconds
/// (sanity check — ensures the revalidation loop doesn't fire spuriously).
#[cfg(feature = "fast-revalidate")]
#[tokio::test]
async fn jsonrpc_ws_stays_open_with_valid_token() {
    // Token valid for 300 seconds — well beyond the test window.
    let access_token = mint_short_token(300);

    let addr = spawn_relay_no_db().await;
    let url = format!("ws://127.0.0.1:{}/ws?token={}", addr.port(), access_token);

    let mut req = url.into_client_request().unwrap();
    req.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("arc.json-rpc"),
    );

    let (mut ws, _resp) = connect_async(req).await.expect("WS connect failed");

    // Wait 2 seconds — two revalidation ticks with fast-revalidate.
    // A valid token should NOT trigger a close.
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(cf))) => return Some(cf),
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => return None,
            }
        }
    })
    .await;

    // Timeout means no close frame arrived — correct.
    assert!(
        result.is_err(),
        "connection should stay open with a valid token; got unexpected Close: {result:?}"
    );

    let _ = ws.close(None).await;
}
