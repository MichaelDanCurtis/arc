use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

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

/// In-memory audit log (will be replaced by DB in production)
#[derive(Debug, Clone, Default)]
pub struct AuditLog {
    entries: Arc<Mutex<Vec<AuditEntry>>>,
    next_id: Arc<Mutex<u64>>,
}

impl AuditLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, actor: &str, action: &str, resource: &str, detail: &str) {
        let mut entries = self.entries.lock().unwrap();
        let mut next_id = self.next_id.lock().unwrap();
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

    pub fn query(
        &self,
        limit: usize,
        offset: usize,
        actor: Option<&str>,
        action: Option<&str>,
    ) -> Vec<AuditEntry> {
        let entries = self.entries.lock().unwrap();
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
}

/// GET /api/audit — query audit log
pub async fn get_audit_log(
    State(log): State<AuditLog>,
    Query(q): Query<AuditQuery>,
) -> Json<Vec<AuditEntry>> {
    let entries = log.query(
        q.limit.unwrap_or(50),
        q.offset.unwrap_or(0),
        q.actor.as_deref(),
        q.action.as_deref(),
    );
    Json(entries)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_log_record_and_query() {
        let log = AuditLog::new();
        log.record("alice", "login", "session/1", "logged in");
        log.record("bob", "create", "team/2", "created team");
        log.record("alice", "logout", "session/1", "logged out");

        let all = log.query(50, 0, None, None);
        assert_eq!(all.len(), 3);
        // Results are returned newest-first
        assert_eq!(all[0].actor, "alice");
        assert_eq!(all[0].action, "logout");
    }

    #[test]
    fn audit_log_filter_by_actor() {
        let log = AuditLog::new();
        log.record("alice", "login", "session/1", "");
        log.record("bob", "login", "session/2", "");
        log.record("alice", "logout", "session/1", "");

        let alice_entries = log.query(50, 0, Some("alice"), None);
        assert_eq!(alice_entries.len(), 2);
        assert!(alice_entries.iter().all(|e| e.actor == "alice"));
    }

    #[test]
    fn audit_log_filter_by_action() {
        let log = AuditLog::new();
        log.record("alice", "login", "session/1", "");
        log.record("bob", "create", "team/2", "");
        log.record("alice", "login", "session/3", "");

        let logins = log.query(50, 0, None, Some("login"));
        assert_eq!(logins.len(), 2);
        assert!(logins.iter().all(|e| e.action == "login"));
    }

    #[test]
    fn audit_log_limit_and_offset() {
        let log = AuditLog::new();
        for i in 0..10u64 {
            log.record("actor", "action", &format!("res/{i}"), "");
        }
        let page1 = log.query(3, 0, None, None);
        assert_eq!(page1.len(), 3);

        let page2 = log.query(3, 3, None, None);
        assert_eq!(page2.len(), 3);

        // IDs should differ between pages
        assert_ne!(page1[0].id, page2[0].id);
    }

    #[test]
    fn audit_entry_ids_are_sequential() {
        let log = AuditLog::new();
        log.record("u", "a", "r", "");
        log.record("u", "b", "r", "");
        log.record("u", "c", "r", "");

        let entries = log.query(50, 0, None, None);
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
