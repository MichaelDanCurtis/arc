"use client";

/**
 * useMirrorBlocks — layer a WasmBlockDetector on top of the mirror stream.
 *
 * Composes with the existing `useWebSocket` / `SessionOutput` path by
 * accepting a raw-bytes feed function pattern: the hook returns a `feedBytes`
 * callback that callers invoke for every `SessionOutput` chunk received from
 * the relay. This keeps the hook independent of the WebSocket layer and
 * trivially testable.
 *
 * D2 decision (command-text extraction): implemented HERE in C4.3.
 * Bytes received in the OutputByte stream between PromptStart and CommandStart
 * are accumulated per-block and decoded as UTF-8, then the trailing prompt
 * sigil (`$`, `#`, `%`, `>`) plus surrounding whitespace is stripped.
 * This is a heuristic — ANSI-colored prompts whose sigil is inside an escape
 * sequence will produce an unparsed string (C4.4 will render em-dash fallback
 * for empty command strings). The choice to do this in C4.3 rather than C4.4
 * keeps `WatchBlock.command` always populated by the time the component sees
 * it, which simplifies render logic downstream.
 *
 * OutputByte accumulation: bytes are NOT decoded per-byte (breaks multi-byte
 * UTF-8). We maintain a growable `Uint8Array` per block and decode lazily.
 * For the command buffer (PromptStart→CommandStart window) we also accumulate
 * bytes and decode once at CommandStart.
 */

import { useEffect, useRef, useReducer, useCallback } from "react";
import type { WatchBlock, WatchBlockMode } from "@/types/watch-block";

// ---------------------------------------------------------------------------
// BlockEventJs shape (matches serde tagged enum from beacon-blocks-wasm)
// Note: Rust uses #[serde(tag = "kind")] and DetectorMode uses snake_case.
// ---------------------------------------------------------------------------

type DetectorMode = "osc133" | "heuristic";

type BlockEventJs =
  | { kind: "PromptStart"; mode: DetectorMode }
  | { kind: "CommandStart"; mode: DetectorMode; command: string | null }
  | { kind: "OutputStart"; mode: DetectorMode }
  | { kind: "CommandEnd"; mode: DetectorMode; exit_code: number | null }
  | { kind: "OutputByte"; b: number };

// ---------------------------------------------------------------------------
// Reducer state
// ---------------------------------------------------------------------------

interface ReducerState {
  /** Completed + in-progress blocks, append-only. */
  blocks: WatchBlock[];
  /** Index into `blocks` of the currently-open block, or null. */
  currentIndex: number | null;
  /** Next block id counter. */
  nextId: number;
  /**
   * Accumulator for bytes between PromptStart and CommandStart.
   * Used to extract the command text (D2 heuristic).
   * null when not in PromptStart→CommandStart window.
   */
  promptBytesBuffer: number[] | null;
  /**
   * Scratch buffer for output bytes while a block is in the "output" phase.
   * Accumulated as a number[] (amortized O(1) push) and materialized into a
   * single Uint8Array on CommandEnd. Keeps WatchBlock.output always a settled
   * Uint8Array for readers while avoiding O(n²) copying during streaming.
   * null when not in output phase.
   */
  outputScratch: number[] | null;
  /**
   * Phase tracks which segment of a block we're currently accumulating.
   * - "prompt"  : PromptStart received, awaiting CommandStart
   * - "command" : CommandStart received, awaiting OutputStart
   * - "output"  : OutputStart received, accumulating output bytes
   * - null      : no open block
   */
  phase: "prompt" | "command" | "output" | null;
}

const INITIAL_STATE: ReducerState = {
  blocks: [],
  currentIndex: null,
  nextId: 0,
  promptBytesBuffer: null,
  outputScratch: null,
  phase: null,
};

// ---------------------------------------------------------------------------
// Pure helper: strip trailing prompt sigil + whitespace from a UTF-8 string.
// Handles "$", "#", "%", ">" at the end after trimming whitespace.
// ---------------------------------------------------------------------------

const SIGIL_RE = /[\s$#%>]+$/u;

export function stripPromptSigil(text: string): string {
  return text.replace(SIGIL_RE, "");
}

// ---------------------------------------------------------------------------
// Pure helper: decode a byte array as UTF-8 (best-effort, non-fatal).
// ---------------------------------------------------------------------------

const decoder = new TextDecoder("utf-8", { fatal: false });

function decodeBytes(bytes: number[]): string {
  return decoder.decode(new Uint8Array(bytes));
}

// ---------------------------------------------------------------------------
// Action types
// ---------------------------------------------------------------------------

type Action =
  | { type: "FEED_EVENTS"; events: BlockEventJs[] }
  | { type: "RESET" };

// ---------------------------------------------------------------------------
// Reducer — pure function, exported for unit testing.
// ---------------------------------------------------------------------------

export function mirrorBlocksReducer(
  state: ReducerState,
  action: Action,
): ReducerState {
  if (action.type === "RESET") return { ...INITIAL_STATE };
  if (action.type !== "FEED_EVENTS") return state;

  let {
    blocks,
    currentIndex,
    nextId,
    promptBytesBuffer,
    outputScratch,
    phase,
  } = state;

  // Work with a mutable copy of blocks to avoid O(n²) spread per byte.
  // We re-freeze at the end before returning.
  blocks = [...blocks];

  for (const event of action.events) {
    switch (event.kind) {
      case "PromptStart": {
        // Close any open block first (defensive — relay should send CommandEnd).
        if (currentIndex !== null && phase !== null) {
          const prev = blocks[currentIndex];
          // Materialize any pending output scratch into the closing block.
          const output =
            outputScratch !== null
              ? new Uint8Array(outputScratch)
              : prev.output;
          blocks[currentIndex] = {
            ...prev,
            output,
            status: "done",
            endedAt: Date.now(),
          };
          outputScratch = null;
        }
        // Open a new block.
        const id = `block-${nextId}`;
        const mode: WatchBlockMode = event.mode;
        const newBlock: WatchBlock = {
          id,
          prompt: "",
          command: "",
          output: new Uint8Array(0),
          mode,
          exitCode: null,
          status: "running",
          startedAt: Date.now(),
          endedAt: null,
        };
        blocks.push(newBlock);
        currentIndex = blocks.length - 1;
        nextId += 1;
        promptBytesBuffer = [];
        phase = "prompt";
        break;
      }

      case "CommandStart": {
        if (currentIndex === null) break;
        // D2: Extract command text from bytes accumulated since PromptStart.
        // Fix 3: also save the raw decoded text as the prompt field.
        let prompt = "";
        let command = "";
        if (promptBytesBuffer !== null && promptBytesBuffer.length > 0) {
          prompt = decodeBytes(promptBytesBuffer).trim();
          command = stripPromptSigil(prompt);
        }
        // If command field is provided directly by the event (future C4.4+), prefer it.
        if (event.command != null && event.command.length > 0) {
          command = event.command;
        }
        blocks[currentIndex] = { ...blocks[currentIndex], prompt, command };
        promptBytesBuffer = null;
        phase = "command";
        break;
      }

      case "OutputStart": {
        if (currentIndex === null) break;
        outputScratch = [];
        phase = "output";
        break;
      }

      case "CommandEnd": {
        if (currentIndex === null) break;
        // Materialize the output scratch buffer into a single Uint8Array (O(n) one-shot).
        const output =
          outputScratch !== null
            ? new Uint8Array(outputScratch)
            : blocks[currentIndex].output;
        blocks[currentIndex] = {
          ...blocks[currentIndex],
          output,
          exitCode: event.exit_code ?? null,
          status: "done",
          endedAt: Date.now(),
        };
        currentIndex = null;
        promptBytesBuffer = null;
        outputScratch = null;
        phase = null;
        break;
      }

      case "OutputByte": {
        if (currentIndex === null) break;
        const b = event.b;
        if (phase === "prompt" && promptBytesBuffer !== null) {
          // Accumulate into the prompt/command-extraction buffer (amortized O(1)).
          promptBytesBuffer.push(b);
        } else if (phase === "output" && outputScratch !== null) {
          // Accumulate into scratch buffer (amortized O(1) — no Uint8Array copy per byte).
          outputScratch.push(b);
        }
        // Bytes in "command" phase (between CommandStart and OutputStart) are
        // discarded — they are echoed input, not useful for the watch view.
        break;
      }

      default:
        break;
    }
  }

  return { blocks, currentIndex, nextId, promptBytesBuffer, outputScratch, phase };
}

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

export interface MirrorBlocksResult {
  blocks: WatchBlock[];
  /** ID of the currently-open (running) block, or null. */
  currentBlockId: string | null;
  /**
   * Feed a chunk of raw PTY bytes into the detector.
   * Call this from your SessionOutput handler on every incoming mirror frame.
   */
  feedBytes: (bytes: Uint8Array) => void;
}

/**
 * useMirrorBlocks — instantiates a WasmBlockDetector for `sessionId`, feeds
 * raw PTY bytes through it, and maintains an append-only list of WatchBlocks.
 *
 * The hook re-creates the detector whenever `sessionId` changes (handles
 * Next.js HMR reloads and navigation between sessions cleanly).
 *
 * Usage:
 * ```tsx
 * const { blocks, currentBlockId, feedBytes } = useMirrorBlocks(sessionId);
 * // In your SessionOutput handler:
 * feedBytes(new Uint8Array(msg.data.bytes));
 * ```
 */
export function useMirrorBlocks(sessionId: string | null): MirrorBlocksResult {
  const [state, dispatch] = useReducer(mirrorBlocksReducer, INITIAL_STATE);

  // Hold a ref to the WasmBlockDetector instance so we can call feed()
  // synchronously without going through React state.
  // Type is `any` because the WASM class type is only available at runtime
  // (the .d.ts types the constructor as returning WasmBlockDetector).
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const detectorRef = useRef<any>(null);

  // Track the sessionId for which the detector was created.
  const detectorSessionRef = useRef<string | null>(null);

  useEffect(() => {
    if (!sessionId) return;

    let detector: { feed(bytes: Uint8Array): BlockEventJs[]; free(): void } | null = null;

    // Reset reducer so stale blocks from the previous session are cleared
    // immediately — before the new detector resolves.
    dispatch({ type: "RESET" });

    // Lazily import the WASM module. The bundler-target wasm-bindgen output
    // auto-initialises on import; no explicit `init()` call is needed.
    import("@/wasm/beacon-blocks/beacon_blocks_wasm")
      .then(({ WasmBlockDetector }) => {
        // Race guard: if sessionId changed mid-import, skip instantiation for
        // the now-stale session.
        if (detectorSessionRef.current === sessionId) return;
        detector = new WasmBlockDetector() as typeof detector;
        detectorRef.current = detector;
        detectorSessionRef.current = sessionId;
      })
      .catch((err: unknown) => {
        console.error("[useMirrorBlocks] failed to load WasmBlockDetector:", err);
      });

    return () => {
      // Free the WASM memory and clear refs on unmount or sessionId change.
      if (detectorRef.current && detectorSessionRef.current === sessionId) {
        try {
          detectorRef.current.free();
        } catch {
          // Ignore double-free if the module was never fully initialised.
        }
        detectorRef.current = null;
        detectorSessionRef.current = null;
      }
    };
  }, [sessionId]);

  const feedBytes = useCallback((bytes: Uint8Array) => {
    const detector = detectorRef.current;
    // Bytes arriving before the WASM detector resolves are discarded.
    // Callers that can't tolerate data loss should buffer.
    if (!detector) return;

    let events: BlockEventJs[];
    try {
      // feed() returns the JS Array from serde-wasm-bindgen serialisation.
      events = detector.feed(bytes) as BlockEventJs[];
    } catch (err) {
      console.error("[useMirrorBlocks] detector.feed() threw:", err);
      return;
    }

    if (!Array.isArray(events) || events.length === 0) return;

    dispatch({ type: "FEED_EVENTS", events });
  }, []);

  const currentBlockId =
    state.currentIndex !== null
      ? (state.blocks[state.currentIndex]?.id ?? null)
      : null;

  return {
    blocks: state.blocks,
    currentBlockId,
    feedBytes,
  };
}
