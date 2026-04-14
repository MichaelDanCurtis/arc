use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::{Arc, Mutex};
use tracing::error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: u64,
    pub timestamp_ms: u64,
    pub actor: String,
    pub action: String,
    pub resource: String,
    pub detail: String,
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub actor: Option<String>,
    pub action: Option<String>,
}

/// Audit log backend — either in-memory (tests, dev) or Postgres-backed.
#[derive(Debug)]
enum Backend {
    InMemory {
        entries: Mutex<Vec<AuditEntry>>,
        next_id: Mutex<u64>,
    },
    Postgres {
        pool: PgPool,
    },
}

/// Audit log. Cheaply cloneable — callers share a single backend.
#[derive(Debug, Clone)]
pub struct AuditLog {
    backend: Arc<Backend>,
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditLog {
    /// In-memory audit log — used for tests and local dev without a DB.
    pub fn new() -> Self {
        Self {
            backend: Arc::new(Backend::InMemory {
                entries: Mutex::new(Vec::new()),
                next_id: Mutex::new(0),
            }),
        }
    }

    /// Postgres-backed audit log — writes persist to the `audit_log` table.
    pub fn with_pool(pool: PgPool) -> Self {
        Self {
            backend: Arc::new(Backend::Postgres { pool }),
        }
    }

    /// Record an audit event. Errors are logged but not returned — the audit
    /// log should never fail a user-facing request.
    pub async fn record(&self, actor: &str, action: &str, resource: &str, detail: &str) {
        match &*self.backend {
            Backend::InMemory { entries, next_id } => {
                let mut entries = entries.lock().unwrap();
                let mut next_id = next_id.lock().unwrap();
                *next_id += 1;
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                entries.push(AuditEntry {
                    id: *next_id,
                    timestamp_ms: now_ms,
                    actor: actor.to_string(),
                    action: action.to_string(),
                    resource: resource.to_string(),
                    detail: detail.to_string(),
                });
            }
            Backend::Postgres { pool } => {
                // actor may be a UUID string (user id) or a free-form label.
                // Parse optimistically; fall back to NULL + stash label in metadata.
                let (actor_id, actor_label) = match uuid::Uuid::parse_str(actor) {
                    Ok(id) => (Some(id), None),
                    Err(_) => (None, Some(actor.to_string())),
                };
                // Split resource "type/id" into target_type + target_id if possible.
                let (target_type, target_id) = match resource.split_once('/') {
                    Some((t, i)) if !t.is_empty() && !i.is_empty() => {
                        (Some(t.to_string()), Some(i.to_string()))
                    }
                    _ => (None, Some(resource.to_string())),
                };
                let metadata = serde_json::json!({
                    "detail": detail,
                    "actor_label": actor_label,
                });
                let result = sqlx::query(
                    "INSERT INTO audit_log (actor_id, action, target_type, target_id, metadata) \
                     VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(actor_id)
                .bind(action)
                .bind(target_type)
                .bind(target_id)
                .bind(metadata)
                .execute(pool)
                .await;
                if let Err(e) = result {
                    error!(error = %e, actor, action, "audit log insert failed");
                }
            }
        }
    }

    pub async fn query(
        &self,
        limit: usize,
        offset: usize,
        actor: Option<&str>,
        action: Option<&str>,
    ) -> Vec<AuditEntry> {
        match &*self.backend {
            Backend::InMemory { entries, .. } => {
                let entries = entries.lock().unwrap();
                entries
                    .iter()
                    .rev()
                    .filter(|e| actor.map_or(true, |a| e.actor == a))
                    .filter(|e| action.map_or(true, |a| e.action == a))
                    .skip(offset)
                    .take(limit)
                    .cloned()
                    .collect()
            }
            Backend::Postgres { pool } => {
                // Best-effort UUID parse for actor filter. If not a UUID, match
                // against metadata->>actor_label so caller can filter by free-form label too.
                let actor_uuid = actor.and_then(|a| uuid::Uuid::parse_str(a).ok());
                let sql = "SELECT id, actor_id, action, target_type, target_id, \
                                  metadata, at \
                           FROM audit_log \
                           WHERE ($1::uuid IS NULL OR actor_id = $1::uuid) \
                             AND ($2::text IS NULL OR metadata->>'actor_label' = $2) \
                             AND ($3::text IS NULL OR action = $3) \
                           ORDER BY id DESC \
                           OFFSET $4 LIMIT $5";
                let rows = sqlx::query_as::<_, AuditRow>(sql)
                    .bind(actor_uuid)
                    .bind(if actor_uuid.is_none() { actor } else { None })
                    .bind(action)
                    .bind(offset as i64)
                    .bind(limit as i64)
                    .fetch_all(pool)
                    .await;
                match rows {
                    Ok(rows) => rows.into_iter().map(AuditEntry::from).collect(),
                    Err(e) => {
                        error!(error = %e, "audit log query failed");
                        Vec::new()
                    }
                }
            }
        }
    }
}

/// Row shape for SELECT from audit_log.
#[derive(sqlx::FromRow)]
struct AuditRow {
    id: i64,
    actor_id: Option<uuid::Uuid>,
    action: String,
    target_type: Option<String>,
    target_id: Option<String>,
    metadata: Option<serde_json::Value>,
    at: chrono::DateTime<chrono::Utc>,
}

impl From<AuditRow> for AuditEntry {
    fn from(row: AuditRow) -> Self {
        let resource = match (row.target_type.as_deref(), row.target_id.as_deref()) {
            (Some(t), Some(i)) => format!("{t}/{i}"),
            (None, Some(i)) => i.to_string(),
            _ => String::new(),
        };
        let detail = row
            .metadata
            .as_ref()
            .and_then(|m| m.get("detail"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let actor_label = row
            .metadata
            .as_ref()
            .and_then(|m| m.get("actor_label"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let actor = match (row.actor_id, actor_label) {
            (Some(uid), _) => uid.to_string(),
            (None, Some(label)) => label,
            _ => String::new(),
        };
        AuditEntry {
            id: row.id.max(0) as u64,
            timestamp_ms: row.at.timestamp_millis().max(0) as u64,
            actor,
            action: row.action,
            resource,
            detail,
        }
    }
}

/// GET /api/audit — query audit log
pub async fn get_audit_log(
    State(log): State<AuditLog>,
    Query(q): Query<AuditQuery>,
) -> Result<Json<Vec<AuditEntry>>, StatusCode> {
    let entries = log
        .query(
            q.limit.unwrap_or(50),
            q.offset.unwrap_or(0),
            q.actor.as_deref(),
            q.action.as_deref(),
        )
        .await;
    Ok(Json(entries))
}

// ---------------------------------------------------------------------------
// Tests (in-memory backend only — DB tests require a live Postgres)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn audit_log_record_and_query() {
        let log = AuditLog::new();
        log.record("alice", "login", "session/1", "logged in").await;
        log.record("bob", "create", "team/2", "created team").await;
        log.record("alice", "logout", "session/1", "logged out")
            .await;

        let all = log.query(50, 0, None, None).await;
        assert_eq!(all.len(), 3);
        // Results are returned newest-first
        assert_eq!(all[0].actor, "alice");
        assert_eq!(all[0].action, "logout");
    }

    #[tokio::test]
    async fn audit_log_filter_by_actor() {
        let log = AuditLog::new();
        log.record("alice", "login", "session/1", "").await;
        log.record("bob", "login", "session/2", "").await;
        log.record("alice", "logout", "session/1", "").await;

        let alice_entries = log.query(50, 0, Some("alice"), None).await;
        assert_eq!(alice_entries.len(), 2);
        assert!(alice_entries.iter().all(|e| e.actor == "alice"));
    }

    #[tokio::test]
    async fn audit_log_filter_by_action() {
        let log = AuditLog::new();
        log.record("alice", "login", "session/1", "").await;
        log.record("bob", "create", "team/2", "").await;
        log.record("alice", "login", "session/3", "").await;

        let logins = log.query(50, 0, None, Some("login")).await;
        assert_eq!(logins.len(), 2);
        assert!(logins.iter().all(|e| e.action == "login"));
    }

    #[tokio::test]
    async fn audit_log_limit_and_offset() {
        let log = AuditLog::new();
        for i in 0..10u64 {
            log.record("actor", "action", &format!("res/{i}"), "")
                .await;
        }
        let page1 = log.query(3, 0, None, None).await;
        assert_eq!(page1.len(), 3);

        let page2 = log.query(3, 3, None, None).await;
        assert_eq!(page2.len(), 3);

        // IDs should differ between pages
        assert_ne!(page1[0].id, page2[0].id);
    }

    #[tokio::test]
    async fn audit_entry_ids_are_sequential() {
        let log = AuditLog::new();
        log.record("u", "a", "r", "").await;
        log.record("u", "b", "r", "").await;
        log.record("u", "c", "r", "").await;

        let entries = log.query(50, 0, None, None).await;
        // Returned newest-first, so ids descend
        assert!(entries[0].id > entries[1].id);
        assert!(entries[1].id > entries[2].id);
    }

    #[test]
    fn audit_entry_serializes_correctly() {
        let entry = AuditEntry {
            id: 1,
            timestamp_ms: 1_000_000,
            actor: "alice".to_string(),
            action: "login".to_string(),
            resource: "session/1".to_string(),
            detail: "logged in from CLI".to_string(),
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["id"], 1);
        assert_eq!(json["actor"], "alice");
        assert_eq!(json["action"], "login");
    }
}
