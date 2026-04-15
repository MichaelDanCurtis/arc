/// Integration test: relay sends close code 4001 when an access token expires
/// mid-connection.
///
/// # Test approach
///
/// We spin up an in-process axum server bound to a random OS-assigned port
/// (no external process, no Docker, no real Postgres). The WS upgrade handler
/// only needs the JWT secret — it never touches the DB until after the first
/// `HostHello` / `AttachSession` message, which we deliberately never send.
/// So a lazy `PgPool` (never actually connected) is sufficient.
///
/// We mint a 2-second access token, connect via `tokio-tungstenite`, sleep
/// past the expiry window, then send a ping.  The relay's periodic 1-second
/// expiry tick should have detected the expired token and sent a Close(4001)
/// frame before (or in response to) our ping.
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};

const JWT_SECRET: &str = "test-secret-that-is-long-enough-32x";

// ---------------------------------------------------------------------------
// Minimal in-process relay helpers
// ---------------------------------------------------------------------------

/// Spawn an axum server on a random port and return the bound address.
async fn spawn_relay() -> std::net::SocketAddr {
    use arc_relay::api::{build_router, AppState};
    use arc_relay::audit::AuditLog;
    use arc_relay::config::ArcConfig;
    use arc_relay::relay::router::RelayRouter;

    let pool =
        sqlx::PgPool::connect_lazy("postgres://arc:arc@localhost:5432/arc").expect("lazy pool");

    let mut config = ArcConfig::default();
    config.jwt_secret = JWT_SECRET.to_string();

    let state = AppState {
        pool,
        config: Arc::new(config),
        relay: Arc::new(RelayRouter::new()),
        audit: AuditLog::new(),
    };

    let router = build_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("axum serve failed");
    });

    addr
}

/// Mint an access token with a short TTL (in seconds).
fn mint_token(ttl_secs: u64) -> String {
    use arc_relay::auth::jwt::generate_token_pair;
    let user_id = uuid::Uuid::new_v4();
    generate_token_pair(user_id, JWT_SECRET, ttl_secs, 600)
        .expect("generate_token_pair")
        .access_token
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

/// The relay must close the WebSocket with code 4001 once the access token
/// expires, even if the client never sends a message.
#[tokio::test]
async fn ws_closes_with_4001_when_access_token_expires() {
    // Issue a token that expires in 2 seconds.
    let access_token = mint_token(2);

    let addr = spawn_relay().await;
    let url = format!("ws://127.0.0.1:{}/ws?token={}", addr.port(), access_token);

    let (mut ws, _resp) = connect_async(&url).await.expect("WS connect failed");

    // Wait 4 seconds — well past the 2-second token TTL.
    // The relay's 1-second periodic tick will detect expiry and send Close(4001).
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;

    // Optionally nudge the relay by sending a ping so it processes the socket.
    // (In practice the tick fires independently, but sending a frame ensures
    // the relay's select! wakes up if it hasn't already.)
    let _ = ws.send(Message::Ping(vec![])).await;

    // Drain frames until we see a Close or the stream ends.
    let close_code = loop {
        match ws.next().await {
            Some(Ok(Message::Close(Some(cf)))) => {
                break u16::from(cf.code);
            }
            Some(Ok(Message::Close(None))) => {
                panic!("received Close frame with no close frame data (expected code 4001)");
            }
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) | Some(Ok(Message::Text(_))) => {
                // Ignore non-data frames and keep draining.
                continue;
            }
            Some(Ok(Message::Binary(_))) => {
                // Relay may send an AttachDenied or similar before closing; keep draining.
                continue;
            }
            Some(Ok(Message::Frame(_))) => {
                continue;
            }
            Some(Err(e)) => {
                panic!("WS error while waiting for close frame: {e}");
            }
            None => {
                panic!("WebSocket stream ended without a Close frame");
            }
        }
    };

    assert_eq!(
        close_code, 4001,
        "expected close code 4001 (token expired), got {close_code}"
    );
}
