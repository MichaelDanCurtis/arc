use beacon_model::{BlockFooter, BlockHeader, BlockId, SessionId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum ClientToArc {
    // Host-side: session lifecycle
    HostHello {
        host_id: String,
        auth_token: String,
        protocol_version: u16,
    },
    SessionRegister {
        session_id: SessionId,
        name: String,
    },
    SessionUnregister {
        session_id: SessionId,
    },

    // Host-side: block stream
    BlockStart {
        session_id: SessionId,
        header: BlockHeader,
    },
    BlockOutputChunk {
        session_id: SessionId,
        block_id: BlockId,
        seq: u64,
        bytes: Vec<u8>,
    },
    BlockEnd {
        session_id: SessionId,
        footer: BlockFooter,
    },
    SessionResize {
        session_id: SessionId,
        cols: u16,
        rows: u16,
    },

    // Client-side: attach + input
    AttachSession {
        session_id: SessionId,
        since_block: Option<BlockId>,
    },
    DetachSession {
        session_id: SessionId,
    },
    SessionInput {
        session_id: SessionId,
        bytes: Vec<u8>,
    },
    SessionResizeRequest {
        session_id: SessionId,
        cols: u16,
        rows: u16,
    },

    Pong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum ArcToClient {
    AttachAck {
        session_id: SessionId,
        current_cols: u16,
        current_rows: u16,
        resume_from: Option<BlockId>,
    },
    AttachDenied {
        session_id: SessionId,
        reason: String,
    },
    SessionGone {
        session_id: SessionId,
        reason: String,
    },

    BlockStart {
        session_id: SessionId,
        header: BlockHeader,
    },
    BlockOutputChunk {
        session_id: SessionId,
        block_id: BlockId,
        seq: u64,
        bytes: Vec<u8>,
    },
    BlockEnd {
        session_id: SessionId,
        footer: BlockFooter,
    },

    ReplayBegin {
        session_id: SessionId,
        total_blocks: u32,
    },
    ReplayBlock {
        session_id: SessionId,
        block: beacon_model::Block,
    },
    ReplayEnd {
        session_id: SessionId,
    },

    Ping,
    Pong,

    Error {
        code: String,
        message: String,
    },

    HelloAck {
        protocol_version: u16,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_to_arc_variants_serde_distinctly() {
        let a = ClientToArc::DetachSession {
            session_id: SessionId::new(),
        };
        let s = serde_json::to_string(&a).unwrap();
        assert!(s.contains("DetachSession"), "got: {s}");
    }

    #[test]
    fn hello_ack_round_trip() {
        let ack = ArcToClient::HelloAck {
            protocol_version: 1,
        };
        let s = serde_json::to_string(&ack).unwrap();
        let back: ArcToClient = serde_json::from_str(&s).unwrap();
        assert_eq!(ack, back);
    }

    #[test]
    fn arc_to_client_ping_pong() {
        let p = ArcToClient::Ping;
        let s = serde_json::to_string(&p).unwrap();
        let back: ArcToClient = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, ArcToClient::Ping));
    }
}
