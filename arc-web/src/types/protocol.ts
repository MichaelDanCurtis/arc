import type { Block, BlockHeader, BlockFooter } from "./block";

// --- Support types for session mirroring ---

export interface DriverInfo {
  user_id: string;
  device_label: string;
  /** Unix timestamp (seconds) when the lease was granted. Rust i64 → TS number. */
  since: number;
}

export interface WatcherInfo {
  user_id: string;
  device_label: string;
  can_drive: boolean;
}

// --- Client → Arc ---

export type ClientToArc =
  | { kind: "AttachSession"; data: { session_id: string; since_block: string | null } }
  | { kind: "DetachSession"; data: { session_id: string } }
  | { kind: "SessionInput"; data: { session_id: string; bytes: Uint8Array } }
  | { kind: "SessionResizeRequest"; data: { session_id: string; cols: number; rows: number } }
  | { kind: "Pong"; data: null }
  // Session mirroring: subscribe/control
  | { kind: "SubscribeSession"; data: { session_id: string; since_seq: number | null } }
  | { kind: "UnsubscribeSession"; data: { session_id: string } }
  | { kind: "RequestControl"; data: { session_id: string } }
  | { kind: "ApproveControl"; data: { session_id: string; request_id: string } }
  | { kind: "DenyControl"; data: { session_id: string; request_id: string } }
  | { kind: "RevokeControl"; data: { session_id: string } }
  // Host-side mirror stream (included for completeness)
  | { kind: "PushOutput"; data: { session_id: string; bytes: Uint8Array; seq: number } }
  | { kind: "OutputDiscontinuity"; data: { session_id: string; last_seq: number } };

// --- Arc → Client ---

export type ArcToClient =
  | { kind: "AttachAck"; data: { session_id: string; current_cols: number; current_rows: number; resume_from: string | null } }
  | { kind: "AttachDenied"; data: { session_id: string; reason: string } }
  | { kind: "SessionGone"; data: { session_id: string; reason: string } }
  | { kind: "BlockStart"; data: { session_id: string; header: BlockHeader } }
  | { kind: "BlockOutputChunk"; data: { session_id: string; block_id: string; seq: number; bytes: Uint8Array } }
  | { kind: "BlockEnd"; data: { session_id: string; footer: BlockFooter } }
  | { kind: "ReplayBegin"; data: { session_id: string; total_blocks: number } }
  | { kind: "ReplayBlock"; data: { session_id: string; block: Block } }
  | { kind: "ReplayEnd"; data: { session_id: string } }
  | { kind: "Ping"; data: null }
  | { kind: "Pong"; data: null }
  | { kind: "Error"; data: { code: string; message: string } }
  | { kind: "HelloAck"; data: { protocol_version: number } }
  // Session mirroring: output stream + control events
  | { kind: "SessionOutput"; data: { session_id: string; bytes: Uint8Array; seq: number; replay: boolean } }
  | { kind: "SessionDiscontinuity"; data: { session_id: string; last_seq: number } }
  | { kind: "LeaseChanged"; data: { session_id: string; driver: DriverInfo | null } }
  | { kind: "ControlRequested"; data: { session_id: string; request_id: string; from_user: string; from_device: string } }
  | { kind: "PresenceChanged"; data: { session_id: string; watchers: WatcherInfo[] } }
  | { kind: "HostInput"; data: { session_id: string; bytes: Uint8Array; from_user: string } }
  | { kind: "SessionRevoked"; data: { session_id: string; reason: string } };
