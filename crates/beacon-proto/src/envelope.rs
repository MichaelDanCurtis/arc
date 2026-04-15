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

    // Shadow diff proxy: host replies to Arc with the diff.
    ShadowDiffResponse {
        session_id: SessionId,
        request_id: String,
        diff: String,
    },

    // Host-side: raw PTY output stream (for mirroring)
    PushOutput {
        session_id: SessionId,
        bytes: Vec<u8>,
        seq: u64,
    },
    /// Host-side: signal a gap in the output stream (after OutputTee drops).
    /// Pairs with ScrollbackRing's out-of-range detection on the relay side.
    OutputDiscontinuity {
        session_id: SessionId,
        last_seq: u64,
    },

    // Session mirroring: subscribe/control
    SubscribeSession {
        session_id: SessionId,
        since_seq: Option<u64>,
    },
    UnsubscribeSession {
        session_id: SessionId,
    },
    RequestControl {
        session_id: SessionId,
    },
    ApproveControl {
        session_id: SessionId,
        request_id: String,
    },
    DenyControl {
        session_id: SessionId,
        request_id: String,
    },
    RevokeControl {
        session_id: SessionId,
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
    /// The session has been terminated or evicted — it no longer exists.
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

    // Shadow diff proxy: Arc asks the host to produce a diff for a session.
    ShadowDiffRequest {
        session_id: SessionId,
        request_id: String,
    },

    // Session mirroring: output stream + control events

    /// PTY output from the session. `replay: true` means this chunk is part
    /// of the catch-up scroll-back delivered right after subscribe;
    /// `replay: false` means it's a live byte from the session as it happened.
    /// Downstream clients should not treat replay chunks as new terminal events
    /// for UI purposes (no beep, no notification, no activity indicator).
    SessionOutput {
        session_id: SessionId,
        bytes: Vec<u8>,
        seq: u64,
        replay: bool,
    },
    SessionDiscontinuity {
        session_id: SessionId,
        last_seq: u64,
    },
    LeaseChanged {
        session_id: SessionId,
        driver: Option<crate::ipc::DriverInfo>,
    },
    ControlRequested {
        session_id: SessionId,
        request_id: String,
        from_user: String,
        from_device: String,
    },
    PresenceChanged {
        session_id: SessionId,
        watchers: Vec<crate::ipc::WatcherInfo>,
    },
    /// Keystroke echo from the session's current driver (host), forwarded to
    /// every subscribed watcher so their local UI can show what's being typed.
    /// Paired with `ClientToArc::SessionInput` (which travels the other direction:
    /// watcher-turned-driver → Arc → host).
    HostInput {
        session_id: SessionId,
        bytes: Vec<u8>,
        from_user: String,
    },
    /// The user's subscription was revoked but the session still exists
    /// (e.g. host stopped sharing with this user, or a permission change).
    /// Contrast with `SessionGone`, which means the session is actually gone.
    SessionRevoked {
        session_id: SessionId,
        reason: String,
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

    #[test]
    fn shadow_diff_request_response_round_trip() {
        let sid = SessionId::new();

        // Request flows Arc -> host, so it lives on ArcToClient.
        let req = ArcToClient::ShadowDiffRequest {
            session_id: sid,
            request_id: "req-001".to_string(),
        };
        let req_json = serde_json::to_string(&req).unwrap();
        let req_back: ArcToClient = serde_json::from_str(&req_json).unwrap();
        assert_eq!(req, req_back);

        // Response flows host -> Arc, so it stays on ClientToArc.
        let resp = ClientToArc::ShadowDiffResponse {
            session_id: sid,
            request_id: "req-001".to_string(),
            diff: "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+new".to_string(),
        };
        let resp_json = serde_json::to_string(&resp).unwrap();
        let resp_back: ClientToArc = serde_json::from_str(&resp_json).unwrap();
        assert_eq!(resp, resp_back);
    }
}
