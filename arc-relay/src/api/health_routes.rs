//! `/health` — tier-one observability probe target.
//!
//! Returns 200 with JSON `{status, version, uptime_seconds, build_sha}`. No
//! auth required; no DB ping (yet). Designed to be cheap enough that an
//! external uptime probe can hit it every 60s without measurable load.
//!
//! `build_sha` is read at compile time from the `BUILD_SHA` env var (set by
//! the Dockerfile via `--build-arg`). Falls back to `"unknown"` when not set
//! — useful for `cargo run` from a developer laptop.
//!
//! `uptime_seconds` is measured from first access of `START_TIME`. To pin
//! it to true process-start, `main.rs` should `LazyLock::force(&START_TIME)`
//! once before serving — that gives accurate uptime even for the very first
//! `/health` request.

use std::sync::LazyLock;
use std::time::Instant;

use axum::Json;
use serde::Serialize;

pub static START_TIME: LazyLock<Instant> = LazyLock::new(Instant::now);

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub uptime_seconds: u64,
    pub build_sha: &'static str,
}

pub async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds: START_TIME.elapsed().as_secs(),
        build_sha: option_env!("BUILD_SHA").unwrap_or("unknown"),
    })
}
