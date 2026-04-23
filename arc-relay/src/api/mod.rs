use std::sync::Arc;

use axum::{
    middleware,
    routing::{get, post},
    Router,
};

use crate::audit::AuditLog;
use crate::auth::middleware::inject_jwt_secret;
use crate::config::ArcConfig;
use crate::mirror::invitation::InvitationStore;
use crate::mirror::MirrorRegistry;
use crate::relay::router::RelayRouter;

pub mod auth_routes;
pub mod marketplace_author_routes;
pub mod marketplace_install_routes;
pub mod marketplace_org_routes;
pub mod marketplace_review_routes;
pub mod marketplace_routes;
pub mod oauth_routes;
pub mod plugin_routes;
pub mod session_routes;
pub mod shadow_routes;
pub mod team_routes;
pub mod user_routes;

/// Shared application state injected into every route handler.
#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub config: Arc<ArcConfig>,
    pub relay: Arc<RelayRouter>,
    pub audit: AuditLog,
    pub mirror_registry: MirrorRegistry,
    pub invitations: InvitationStore,
    pub r2: Option<crate::storage::R2Client>,
}

/// Assemble the top-level Axum router.
pub fn build_router(state: AppState) -> Router {
    let jwt_secret = state.config.jwt_secret.clone();

    // ── Public auth routes (no JWT required) ────────────────────────────────
    let auth = Router::new()
        .route("/register", post(auth_routes::register))
        .route("/login", post(auth_routes::login))
        .route("/refresh", post(auth_routes::refresh))
        .route("/oauth/:provider", get(oauth_routes::oauth_redirect))
        .route("/oauth/:provider/callback", get(oauth_routes::oauth_callback));

    // ── Protected API routes (Bearer JWT required) ───────────────────────────
    //
    // inject_jwt_secret is a `from_fn_with_state` middleware that takes a
    // plain `String` state (the JWT secret) and inserts it into request
    // extensions as `JwtSecret`. The `AuthUser` extractor then reads it.
    let api = Router::new()
        .route("/users/me", get(user_routes::get_me))
        .route("/sessions", get(session_routes::list_sessions))
        .route("/sessions/:id", get(session_routes::get_session))
        .route(
            "/sessions/:id/shadow-diff",
            get(shadow_routes::get_shadow_diff),
        )
        .route(
            "/teams",
            get(team_routes::list_teams).post(team_routes::create_team),
        )
        .route("/teams/:id/members", get(team_routes::list_members))
        .route(
            "/plugins/sync",
            post(plugin_routes::sync_plugins),
        )
        .route(
            "/plugins",
            get(plugin_routes::list_plugins).post(plugin_routes::register_plugin),
        )
        .route(
            "/plugins/:id",
            axum::routing::delete(plugin_routes::delete_plugin)
                .patch(plugin_routes::patch_plugin),
        )
        .route(
            "/plugins/:id/config",
            get(plugin_routes::get_plugin_config).put(plugin_routes::write_plugin_config),
        )
        .layer(middleware::from_fn_with_state(
            jwt_secret,
            inject_jwt_secret,
        ));

    // ── Authenticated marketplace routes (JWT required) ──────────────────────
    let jwt_secret_clone = state.config.jwt_secret.clone();
    let marketplace_authed = Router::new()
        .route("/plugins", post(marketplace_author_routes::create_plugin))
        .route("/my-plugins", get(marketplace_author_routes::my_plugins))
        .route(
            "/plugins/:id",
            axum::routing::patch(marketplace_author_routes::update_plugin)
                .delete(marketplace_author_routes::delete_plugin),
        )
        .route(
            "/plugins/:id/versions",
            post(marketplace_author_routes::upload_version),
        )
        .route("/review", get(marketplace_review_routes::list_pending))
        .route("/review/:id/approve", post(marketplace_review_routes::approve))
        .route("/review/:id/reject", post(marketplace_review_routes::reject))
        .route(
            "/org/:org_id/allowlist",
            get(marketplace_org_routes::get_allowlist)
                .post(marketplace_org_routes::add_to_allowlist),
        )
        .route(
            "/org/:org_id/allowlist/:plugin_id",
            axum::routing::delete(marketplace_org_routes::remove_from_allowlist),
        )
        .route("/install", post(marketplace_install_routes::install))
        .layer(middleware::from_fn_with_state(jwt_secret_clone, inject_jwt_secret));

    // ── Public marketplace routes (no JWT required) ──────────────────────────
    let marketplace = Router::new()
        .merge(marketplace_authed)
        .route("/plugins", get(marketplace_routes::list_plugins))
        .route("/plugins/:slug", get(marketplace_routes::get_plugin))
        .route(
            "/plugins/:slug/versions",
            get(marketplace_routes::get_plugin_versions),
        )
        .route("/categories", get(marketplace_routes::list_categories));

    // ── Setup routes (public, no JWT required) ──────────────────────────────
    let setup = Router::new()
        .route("/status", get(crate::setup::get_setup_status))
        .route("/", post(crate::setup::complete_setup));

    // ── Audit route (public for now; protect behind JWT in production) ───────
    let audit_state = state.audit.clone();
    let audit = Router::new()
        .route("/", get(crate::audit::get_audit_log))
        .with_state(audit_state);

    // ── WebSocket route ──────────────────────────────────────────────────────
    let ws = Router::new().route("/ws", get(crate::ws::handler::ws_handler));

    Router::new()
        .nest("/api/auth", auth)
        .nest("/api/marketplace", marketplace)
        .nest("/api", api)
        .nest("/api/setup", setup)
        .nest("/api/audit", audit)
        .merge(ws)
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt; // for oneshot

    /// Build a minimal AppState with a fake (non-connecting) pool.
    ///
    /// We only exercise routing here — no DB calls are made.
    pub(crate) fn test_state() -> AppState {
        // We can't construct a real PgPool without a DB, but we can at least
        // build the router and test that routes *exist* by checking their
        // response codes (404 = route missing, 405/4xx = route found).
        //
        // For route-shape tests we use a disconnected pool constructed via
        // sqlx's lazy-connect approach: the pool is created but no connection
        // is actually opened until a query runs.
        let pool =
            sqlx::PgPool::connect_lazy("postgres://arc:arc@localhost:5432/arc").expect("lazy pool");
        AppState {
            pool: pool.clone(),
            config: Arc::new(ArcConfig::default()),
            relay: Arc::new(RelayRouter::new()),
            audit: crate::audit::AuditLog::new(),
            mirror_registry: MirrorRegistry::default(),
            invitations: InvitationStore::new(pool),
            r2: None,
        }
    }

    // ── route-shape tests ───────────────────────────────────────────────────

    /// Helper: send a request and return the status code.
    async fn status(app: Router, method: &str, uri: &str) -> StatusCode {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        app.oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn auth_register_route_exists() {
        // A POST with no body should reach the handler and fail with 422
        // (Unprocessable Entity — JSON body required) rather than 404.
        let app = build_router(test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/register")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let s = app.oneshot(req).await.unwrap().status();
        // 422 = handler reached but body rejected; 404 = route not found.
        assert_ne!(s, StatusCode::NOT_FOUND, "/api/auth/register must exist");
    }

    #[tokio::test]
    async fn auth_login_route_exists() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let s = app.oneshot(req).await.unwrap().status();
        assert_ne!(s, StatusCode::NOT_FOUND, "/api/auth/login must exist");
    }

    #[tokio::test]
    async fn auth_refresh_route_exists() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/refresh")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let s = app.oneshot(req).await.unwrap().status();
        assert_ne!(s, StatusCode::NOT_FOUND, "/api/auth/refresh must exist");
    }

    #[tokio::test]
    async fn users_me_route_requires_auth() {
        // Without an Authorization header the JWT middleware should reject with 401.
        let s = status(build_router(test_state()), "GET", "/api/users/me").await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "/api/users/me needs auth");
    }

    #[tokio::test]
    async fn sessions_route_requires_auth() {
        let s = status(build_router(test_state()), "GET", "/api/sessions").await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "/api/sessions needs auth");
    }

    #[tokio::test]
    async fn session_by_id_route_requires_auth() {
        let s = status(build_router(test_state()), "GET", "/api/sessions/some-id").await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "/api/sessions/:id needs auth");
    }

    #[tokio::test]
    async fn teams_route_requires_auth() {
        let s = status(build_router(test_state()), "GET", "/api/teams").await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "/api/teams needs auth");
    }

    #[tokio::test]
    async fn teams_post_route_requires_auth() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/api/teams")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"Test"}"#))
            .unwrap();
        let s = app.oneshot(req).await.unwrap().status();
        assert_eq!(s, StatusCode::UNAUTHORIZED, "POST /api/teams needs auth");
    }

    #[tokio::test]
    async fn team_members_route_requires_auth() {
        let s = status(
            build_router(test_state()),
            "GET",
            "/api/teams/550e8400-e29b-41d4-a716-446655440000/members",
        )
        .await;
        assert_eq!(
            s,
            StatusCode::UNAUTHORIZED,
            "/api/teams/:id/members needs auth"
        );
    }

    #[tokio::test]
    async fn marketplace_plugins_route_exists() {
        let s = status(build_router(test_state()), "GET", "/api/marketplace/plugins").await;
        // 500 (lazy pool, no DB) means route exists; 404 means it doesn't
        assert_ne!(s, StatusCode::NOT_FOUND, "/api/marketplace/plugins must exist");
    }

    #[tokio::test]
    async fn marketplace_categories_route_exists() {
        let s = status(build_router(test_state()), "GET", "/api/marketplace/categories").await;
        assert_ne!(s, StatusCode::NOT_FOUND, "/api/marketplace/categories must exist");
    }

    #[tokio::test]
    async fn oauth_redirect_route_exists() {
        let s = status(build_router(test_state()), "GET", "/api/auth/oauth/google").await;
        // Should be 501 (not configured) not 404 (route missing)
        assert_ne!(s, StatusCode::NOT_FOUND, "/api/auth/oauth/:provider must exist");
    }

    #[tokio::test]
    async fn unknown_route_returns_404() {
        let s = status(build_router(test_state()), "GET", "/api/unknown-path").await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    // ── protected route with valid token ────────────────────────────────────

    #[tokio::test]
    async fn users_me_with_valid_token_reaches_handler() {
        use crate::auth::jwt::generate_token_pair;

        let state = test_state();
        let app = build_router(state.clone());

        // Mint a valid access token for a random user ID.
        let user_id = uuid::Uuid::new_v4();
        let pair = generate_token_pair(user_id, &state.config.jwt_secret, 900, 86_400).unwrap();

        let req = Request::builder()
            .method("GET")
            .uri("/api/users/me")
            .header("authorization", format!("Bearer {}", pair.access_token))
            .body(Body::empty())
            .unwrap();

        let s = app.oneshot(req).await.unwrap().status();
        // The valid JWT bypasses auth middleware — the handler is reached.
        // With a real (lazy) pool the user won't exist so we get 404 or 500,
        // but crucially not 401 (auth rejected) or 403 (forbidden).
        assert_ne!(s, StatusCode::UNAUTHORIZED);
        assert_ne!(s, StatusCode::FORBIDDEN);
    }
}
