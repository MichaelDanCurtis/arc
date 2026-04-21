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

/// Strategy when installing over an existing plugin of the same name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum InstallStrategy {
    Fresh,
    ReplaceKeepConfig,
    ReplaceFresh,
}

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
    // ── Plugin variants ───────────────────────────────────────────────────
    /// List all plugins with state, pid, rss, last activity, crashes, dropped chunks.
    ListPlugins,
    /// User invoked a plugin-registered palette command.
    InvokeCommand {
        plugin: String,
        id: String,
        session_id: String,
    },
    /// User invoked a plugin-added block action.
    InvokeBlockAction {
        plugin: String,
        id: String,
        block_id: String,
        session_id: String,
    },
    // ── Plugin management variants (plugins-pane milestone) ───────────────
    /// Install a plugin from a validated bundle path or directory.
    InstallPlugin {
        bundle_path: std::path::PathBuf,
        strategy: InstallStrategy,
    },
    /// Uninstall a plugin by name; optionally preserve config.toml.
    UninstallPlugin {
        name: String,
        keep_config: bool,
    },
    /// Enable or disable a plugin. Disabling kills the subprocess.
    SetPluginEnabled {
        name: String,
        enabled: bool,
    },
    /// Read user-editable config.toml text.
    ReadPluginConfig { name: String },
    /// Write new config.toml text (validated as TOML before persisting).
    WritePluginConfig { name: String, toml: String },
    /// Tail the last `max_lines` of a plugin's log file.
    ReadPluginLog { name: String, max_lines: u32 },
    /// Begin streaming PluginLogChunk events for `name`.
    SubscribePluginLog { name: String },
    /// Stop streaming log chunks.
    UnsubscribePluginLog { name: String },
    /// Synchronous UI→plugin RPC. Plugin returns JSON.
    CallPluginUI {
        name: String,
        method: String,
        args_json: String,
    },
    /// Preview a plugin bundle: extract + validate without installing.
    /// Returns manifest metadata, permissions, and icon bytes if present.
    PreviewPlugin {
        bundle_path: std::path::PathBuf,
    },
}

/// Response from hostd to a terminal app over IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum IpcResponse {
    Ok,
    SessionList { sessions: Vec<IpcSessionInfo> },
    Error { message: String },
    Plugins(Vec<PluginInfo>),
    /// Response body for ReadPluginConfig / WritePluginConfig-success.
    PluginConfig { toml: String },
    /// Response body for ReadPluginLog.
    PluginLog { text: String },
    /// Response body for CallPluginUI.
    PluginUiResult { result_json: String },
    /// Response body for PreviewPlugin.
    PluginPreview {
        /// JSON-serialized `{name, version, author, description}` from manifest.
        manifest_json: String,
        /// Human-readable permission strings from the manifest.
        permissions: Vec<String>,
        /// Whether an `icon.png` exists in the bundle.
        has_icon: bool,
        /// Raw PNG bytes if present and ≤ 256 KB, else None.
        icon_png: Option<Vec<u8>>,
    },
}

/// Plugin info returned in ListPlugins responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginInfo {
    pub name: String,
    pub status: String, // "running" | "idle" | "quarantined" | "not_spawned" | "disabled" | "install_error"
    pub pid: Option<u32>,
    pub rss_bytes: Option<u64>,
    pub last_activity_secs_ago: Option<u64>,
    pub crashes: u32,
    pub dropped_chunks: u64,
    // New fields for the Plugins pane.
    pub version: String,
    pub author: String,
    pub description: String,
    pub config_path: Option<std::path::PathBuf>,
    pub ui_entrypoint: Option<String>,
    pub has_icon: bool,
    pub permissions: Vec<String>,
    pub install_error: Option<String>,
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
    /// The user's subscription was revoked but the session still exists
    /// (e.g. host stopped sharing with this user, or a permission change).
    SessionRevoked {
        session_id: SessionId,
        reason: String,
    },
    /// A plugin emitted a UI-facing event (command registration, block action, etc.).
    PluginUiEvent(PluginUiEvent),
    /// One live chunk of a plugin's stderr log. Fires only while subscribed.
    PluginLogChunk { plugin: String, bytes: Vec<u8> },
}

/// Plugin-emitted UI event variants.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "data")]
pub enum PluginUiEvent {
    CommandRegistered { plugin: String, id: String, label: String },
    CommandUnregistered { plugin: String, id: String },
    BlockActionAdded { plugin: String, block_id: String, id: String, label: String },
    BlockActionRemoved { plugin: String, block_id: String, id: String },
    EmitSessionNote { session_id: String, text: String },
    QuarantineChanged { plugin: String, quarantined: bool },
    /// Forwarded plugin→UI event (e.g. progress updates, data pushes).
    UiEvent { plugin: String, method: String, data_json: String },
}

impl IpcEvent {
    /// Return the session ID this event is associated with.
    pub fn session_id(&self) -> Option<&SessionId> {
        match self {
            Self::RemoteInput { session_id, .. }
            | Self::LeaseChanged { session_id, .. }
            | Self::ControlRequested { session_id, .. }
            | Self::PresenceChanged { session_id, .. }
            | Self::SessionRevoked { session_id, .. } => Some(session_id),
            Self::PluginUiEvent(_) | Self::PluginLogChunk { .. } => None,
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

    #[test]
    fn plugin_ui_event_roundtrips() {
        let evt = IpcEvent::PluginUiEvent(PluginUiEvent::CommandRegistered {
            plugin: "beacon-hello".into(),
            id: "stats".into(),
            label: "Show Stats".into(),
        });
        let bytes = rmp_serde::to_vec_named(&evt).unwrap();
        let back: IpcEvent = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(back, evt);
    }

    #[test]
    fn list_plugins_request_roundtrips() {
        let req = IpcRequest::ListPlugins;
        let bytes = rmp_serde::to_vec_named(&req).unwrap();
        let _back: IpcRequest = rmp_serde::from_slice(&bytes).unwrap();
    }

    #[test]
    fn install_plugin_round_trip() {
        let req = IpcRequest::InstallPlugin {
            bundle_path: "/tmp/foo.beaconplug".into(),
            strategy: InstallStrategy::Fresh,
        };
        let bytes = rmp_serde::to_vec_named(&req).unwrap();
        let back: IpcRequest = rmp_serde::from_slice(&bytes).unwrap();
        assert!(matches!(back, IpcRequest::InstallPlugin { .. }));
    }

    #[test]
    fn plugin_log_chunk_event_round_trip() {
        let evt = IpcEvent::PluginLogChunk {
            plugin: "hello".into(),
            bytes: b"line\n".to_vec(),
        };
        let bytes = rmp_serde::to_vec_named(&evt).unwrap();
        let back: IpcEvent = rmp_serde::from_slice(&bytes).unwrap();
        assert!(matches!(back, IpcEvent::PluginLogChunk { .. }));
    }

    #[test]
    fn read_plugin_config_response_round_trip() {
        let resp = IpcResponse::PluginConfig { toml: "k = 1".into() };
        let bytes = rmp_serde::to_vec_named(&resp).unwrap();
        let back: IpcResponse = rmp_serde::from_slice(&bytes).unwrap();
        assert!(matches!(back, IpcResponse::PluginConfig { .. }));
    }

    #[test]
    fn preview_plugin_request_round_trip() {
        let req = IpcRequest::PreviewPlugin {
            bundle_path: "/tmp/foo.beaconplug".into(),
        };
        let bytes = rmp_serde::to_vec_named(&req).unwrap();
        let back: IpcRequest = rmp_serde::from_slice(&bytes).unwrap();
        assert!(matches!(back, IpcRequest::PreviewPlugin { .. }));
    }

    #[test]
    fn plugin_preview_response_round_trip() {
        let resp = IpcResponse::PluginPreview {
            manifest_json: r#"{"name":"test","version":"0.1.0","author":"me","description":"d"}"#
                .into(),
            permissions: vec!["read_session_blocks".into()],
            has_icon: true,
            icon_png: Some(vec![0x89, 0x50, 0x4e, 0x47]),
        };
        let bytes = rmp_serde::to_vec_named(&resp).unwrap();
        let back: IpcResponse = rmp_serde::from_slice(&bytes).unwrap();
        match back {
            IpcResponse::PluginPreview {
                manifest_json,
                permissions,
                has_icon,
                icon_png,
            } => {
                assert!(manifest_json.contains("test"));
                assert_eq!(permissions, vec!["read_session_blocks"]);
                assert!(has_icon);
                assert_eq!(icon_png, Some(vec![0x89, 0x50, 0x4e, 0x47]));
            }
            _ => panic!("expected PluginPreview"),
        }
    }
}
