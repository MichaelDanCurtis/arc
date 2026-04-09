use crate::{BlockId, HostInfo, SessionId, TabId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// Opaque shadow git commit reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ShadowCommitId(pub String);

/// Sent at the start of a command: everything knowable before the command runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    pub id: BlockId,
    pub session_id: SessionId,
    pub tab_id: TabId,
    pub command: String,
    pub cwd: PathBuf,
    pub started_at: SystemTime,
    pub env_snapshot: HashMap<String, String>,
    pub host_info: HostInfo,
}

/// Sent when the command ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockFooter {
    pub id: BlockId,
    pub exit_code: Option<i32>,
    pub ended_at: SystemTime,
    pub shadow_git_ref: Option<ShadowCommitId>,
}

/// Canonical structured Block — the source of truth across storage,
/// wire protocol, AI context, and autocomplete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    /// Raw UTF-8 bytes with ANSI escape sequences preserved.
    pub output: Vec<u8>,
    /// `None` while the block is still running.
    pub footer: Option<BlockFooter>,
}

impl Block {
    pub fn is_complete(&self) -> bool {
        self.footer.is_some()
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.footer.as_ref().and_then(|f| f.exit_code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_header() -> BlockHeader {
        BlockHeader {
            id: BlockId::new(),
            session_id: SessionId::new(),
            tab_id: TabId::new(),
            command: "cargo test".to_string(),
            cwd: PathBuf::from("/tmp"),
            started_at: SystemTime::UNIX_EPOCH,
            env_snapshot: HashMap::new(),
            host_info: HostInfo::current(),
        }
    }

    #[test]
    fn header_is_serde_round_trip_safe() {
        let h = sample_header();
        let json = serde_json::to_string(&h).unwrap();
        let back: BlockHeader = serde_json::from_str(&json).unwrap();
        assert_eq!(h.id, back.id);
        assert_eq!(h.command, back.command);
    }

    #[test]
    fn block_pairs_header_and_footer() {
        let h = sample_header();
        let f = BlockFooter {
            id: h.id,
            exit_code: Some(0),
            ended_at: SystemTime::UNIX_EPOCH,
            shadow_git_ref: None,
        };
        let b = Block {
            header: h.clone(),
            output: b"hello\n".to_vec(),
            footer: Some(f.clone()),
        };
        assert_eq!(b.header.id, f.id);
        assert_eq!(b.output, b"hello\n");
        assert!(b.is_complete());
    }

    #[test]
    fn running_block_is_not_complete() {
        let h = sample_header();
        let b = Block {
            header: h,
            output: Vec::new(),
            footer: None,
        };
        assert!(!b.is_complete());
        assert_eq!(b.exit_code(), None);
    }
}
