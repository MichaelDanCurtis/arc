/**
 * WatchBlock — reduced block shape for the /watch mirror stream.
 *
 * This type is intentionally separate from `Block` (attach-side) to keep the
 * attach-side `Block` type strict and free of partial-header footguns (D3).
 *
 * `output` accumulates raw PTY bytes from `OutputByte` events. Decode lazily
 * at render time using `TextDecoder` with `stream: true` to handle multi-byte
 * UTF-8 codepoints that may straddle chunk boundaries.
 *
 * `command` is populated by the D2 heuristic: bytes received between
 * `PromptStart` and `CommandStart` are accumulated and decoded, then the
 * trailing prompt sigil (`$`, `#`, `%`, `>`) and surrounding whitespace are
 * stripped. Empty string if no bytes arrived in that window (C4.4 will
 * render an em-dash fallback for empty commands).
 */

export type WatchBlockMode = "osc133" | "heuristic";
export type WatchBlockStatus = "running" | "done";

export interface WatchBlock {
  /** Locally-generated monotonic id (e.g. "block-0", "block-1"). */
  id: string;
  /**
   * Raw bytes received between PromptStart and CommandStart.
   * Used by C4.4 to render the prompt text (e.g. "user@host:~$").
   */
  prompt: string;
  /**
   * Heuristically-extracted command text (D2).
   * Bytes accumulated between PromptStart and CommandStart, decoded as UTF-8
   * and stripped of trailing prompt sigil + whitespace.
   * Empty string when the window contained no bytes.
   */
  command: string;
  /**
   * Raw output bytes accumulated from OutputByte events after OutputStart.
   * Decode lazily at render time via TextDecoder with stream:true.
   */
  output: Uint8Array;
  /** Which detector produced this block's boundary events. */
  mode: WatchBlockMode;
  /** Exit code from CommandEnd, or null for heuristic blocks and running blocks. */
  exitCode: number | null;
  status: WatchBlockStatus;
  /** Unix timestamp (ms) when PromptStart was received. */
  startedAt: number;
  /** Unix timestamp (ms) when CommandEnd was received, or null if still running. */
  endedAt: number | null;
}
