import type { Block, BlockHeader, BlockFooter } from "./block";

// --- Client → Arc ---

export type ClientToArc =
  | { kind: "AttachSession"; data: { session_id: string; since_block: string | null } }
  | { kind: "DetachSession"; data: { session_id: string } }
  | { kind: "SessionInput"; data: { session_id: string; bytes: Uint8Array } }
  | { kind: "SessionResizeRequest"; data: { session_id: string; cols: number; rows: number } }
  | { kind: "Pong"; data: null };

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
  | { kind: "HelloAck"; data: { protocol_version: number } };
