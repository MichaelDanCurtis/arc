//! IPC protocol types shared between beacon-hostd and terminal apps
//! (e.g. the Mac client via beacon-ffi).
//!
//! Wire format: 4-byte big-endian length + msgpack body (encoded with
//! `rmp_serde::to_vec_named`).
//!
//! These types live in `beacon-proto` so that both the daemon and the FFI
//! shim can produce/consume byte-identical frames without duplicating the
//! schema. `IpcSessionInfo::state` is a free-form `String` rather than the
//! richer `beacon_session::SessionState` so this crate can stay free of a
//! `beacon-session` dependency — hostd converts at the IPC boundary.

use crate::codec::{decode, encode, CodecError};
use beacon_model::{BlockFooter, BlockHeader, BlockId, SessionId};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Request from a terminal app to hostd over IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum IpcRequest {
    RegisterSession {
        session_id: SessionId,
        name: String,
    },
    UnregisterSession {
        session_id: SessionId,
    },
    PushBlockStart {
        session_id: SessionId,
        header: BlockHeader,
    },
    PushBlockChunk {
        session_id: SessionId,
        block_id: BlockId,
        seq: u64,
        bytes: Vec<u8>,
    },
    PushBlockEnd {
        session_id: SessionId,
        footer: BlockFooter,
    },
    Resize {
        session_id: SessionId,
        cols: u16,
        rows: u16,
    },
    Status,
    // ── Session-mirroring variants ────────────────────────────────────────
    /// Push raw PTY output bytes with a monotonic sequence number.
    PushOutput {
        session_id: SessionId,
        bytes: Vec<u8>,
        seq: u64,
    },
    /// Signal a gap in the output stream (e.g. after reconnect).
    OutputDiscontinuity {
        session_id: SessionId,
        last_seq: u64,
    },
    /// Subscribe to the mirrored output stream of a session.
    Subscribe {
        session_id: SessionId,
    },
    /// Unsubscribe from the mirrored output stream of a session.
    Unsubscribe {
        session_id: SessionId,
    },
    /// Grant keyboard-control of a session to another user.
    GrantControl {
        session_id: SessionId,
        to_user: String,
        request_id: String,
    },
    /// Deny a pending control request.
    DenyControl {
        session_id: SessionId,
        request_id: String,
    },
    /// Revoke active keyboard control from the current driver.
    RevokeControl {
        session_id: SessionId,
    },
}

/// Response from hostd to a terminal app over IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum IpcResponse {
    Ok,
    SessionList { sessions: Vec<IpcSessionInfo> },
    Error { message: String },
}

/// Session info returned in Status responses.
///
/// `state` is a string (e.g. "Created" / "Registered" / "Active" /
/// "Disconnected") so this struct doesn't pull in `beacon-session`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcSessionInfo {
    pub id: SessionId,
    pub name: String,
    pub state: String,
    pub buffered_messages: usize,
}

/// Hostd → terminal-app event stream (one-way push of async notifications).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum IpcEvent {
    /// Raw PTY output bytes forwarded from the owning session.
    RemoteInput {
        session_id: SessionId,
        bytes: Vec<u8>,
        from_user: String,
    },
    /// The keyboard-control lease changed (new driver or lease dropped).
    LeaseChanged {
        session_id: SessionId,
        driver: Option<DriverInfo>,
    },
    /// Another user is requesting keyboard control of this session.
    ControlRequested {
        session_id: SessionId,
        request_id: String,
        from_user: String,
        from_device: String,
    },
    /// The set of watchers observing this session changed.
    PresenceChanged {
        session_id: SessionId,
        watchers: Vec<WatcherInfo>,
    },
}

impl IpcEvent {
    /// Return the session ID this event is associated with.
    pub fn session_id(&self) -> &SessionId {
        match self {
            Self::RemoteInput { session_id, .. }
            | Self::LeaseChanged { session_id, .. }
            | Self::ControlRequested { session_id, .. }
            | Self::PresenceChanged { session_id, .. } => session_id,
        }
    }
}

/// Details about the user currently holding keyboard-control of a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverInfo {
    pub user_id: String,
    pub device_label: String,
    /// Unix timestamp (seconds) when the lease was granted.
    pub since: i64,
}

/// Details about a single watcher observing a mirrored session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatcherInfo {
    pub user_id: String,
    pub device_label: String,
    pub can_drive: bool,
}

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("codec: {0}")]
    Codec(#[from] CodecError),
    #[error("frame too large: {0} bytes")]
    FrameTooLarge(usize),
}

pub const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024; // 16MB

/// Read a length-delimited IPC request from a stream.
/// Format: 4-byte big-endian length + msgpack body.
pub async fn read_ipc_request<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Option<IpcRequest>, IpcError> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_SIZE {
        return Err(IpcError::FrameTooLarge(len));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    let req: IpcRequest = decode(&body)?;
    Ok(Some(req))
}

/// Write a length-delimited IPC response to a stream.
pub async fn write_ipc_response<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    response: &IpcResponse,
) -> Result<(), IpcError> {
    let body = encode(response)?;
    let len = (body.len() as u32).to_be_bytes();
    writer.write_all(&len).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

/// Read a length-delimited IPC event from a stream.
/// Format: 4-byte big-endian length + msgpack body.
pub async fn read_ipc_event<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Option<IpcEvent>, IpcError> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_SIZE {
        return Err(IpcError::FrameTooLarge(len));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    let evt: IpcEvent = decode(&body)?;
    Ok(Some(evt))
}

/// Write a length-delimited IPC event to a stream.
pub async fn write_ipc_event<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    event: &IpcEvent,
) -> Result<(), IpcError> {
    let body = encode(event)?;
    let len = (body.len() as u32).to_be_bytes();
    writer.write_all(&len).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_request_round_trip_msgpack() {
        let req = IpcRequest::RegisterSession {
            session_id: SessionId::new(),
            name: "test".into(),
        };
        let bytes = encode(&req).unwrap();
        let back: IpcRequest = decode(&bytes).unwrap();
        assert!(matches!(back, IpcRequest::RegisterSession { .. }));
    }

    #[test]
    fn ipc_response_round_trip_msgpack() {
        let resp = IpcResponse::Ok;
        let bytes = encode(&resp).unwrap();
        let back: IpcResponse = decode(&bytes).unwrap();
        assert!(matches!(back, IpcResponse::Ok));
    }

    #[test]
    fn ipc_session_list_round_trip() {
        let resp = IpcResponse::SessionList {
            sessions: vec![IpcSessionInfo {
                id: SessionId::new(),
                name: "shell".into(),
                state: "Active".into(),
                buffered_messages: 42,
            }],
        };
        let bytes = encode(&resp).unwrap();
        let back: IpcResponse = decode(&bytes).unwrap();
        assert!(matches!(back, IpcResponse::SessionList { .. }));
    }
}
