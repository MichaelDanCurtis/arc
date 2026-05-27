use std::sync::Arc;

use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post},
    Router,
};

/// Maximum allowed request body size for marketplace plugin bundle uploads
/// (POST `/api/marketplace/plugins/:slug/versions`).
///
/// 50 MiB is intentionally generous for a single-binary plugin while still
/// preventing a pro-tier author from OOMing the relay by streaming an
/// unbounded multipart field into memory. Axum returns `413 Payload Too
/// Large` automatically when a request body exceeds this limit.
///
/// Bumping this cap requires also revisiting the multipart handler in
/// `marketplace_author_routes::upload_version` (the entire file field is
/// still buffered into `Vec<u8>` for SHA-256 + R2 upload — at much larger
/// sizes that buffer becomes a problem on its own).
const MAX_PLUGIN_BUNDLE_BYTES: usize = 50 * 1024 * 1024;

use crate::ai_rpc::{ArcAiBackend, ArcAiSessionStore};
use crate::audit::AuditLog;
use crate::auth::middleware::inject_jwt_secret;
use crate::config::ArcConfig;
use crate::mirror::invitation::InvitationStore;
use crate::mirror::MirrorRegistry;
use crate::relay::router::RelayRouter;
use crate::rpc::ArcSessionStore;

pub mod ai_routes;
pub mod auth_routes;
pub mod graphql_routes;
pub mod health_routes;
pub mod time_routes;
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
    pub arc_session_store: ArcSessionStore,
    pub arc_ai_session_store: ArcAiSessionStore,
    pub arc_ai_backend: Option<Arc<dyn ArcAiBackend>>,
}

/// Expose `PgPool` via `FromRef` so extractors that need DB access
/// (e.g. `RequirePro` in `auth::middleware`) can pull a connection
/// pool without depending on the concrete `AppState` type.
impl axum::extract::FromRef<AppState> for sqlx::PgPool {
    fn from_ref(state: &AppState) -> Self {
        state.pool.clone()
    }
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

    // ── AI fan-out routes (Bearer JWT required) ─────────────────────────────
    let jwt_secret_ai = state.config.jwt_secret.clone();
    let ai = Router::new()
        .route("/chat", post(ai_routes::chat_handler))
        .route("/stream_chat", post(ai_routes::stream_chat_handler))
        .route("/complete", post(ai_routes::complete_handler))
        .route("/list_models", get(ai_routes::list_models_handler))
        .layer(middleware::from_fn_with_state(
            jwt_secret_ai,
            inject_jwt_secret,
        ));

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

    // ── Combined marketplace router ──────────────────────────────────────────
    // Public and authed routes are merged into one router to avoid param-name
    // conflicts (/:slug vs /:id at the same path position). The JWT middleware
    // runs on all marketplace routes; handlers that require auth use the
    // AuthUser extractor which returns 401 when the header is absent.
    let jwt_secret_clone = state.config.jwt_secret.clone();
    let marketplace = Router::new()
        // ── public read routes ──────────────────────────────────────────────
        .route(
            "/plugins",
            get(marketplace_routes::list_plugins)
                .post(marketplace_author_routes::create_plugin),
        )
        .route("/categories", get(marketplace_routes::list_categories))
        .route("/my-plugins", get(marketplace_author_routes::my_plugins))
        // ── plugin detail + CRUD (same :slug param used for both GET and PATCH/DELETE)
        .route(
            "/plugins/:slug",
            get(marketplace_routes::get_plugin)
                .patch(marketplace_author_routes::update_plugin)
                .delete(marketplace_author_routes::delete_plugin),
        )
        // ── versions (GET public list, POST upload new version)
        .route(
            "/plugins/:slug/versions",
            get(marketplace_routes::get_plugin_versions)
                .post(marketplace_author_routes::upload_version),
        )
        // ── admin review queue
        .route("/review", get(marketplace_review_routes::list_pending))
        .route("/review/:id/approve", post(marketplace_review_routes::approve))
        .route("/review/:id/reject", post(marketplace_review_routes::reject))
        // ── org allowlist
        .route(
            "/org/:org_id/allowlist",
            get(marketplace_org_routes::get_allowlist)
                .post(marketplace_org_routes::add_to_allowlist),
        )
        .route(
            "/org/:org_id/allowlist/:plugin_id",
            axum::routing::delete(marketplace_org_routes::remove_from_allowlist),
        )
        // ── install
        .route("/install", post(marketplace_install_routes::install))
        // ── signing key routes
        .route("/signing-key", axum::routing::put(marketplace_author_routes::set_signing_key))
        .route("/signing-key/server", get(marketplace_author_routes::get_server_public_key))
        // F2: Cap request body size for the entire marketplace surface so
        // a pro-tier author can't OOM the relay by uploading a giant
        // bundle. axum's default is 2 MiB which would block legitimate
        // plugin bundles; MAX_PLUGIN_BUNDLE_BYTES (50 MiB) is the new
        // explicit ceiling. axum returns 413 when this is exceeded.
        .layer(DefaultBodyLimit::max(MAX_PLUGIN_BUNDLE_BYTES))
        .layer(middleware::from_fn_with_state(jwt_secret_clone, inject_jwt_secret));

    // ── Setup routes (public, no JWT required) ──────────────────────────────
    let setup = Router::new()
        .route("/status", get(crate::setup::get_setup_status))
        .route("/", post(crate::setup::complete_setup));

    // ── Audit route — admin-only, JWT-gated ──────────────────────────────────
    //
    // The handler now takes `State<AppState>` + `AuthUser` + checks
    // `user::is_admin`. Returns 401 without a Bearer token, 403 for a
    // non-admin user. The JWT secret is injected via the same
    // `inject_jwt_secret` middleware used by the marketplace review router.
    let jwt_secret_audit = state.config.jwt_secret.clone();
    let audit = Router::new()
        .route("/", get(crate::audit::get_audit_log))
        .layer(middleware::from_fn_with_state(
            jwt_secret_audit,
            inject_jwt_secret,
        ));

    // ── WebSocket route ──────────────────────────────────────────────────────
    let ws = Router::new().route("/ws", get(crate::ws::handler::ws_handler));

    // ── Beam GraphQL-compat surface ─────────────────────────────────────────
    // Beam OSS clients hit `/graphql/v2?op=<OperationName>` for a handful of
    // legacy queries — most notably `FreeAvailableModels` on first launch.
    // The stub handler returns sensible empty responses so the client
    // doesn't spam DNS-failure warnings.
    let graphql = Router::new().route("/v2", post(graphql_routes::graphql_handler));

    // ── /api/health — tier-one uptime probe target ───────────────────────────
    // Lives under /api/* so Traefik's PathPrefix(`/api`) rule routes it to
    // arc-relay. A root-level /health would 404 from arc-web (the catch-all
    // frontend) because Traefik doesn't route bare /health to the relay.
    let health = Router::new().route("/", get(health_routes::health_handler));

    // ── /api/v1 — versioned Beam-required + future-stable endpoints ──────────
    // Per REQUIREMENTS.md §8a. Grows as endpoints land:
    //   - /current_time (shipped — D1)
    //   - /telemetry, /relevant_files, /voice/transcribe, /credential_meter (TBD)
    //   - /conversations/sync, /images/upload (per docs/specs/)
    let v1 = Router::new().route("/current_time", get(time_routes::current_time_handler));

    Router::new()
        .nest("/api/health", health)
        .nest("/api/v1", v1)
        .nest("/api/auth", auth)
        .nest("/api/marketplace", marketplace)
        .nest("/api", api)
        .nest("/v1/ai", ai)
        .nest("/api/setup", setup)
        .nest("/api/audit", audit)
        .nest("/graphql", graphql)
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
            arc_session_store: ArcSessionStore::new(64),
            arc_ai_session_store: ArcAiSessionStore::new(64),
            arc_ai_backend: None,
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

    // ── audit log access control ─────────────────────────────────────────────
    //
    // Prior to F1 the audit route was mounted as a public router. These
    // tests assert it now requires a Bearer JWT and rejects non-admins.

    #[tokio::test]
    async fn audit_route_requires_auth() {
        // Audit router is nested at /api/audit with a single route at "/" —
        // axum 0.7 nesting strips the prefix so the actual match is /api/audit
        // (no trailing slash), same shape as /api/health.
        //
        // Important assertion: an unauthenticated caller cannot get 200.
        // Prior to F1 this returned 200 with the full log payload.
        let s = status(build_router(test_state()), "GET", "/api/audit").await;
        assert_eq!(
            s,
            StatusCode::UNAUTHORIZED,
            "/api/audit must return 401 without a Bearer token (got {s})"
        );
    }

    #[tokio::test]
    async fn audit_route_with_non_admin_token_returns_forbidden() {
        use crate::auth::jwt::generate_token_pair;

        let state = test_state();
        let app = build_router(state.clone());

        // Mint a valid access token for a random user ID. The user won't
        // exist in DB (lazy pool / no real connection). With the F1 fix
        // the handler calls `user::is_admin` which will either return
        // Ok(false) (no membership row → 403) or Err (DB lookup failed
        // → 500). Either way the route MUST NOT return 200 to a
        // non-admin caller, and MUST NOT return 401 (token was valid).
        let user_id = uuid::Uuid::new_v4();
        let pair =
            generate_token_pair(user_id, &state.config.jwt_secret, 900, 86_400).unwrap();

        let req = Request::builder()
            .method("GET")
            .uri("/api/audit")
            .header("authorization", format!("Bearer {}", pair.access_token))
            .body(Body::empty())
            .unwrap();

        let s = app.oneshot(req).await.unwrap().status();
        assert_ne!(
            s,
            StatusCode::OK,
            "/api/audit must NOT return 200 to a non-admin caller (got {s})"
        );
        assert_ne!(
            s,
            StatusCode::UNAUTHORIZED,
            "/api/audit with a valid token must NOT return 401 (got {s})"
        );
    }

    // ── /api/v1/current_time (D1 — Beam clock-skew check target) ────────────

    #[tokio::test]
    async fn current_time_route_returns_unix_seconds_and_iso() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/current_time")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status_code = resp.status();
        assert_eq!(
            status_code,
            StatusCode::OK,
            "/api/v1/current_time must return 200 (got {status_code})"
        );

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&bytes).expect("body must be JSON");
        assert!(
            body.get("unix_seconds")
                .and_then(|v| v.as_u64())
                .is_some_and(|n| n > 1_700_000_000),
            "unix_seconds must be a recent u64, body was {body}"
        );
        assert!(
            body.get("iso8601")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.contains('T') && s.ends_with('Z')),
            "iso8601 must be an RFC3339-style string with T and Z, body was {body}"
        );
    }

    // ── tier gating ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn marketplace_plugin_publish_requires_pro_tier() {
        use crate::auth::jwt::generate_token_pair;

        let state = test_state();
        let app = build_router(state.clone());

        // Mint a valid access token for a random user ID. The user won't
        // exist in DB (lazy pool / no real connection) — RequirePro must
        // fail-closed and return 403 when it can't verify the user is
        // `pro`-tier. This is BOTH the route-level entitlement gate AND
        // the security-fallback stance.
        let user_id = uuid::Uuid::new_v4();
        let pair =
            generate_token_pair(user_id, &state.config.jwt_secret, 900, 86_400).unwrap();

        let req = Request::builder()
            .method("POST")
            .uri("/api/marketplace/plugins")
            .header("authorization", format!("Bearer {}", pair.access_token))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"slug":"x","name":"x","summary":"x","kind":"theme","category":"ai"}"#))
            .unwrap();

        let s = app.oneshot(req).await.unwrap().status();
        assert_eq!(
            s,
            StatusCode::FORBIDDEN,
            "POST /api/marketplace/plugins must require pro tier (got {s})"
        );
    }

    // ── /api/health (tier-one observability probe target) ────────────────────

    #[tokio::test]
    async fn health_route_returns_ok_json_with_required_fields() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("GET")
            .uri("/api/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status_code = resp.status();
        assert_eq!(
            status_code,
            StatusCode::OK,
            "/api/health must return 200 (got {status_code}) — route may not be wired"
        );

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&bytes).expect("/api/health body must be JSON");
        assert_eq!(
            body.get("status").and_then(|s| s.as_str()),
            Some("ok"),
            "status must be \"ok\", body was {body}"
        );
        assert!(
            body.get("version")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty()),
            "version must be a non-empty string, body was {body}"
        );
        assert!(
            body.get("uptime_seconds")
                .and_then(|u| u.as_u64())
                .is_some(),
            "uptime_seconds must be a u64, body was {body}"
        );
        assert!(
            body.get("build_sha").and_then(|b| b.as_str()).is_some(),
            "build_sha must be a string (may be empty), body was {body}"
        );
    }

    #[tokio::test]
    async fn graphql_v2_route_exists_and_serves_free_available_models() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/graphql/v2?op=FreeAvailableModels")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"query":"query FreeAvailableModels { __typename }","variables":{}}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status_code = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status_code, StatusCode::OK, "/graphql/v2 must respond 200, body was {body}");
        // Stub shape: data.freeAvailableModels.models = []
        let models = body
            .get("data")
            .and_then(|d| d.get("freeAvailableModels"))
            .and_then(|f| f.get("models"))
            .expect("data.freeAvailableModels.models");
        assert!(models.is_array());
        assert!(models.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn graphql_v2_unknown_op_returns_graphql_error() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/graphql/v2?op=SomeUnknownOp")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // GraphQL-shape error response, not a 4xx
        assert!(body.get("errors").is_some(), "expected GraphQL errors field, got {body}");
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
