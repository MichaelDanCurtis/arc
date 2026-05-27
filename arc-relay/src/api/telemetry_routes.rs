//! `/api/v1/telemetry` — opt-in event sink for Beam client telemetry.
//!
//! Per REQUIREMENTS.md §8a (Beam-required endpoints, task arc-015 / D2).
//! Beam's default telemetry posture is **off**; clients explicitly opt in
//! before the runtime starts POSTing events here. The wire contract is
//! deliberately permissive (arbitrary JSON) so the schema can evolve on
//! the client without a relay redeploy.
//!
//! ### Auth
//!
//! Bearer JWT is **required**. No anonymous-token path exists on arc-relay
//! (REQUIREMENTS.md §1 explicitly rejected one on 2026-05-16 — Beam users
//! without an account run offline-only). The [`AuthUser`] extractor
//! returns `401 Unauthorized` for missing/invalid tokens before the
//! handler body runs, so event ingestion is always tied to a real user.
//!
//! ### Storage
//!
//! v0 is **log-only**: events are emitted via `tracing::info!` tagged
//! with the authenticated `user_id`. No `telemetry_events` table exists
//! yet — when a real analytics pipeline arrives (ClickHouse / S3 sink /
//! external tool), we'll add persistence behind this same wire contract.
//! Log-and-ack is enough to validate the protocol and unblock Beam.
//!
//! ### Response
//!
//! Always `204 No Content` on a successful auth + JSON decode. The
//! handler is intentionally fire-and-forget — clients SHOULD NOT block
//! UX on the response.

use axum::{http::StatusCode, Json};

use crate::auth::middleware::AuthUser;

/// Accept an opt-in telemetry event from an authenticated Beam client.
///
/// The body is arbitrary JSON — schema lives on the client side and may
/// evolve without a server change. We log the payload with the
/// authenticated `user_id` and acknowledge with `204`.
pub async fn telemetry_handler(
    AuthUser(user_id): AuthUser,
    Json(event): Json<serde_json::Value>,
) -> StatusCode {
    tracing::info!(user_id = %user_id, event = ?event, "telemetry event received");
    StatusCode::NO_CONTENT
}
