use crate::{SessionId, TabId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Minimal Tab stub — Plan 2 (beacon-session) will grow this with state machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub cwd: PathBuf,
}

/// Minimal Session stub — Plan 2 extends with local/remote variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub tabs: Vec<Tab>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_contains_tabs_and_is_serde_safe() {
        let s = Session {
            id: SessionId::new(),
            name: "main".to_string(),
            tabs: vec![Tab {
                id: TabId::new(),
                title: "shell".to_string(),
                cwd: PathBuf::from("/tmp"),
            }],
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
