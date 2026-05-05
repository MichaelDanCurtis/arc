// arc-web/src/types/arc-rpc.ts

/** Branded string type to keep arc IDs distinct from arbitrary strings. */
export type ArcSessionId = string & { readonly __brand: "ArcSessionId" };
export type ArcViewerId = string & { readonly __brand: "ArcViewerId" };

// ---------------------------------------------------------------------------
// Method params + results
// ---------------------------------------------------------------------------

export interface SessionAttachParams {
  session_id: string;
  viewer_id: string;
  viewer_name?: string;
}
export interface SessionAttachResult {
  ok: true;
  viewer_id: string;
  role: "controller" | "observer";
  initial_size?: { cols: number; rows: number };
}

export interface SessionDetachParams { session_id: string; }
export interface SessionSubscribeParams { session_id: string; }
export interface SessionUnsubscribeParams { session_id: string; }

export interface SessionInputParams {
  session_id: string;
  data: string; // base64
}

export interface SessionResizeParams {
  session_id: string;
  cols: number;
  rows: number;
}

export interface SessionControlParams {
  session_id: string;
  requester_id: string;
  requester_name?: string;
}

export interface SessionReleaseParams {
  session_id: string;
  controller_id: string;
}

export interface SessionApproveControlParams {
  session_id: string;
  request_id: string;
  requester_id: string;
}

export interface SessionDenyControlParams {
  session_id: string;
  request_id: string;
  requester_id: string;
}

export interface OkResult { ok: true; }

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

export interface MirrorFrameNotif {
  session_id: string;
  seq: number;
  data: string; // base64
  replay?: boolean;
}

export interface MirrorDiscontinuityNotif {
  session_id: string;
  reason?: string;
}

export interface MirrorControlGrantNotif {
  session_id: string;
  controller_id: string;
  controller_name?: string;
}

export interface MirrorControlRevokeNotif {
  session_id: string;
}

export interface MirrorHostInputNotif {
  session_id: string;
  data: string; // base64
  from_user: string;
}

export interface MirrorBlockStartNotif {
  session_id: string;
  header: unknown; // BlockHeader JSON; consumed by WASM detector + UI
}

export interface MirrorBlockChunkNotif {
  session_id: string;
  block_id: string;
  seq: number;
  data: string; // base64
}

export interface MirrorBlockEndNotif {
  session_id: string;
  footer: unknown; // BlockFooter JSON
}

export interface MirrorReplayBeginNotif { session_id: string; }
export interface MirrorReplayEndNotif { session_id: string; }
export interface MirrorReplayBlockNotif {
  session_id: string;
  block: unknown; // Block JSON
}

export interface SessionParticipant {
  user_id: string;
  user_name?: string;
  role: "controller" | "observer";
}
export interface SessionParticipantNotif {
  session_id: string;
  participants: SessionParticipant[];
}

export interface SessionGoneNotif {
  session_id: string;
  reason?: string;
}

export interface SessionRevokedNotif {
  session_id: string;
  reason?: string;
}

export interface SessionControlRequestedNotif {
  session_id: string;
  request_id: string;
  requester_id: string;
  requester_name?: string;
}

// ---------------------------------------------------------------------------
// Method name constants (for compile-time spell-checking at call sites)
// ---------------------------------------------------------------------------

export const ArcMethod = {
  SessionAttach: "arc.session.attach",
  SessionDetach: "arc.session.detach",
  SessionSubscribe: "arc.session.subscribe",
  SessionUnsubscribe: "arc.session.unsubscribe",
  SessionInput: "arc.session.input",
  SessionResize: "arc.session.resize",
  SessionControl: "arc.session.control",
  SessionRelease: "arc.session.release",
  SessionApproveControl: "arc.session.approve_control",
  SessionDenyControl: "arc.session.deny_control",
} as const;

export const ArcNotif = {
  MirrorFrame: "arc.mirror.frame",
  MirrorDiscontinuity: "arc.mirror.discontinuity",
  MirrorControlGrant: "arc.mirror.control_grant",
  MirrorControlRevoke: "arc.mirror.control_revoke",
  MirrorHostInput: "arc.mirror.host_input",
  MirrorBlockStart: "arc.mirror.block_start",
  MirrorBlockChunk: "arc.mirror.block_chunk",
  MirrorBlockEnd: "arc.mirror.block_end",
  MirrorReplayBegin: "arc.mirror.replay_begin",
  MirrorReplayBlock: "arc.mirror.replay_block",
  MirrorReplayEnd: "arc.mirror.replay_end",
  SessionParticipant: "arc.session.participant",
  SessionGone: "arc.session.gone",
  SessionRevoked: "arc.session.revoked",
  SessionControlRequested: "arc.session.control_requested",
} as const;
