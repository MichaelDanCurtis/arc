//! Permission gating for session mirror subscriptions.
//!
//! [`InvitationStore`] answers a single question: is a given user allowed to
//! subscribe to a session's mirror feed, and can they request drive control?
//!
//! The check follows a four-tier fallback:
//!
//! 1. **Owner** — the `sessions.owner_id` always has full access.
//! 2. **User grant** — a `session_grants` row with a matching `grantee_user_id`
//!    and no `revoked_at`.
//! 3. **Email grant** — a `session_grants` row with a matching `target_email`
//!    (case-insensitive) and no `revoked_at`.
//! 4. **Team grant** — a `session_grants` row with a `target_team_id` whose
//!    corresponding `team_members` row contains the requesting user, and no
//!    `revoked_at`.
//!
//! If none of the above match the result is [`SubscribePermission::Denied`].

use sqlx::PgPool;
use uuid::Uuid;

/// Checks whether a user may subscribe to a mirrored session.
pub struct InvitationStore {
    pool: PgPool,
}

/// Result of a permission check for subscribing to a session mirror feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscribePermission {
    /// The requesting user is the session owner — unrestricted access.
    Owner,
    /// The user holds an active grant.
    ///
    /// `can_drive` reflects the `can_request_control` column on the grant row.
    /// When `false` the subscriber may watch but must not send control requests.
    Granted { can_drive: bool },
    /// No matching grant found; the subscription must be rejected.
    Denied,
}

impl InvitationStore {
    /// Create a new store backed by the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Check whether `user_id` / `user_email` may subscribe to `session_id`.
    ///
    /// All queries use runtime-only sqlx (no `!` macros) so that `cargo check`
    /// succeeds without a live database at compile time.
    pub async fn check_subscribe_permission(
        &self,
        session_id: &str,
        user_id: Uuid,
        user_email: &str,
    ) -> sqlx::Result<SubscribePermission> {
        // ------------------------------------------------------------------
        // 1. Owner check
        // ------------------------------------------------------------------
        let owner: Option<Uuid> = sqlx::query_scalar(
            "SELECT owner_id FROM sessions WHERE id = $1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;

        match owner {
            None => return Ok(SubscribePermission::Denied),
            Some(o) if o == user_id => return Ok(SubscribePermission::Owner),
            _ => {}
        }

        // ------------------------------------------------------------------
        // 2. Direct user grant
        // ------------------------------------------------------------------
        let user_grant: Option<(bool,)> = sqlx::query_as(
            "SELECT can_request_control FROM session_grants \
             WHERE session_id = $1 AND grantee_user_id = $2 AND revoked_at IS NULL",
        )
        .bind(session_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((can_drive,)) = user_grant {
            return Ok(SubscribePermission::Granted { can_drive });
        }

        // ------------------------------------------------------------------
        // 3. Email grant (case-insensitive)
        // ------------------------------------------------------------------
        // Note: Postgres three-valued logic means `lower(NULL) = lower($2)`
        // evaluates to NULL (not TRUE), so rows with `target_email IS NULL`
        // are silently excluded. No explicit `AND target_email IS NOT NULL`
        // predicate is needed.
        let email_grant: Option<(bool,)> = sqlx::query_as(
            "SELECT can_request_control FROM session_grants \
             WHERE session_id = $1 AND lower(target_email) = lower($2) AND revoked_at IS NULL",
        )
        .bind(session_id)
        .bind(user_email)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((can_drive,)) = email_grant {
            return Ok(SubscribePermission::Granted { can_drive });
        }

        // ------------------------------------------------------------------
        // 4. Team grant — user must be a member of the granted team
        // ------------------------------------------------------------------
        // If the user is in multiple granted teams, take the most permissive
        // grant — grants are purely additive, and a user shouldn't lose drive
        // privilege because of team iteration order. ORDER BY ... DESC LIMIT 1
        // makes the result deterministic across calls.
        let team_grant: Option<(bool,)> = sqlx::query_as(
            "SELECT g.can_request_control FROM session_grants g \
             JOIN team_members tm ON tm.team_id = g.target_team_id \
             WHERE g.session_id = $1 AND tm.user_id = $2 AND g.revoked_at IS NULL \
             ORDER BY g.can_request_control DESC \
             LIMIT 1",
        )
        .bind(session_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((can_drive,)) = team_grant {
            return Ok(SubscribePermission::Granted { can_drive });
        }

        Ok(SubscribePermission::Denied)
    }
}
