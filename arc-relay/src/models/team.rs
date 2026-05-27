use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Team {
    pub id: Uuid,
    pub name: String,
    pub owner_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TeamMember {
    pub team_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
}

pub async fn create_team(pool: &PgPool, name: &str, owner_id: Uuid) -> Result<Team, sqlx::Error> {
    let id = Uuid::new_v4();
    let team = sqlx::query_as::<_, Team>(
        "INSERT INTO teams (id, name, owner_id) VALUES ($1, $2, $3) RETURNING *",
    )
    .bind(id)
    .bind(name)
    .bind(owner_id)
    .fetch_one(pool)
    .await?;

    sqlx::query("INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, 'owner')")
        .bind(id)
        .bind(owner_id)
        .execute(pool)
        .await?;

    Ok(team)
}

pub async fn list_teams_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<Team>, sqlx::Error> {
    sqlx::query_as::<_, Team>(
        "SELECT t.id, t.name, t.owner_id, t.created_at \
         FROM teams t \
         JOIN team_members tm ON tm.team_id = t.id \
         WHERE tm.user_id = $1 \
         ORDER BY t.created_at ASC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

#[allow(dead_code)]
pub async fn add_team_member(
    pool: &PgPool,
    team_id: Uuid,
    user_id: Uuid,
    role: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, $3) \
         ON CONFLICT (team_id, user_id) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(team_id)
    .bind(user_id)
    .bind(role)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_team_members(
    pool: &PgPool,
    team_id: Uuid,
) -> Result<Vec<TeamMember>, sqlx::Error> {
    sqlx::query_as::<_, TeamMember>(
        "SELECT team_id, user_id, role FROM team_members WHERE team_id = $1",
    )
    .bind(team_id)
    .fetch_all(pool)
    .await
}

/// Returns `true` if `user_id` has any row in `team_members` for `team_id`.
///
/// Used by surfaces that mutate team-scoped state on behalf of the caller
/// (e.g. marketplace install) to enforce that the caller actually belongs
/// to the team they're acting on. Returning `false` here means the caller
/// should be rejected with `403 Forbidden`, not `404`, so the caller can
/// distinguish "team exists, you can't touch it" from "team doesn't exist".
pub async fn is_team_member(
    pool: &PgPool,
    team_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 AS n FROM team_members WHERE team_id = $1 AND user_id = $2 LIMIT 1",
    )
    .bind(team_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Look up a team's `org_id`, if any.
///
/// Returns `Ok(None)` when the team has no org (personal / unaffiliated
/// team) and `Ok(Some(org_id))` when it does. Returns `RowNotFound` when
/// the team itself doesn't exist, so callers can map that to `404`.
pub async fn get_team_org_id(
    pool: &PgPool,
    team_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT org_id FROM teams WHERE id = $1",
    )
    .bind(team_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((org_id,)) => Ok(org_id),
        None => Err(sqlx::Error::RowNotFound),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_pool_url() -> Option<String> {
        std::env::var("DATABASE_URL").ok()
    }

    #[tokio::test]
    #[ignore]
    async fn test_create_team_adds_owner_member() {
        let url = match get_pool_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        let owner_id = Uuid::new_v4();
        // Insert a user first (FK constraint)
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x')")
            .bind(owner_id)
            .bind(format!("team_owner_{}@test.invalid", owner_id))
            .execute(&pool)
            .await
            .expect("insert user");

        let team = create_team(&pool, "Engineering", owner_id)
            .await
            .expect("create_team");
        assert_eq!(team.name, "Engineering");
        assert_eq!(team.owner_id, owner_id);

        let members = list_team_members(&pool, team.id)
            .await
            .expect("list_team_members");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].user_id, owner_id);
        assert_eq!(members[0].role, "owner");
    }

    #[tokio::test]
    #[ignore]
    async fn test_list_teams_for_user() {
        let url = match get_pool_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        let owner_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x')")
            .bind(owner_id)
            .bind(format!("list_teams_{}@test.invalid", owner_id))
            .execute(&pool)
            .await
            .expect("insert user");

        create_team(&pool, "Team A", owner_id)
            .await
            .expect("team a");
        create_team(&pool, "Team B", owner_id)
            .await
            .expect("team b");

        let teams = list_teams_for_user(&pool, owner_id).await.expect("list");
        assert_eq!(teams.len(), 2);
    }

    /// M2 / G1 — is_team_member is the boundary the install handler trusts.
    #[tokio::test]
    #[ignore]
    async fn test_is_team_member_true_and_false() {
        let url = match get_pool_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        let owner = Uuid::new_v4();
        let outsider = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x'), ($3, $4, 'x')",
        )
        .bind(owner)
        .bind(format!("ism_owner_{owner}@test.invalid"))
        .bind(outsider)
        .bind(format!("ism_outsider_{outsider}@test.invalid"))
        .execute(&pool)
        .await
        .expect("insert users");

        let team = create_team(&pool, "MemberTest", owner)
            .await
            .expect("create_team");

        assert!(
            is_team_member(&pool, team.id, owner)
                .await
                .expect("owner check"),
            "create_team must register the owner as a team_member",
        );
        assert!(
            !is_team_member(&pool, team.id, outsider)
                .await
                .expect("outsider check"),
            "users with no team_members row must not be considered members",
        );
    }

    /// M2 / G2 — get_team_org_id powers the org-allowlist lookup.
    #[tokio::test]
    #[ignore]
    async fn test_get_team_org_id_returns_none_and_some() {
        let url = match get_pool_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        let owner = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x')")
            .bind(owner)
            .bind(format!("gtoi_owner_{owner}@test.invalid"))
            .execute(&pool)
            .await
            .expect("insert user");

        let team = create_team(&pool, "OrgTest", owner)
            .await
            .expect("create team");

        // Default: team has no org.
        let org_id = get_team_org_id(&pool, team.id)
            .await
            .expect("lookup pre-attach");
        assert!(org_id.is_none(), "fresh team must have no org_id");

        // Attach to an org.
        let oid = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, $2, $3)")
            .bind(oid)
            .bind("OrgTest")
            .bind(format!("orgtest-{oid}"))
            .execute(&pool)
            .await
            .expect("insert org");
        sqlx::query("UPDATE teams SET org_id = $1 WHERE id = $2")
            .bind(oid)
            .bind(team.id)
            .execute(&pool)
            .await
            .expect("attach team");

        let org_id = get_team_org_id(&pool, team.id)
            .await
            .expect("lookup post-attach");
        assert_eq!(org_id, Some(oid));

        // Missing team_id surfaces as RowNotFound so handlers can map to 404.
        let bogus = Uuid::new_v4();
        let err = get_team_org_id(&pool, bogus).await.unwrap_err();
        assert!(
            matches!(err, sqlx::Error::RowNotFound),
            "missing team must yield RowNotFound, got {err:?}",
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_add_team_member() {
        let url = match get_pool_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        let owner_id = Uuid::new_v4();
        let member_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x'), ($3, $4, 'x')",
        )
        .bind(owner_id)
        .bind(format!("atm_owner_{}@test.invalid", owner_id))
        .bind(member_id)
        .bind(format!("atm_member_{}@test.invalid", member_id))
        .execute(&pool)
        .await
        .expect("insert users");

        let team = create_team(&pool, "AddMember Team", owner_id)
            .await
            .expect("create_team");

        add_team_member(&pool, team.id, member_id, "member")
            .await
            .expect("add_member");

        let members = list_team_members(&pool, team.id).await.expect("list");
        assert_eq!(members.len(), 2);
    }
}
