use arc_relay::{api, audit, config, db, relay, retention};
use std::sync::Arc;

use tower_http::cors::{Any, CorsLayer};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "arc_relay=info".into()),
        )
        .init();

    let config = config::ArcConfig::from_env().expect("invalid configuration");
    let pool = db::init_pool(&config.database_url).await?;
    db::run_migrations(&pool).await?;

    let relay_router = Arc::new(relay::router::RelayRouter::new());
    let audit_log = audit::AuditLog::with_pool(pool.clone());
    let app_state = api::AppState {
        pool: pool.clone(),
        config: Arc::new(config.clone()),
        relay: relay_router.clone(),
        audit: audit_log,
        mirror_registry: arc_relay::mirror::MirrorRegistry::default(),
        invitations: arc_relay::mirror::invitation::InvitationStore::new(pool.clone()),
    };

    retention::spawn_retention_task(pool.clone());

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);
    let app = api::build_router(app_state).layer(cors);

    let addr = &config.bind_addr;
    tracing::info!("arc-relay listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
