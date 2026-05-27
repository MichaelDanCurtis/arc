//! Admin-only API routes.
//!
//! Routes here MUST be wired behind the [`RequireAdmin`] extractor (or
//! a stricter equivalent). The first endpoint is the tier-provisioning
//! handler that activates the existing `RequirePro` gating already in
//! the marketplace router: until someone with admin can mark a user
//! `pro`, every `pro`-gated route on the system returns 403.
//!
//! See `arc-009 / A3` for the design.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::middleware::RequireAdmin;
use crate::models::user::User;

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Body for `PATCH /api/v1/admin/users/:user_id/tier`.
///
/// `tier` must be one of `"free"` or `"pro"`. Anything else is rejected
/// with `400 Bad Request`. The validation lives in the handler (not at
/// the serde layer) so the error response can carry a useful message.
#[derive(Debug, Deserialize)]
pub struct SetTierRequest {
    pub tier: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// PATCH /api/v1/admin/users/:user_id/tier
///
/// Flip a user's entitlement tier between `'free'` and `'pro'`. This
/// is the activation endpoint for the `RequirePro` gating already
/// landed in main (marketplace publish, etc.) — without it, no one
/// can be marked `pro` and every pro-gated route returns 403.
///
/// **Authorisation**: caller must be an admin (one or more
/// `org_memberships` rows with `role = 'admin'`). Enforced by the
/// `RequireAdmin` extractor; non-admins get 403, missing/invalid JWT
/// gets 401.
///
/// **Responses**:
/// - `200 OK` — updated user as JSON (the full `User` row, sans
///   sensitive fields per the struct's serde derivation).
/// - `400 Bad Request` — `tier` field missing or not in
///   `{"free", "pro"}`.
/// - `401 Unauthorized` — no/invalid JWT (from `RequireAdmin`).
/// - `403 Forbidden` — JWT valid but caller is not admin
///   (from `RequireAdmin`).
/// - `404 Not Found` — `user_id` does not exist.
/// - `500 Internal Server Error` — unexpected DB failure.
pub async fn patch_user_tier(
    State(state): State<AppState>,
    RequireAdmin(_admin_id): RequireAdmin,
    Path(user_id): Path<Uuid>,
    Json(req): Json<SetTierRequest>,
) -> Result<Json<User>, (StatusCode, String)> {
    // Whitelist the tier values to avoid arbitrary string injection
    // into the `tier` column. Even though the column is constrained
    // by migration `006_user_tier.sql`, validating here gives a
    // useful 400 response instead of a 500 from a check-constraint
    // violation.
    let tier = req.tier.as_str();
    if tier != "free" && tier != "pro" {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("tier must be \"free\" or \"pro\" (got {:?})", req.tier),
        ));
    }

    let updated = sqlx::query_as::<_, User>(
        "UPDATE users SET tier = $1 WHERE id = $2 RETURNING *",
    )
    .bind(tier)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?
    .ok_or((StatusCode::NOT_FOUND, "User not found".to_string()))?;

    Ok(Json(updated))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_tier_request_deserializes_free() {
        let req: SetTierRequest = serde_json::from_str(r#"{"tier":"free"}"#).unwrap();
        assert_eq!(req.tier, "free");
    }

    #[test]
    fn set_tier_request_deserializes_pro() {
        let req: SetTierRequest = serde_json::from_str(r#"{"tier":"pro"}"#).unwrap();
        assert_eq!(req.tier, "pro");
    }

    #[test]
    fn set_tier_request_rejects_missing_field() {
        let result: Result<SetTierRequest, _> = serde_json::from_str(r#"{}"#);
        assert!(result.is_err(), "missing tier must fail to deserialize");
    }
}
