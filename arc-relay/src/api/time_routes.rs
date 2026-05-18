//! `/api/v1/current_time` — server time endpoint for client clock-skew checks.
//!
//! Per REQUIREMENTS.md §8a (Beam-required endpoints). Beam compares this
//! response against its local clock to detect drift > N seconds; mismatch
//! is surfaced to the user before any time-sensitive operation
//! (signed-plugin verification, JWT issuance, etc.).
//!
//! Returns both unix-seconds (machine-readable) and ISO-8601 (human-readable).
//! No auth required — this is a public clock reference.

use axum::Json;
use chrono::{SecondsFormat, Utc};
use serde::Serialize;

#[derive(Serialize)]
pub struct CurrentTimeResponse {
    /// Seconds since the unix epoch (UTC). Machine-readable; primary
    /// field for clock-skew calculations.
    pub unix_seconds: i64,
    /// RFC 3339 / ISO 8601 timestamp in UTC, e.g. `2026-05-16T04:52:54Z`.
    /// Human-readable; convenient for logs.
    pub iso8601: String,
}

pub async fn current_time_handler() -> Json<CurrentTimeResponse> {
    let now = Utc::now();
    Json(CurrentTimeResponse {
        unix_seconds: now.timestamp(),
        iso8601: now.to_rfc3339_opts(SecondsFormat::Secs, true),
    })
}
