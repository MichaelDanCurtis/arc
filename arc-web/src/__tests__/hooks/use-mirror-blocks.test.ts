/**
 * Unit tests for the mirrorBlocksReducer pure function and stripPromptSigil
 * helper exported from use-mirror-blocks.
 *
 * These tests exercise the reducer directly (no WASM, no React rendering)
 * by constructing synthetic BlockEventJs arrays that mirror what
 * WasmBlockDetector.feed() would produce for a real PTY byte stream.
 *
 * OSC 133 transcript anatomy for reference:
 *   \x1b]133;A  → PromptStart
 *   \x1b]133;B  → CommandStart
 *   \x1b]133;C  → OutputStart
 *   \x1b]133;D;0 → CommandEnd exit_code=0
 *
 * C4.3 D2: command text is extracted heuristically from OutputByte events
 * between PromptStart and CommandStart. The trailing sigil ($, #, %, >) and
 * surrounding whitespace is stripped.
 */

import { describe, it, expect } from "vitest";
import {
  mirrorBlocksReducer,
  stripPromptSigil,
} from "@/hooks/use-mirror-blocks";
import type { WatchBlock } from "@/types/watch-block";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const INITIAL_STATE = {
  blocks: [] as WatchBlock[],
  currentIndex: null as number | null,
  nextId: 0,
  promptBytesBuffer: null as number[] | null,
  outputScratch: null as number[] | null,
  phase: null as "prompt" | "command" | "output" | null,
};

type BlockEventJs =
  | { kind: "PromptStart"; mode: "osc133" | "heuristic" }
  | { kind: "CommandStart"; mode: "osc133" | "heuristic"; command: string | null }
  | { kind: "OutputStart"; mode: "osc133" | "heuristic" }
  | { kind: "CommandEnd"; mode: "osc133" | "heuristic"; exit_code: number | null }
  | { kind: "OutputByte"; b: number };

function feed(state: typeof INITIAL_STATE, events: BlockEventJs[]) {
  return mirrorBlocksReducer(state, { type: "FEED_EVENTS", events });
}

/** Convert an ASCII string to a sequence of OutputByte events. */
function outputBytesFromStr(s: string): BlockEventJs[] {
  return Array.from(s).map((ch) => ({ kind: "OutputByte" as const, b: ch.charCodeAt(0) }));
}

// ---------------------------------------------------------------------------
// stripPromptSigil tests
// ---------------------------------------------------------------------------

describe("stripPromptSigil", () => {
  it("strips trailing $", () => {
    expect(stripPromptSigil("user@host:~$")).toBe("user@host:~");
  });
  it("strips trailing $ with surrounding whitespace", () => {
    expect(stripPromptSigil("  user@host:~$ ")).toBe("  user@host:~");
  });
  it("strips trailing #", () => {
    expect(stripPromptSigil("root#")).toBe("root");
  });
  it("strips trailing %", () => {
    expect(stripPromptSigil("zsh%")).toBe("zsh");
  });
  it("strips trailing >", () => {
    expect(stripPromptSigil("PS>")).toBe("PS");
  });
  it("returns empty string for pure sigil", () => {
    expect(stripPromptSigil("$ ")).toBe("");
  });
  it("does not strip sigils in the middle", () => {
    expect(stripPromptSigil("a$b")).toBe("a$b");
  });
  it("handles empty string", () => {
    expect(stripPromptSigil("")).toBe("");
  });
});

// ---------------------------------------------------------------------------
// Reducer: basic OSC 133 lifecycle
// ---------------------------------------------------------------------------

describe("mirrorBlocksReducer — OSC 133 full lifecycle", () => {
  it("produces a complete WatchBlock for PromptStart→CommandStart→OutputStart→CommandEnd", () => {
    // Synthesise the event sequence for: prompt "$ ", command "ls", output "foo\n"
    const promptBytes = outputBytesFromStr("$ ");   // PromptStart window
    const outputBytes = outputBytesFromStr("foo\n"); // OutputStart window

    const events: BlockEventJs[] = [
      { kind: "PromptStart", mode: "osc133" },
      ...promptBytes,
      { kind: "CommandStart", mode: "osc133", command: null },
      { kind: "OutputStart", mode: "osc133" },
      ...outputBytes,
      { kind: "CommandEnd", mode: "osc133", exit_code: 0 },
    ];

    const state = feed(INITIAL_STATE, events);

    expect(state.blocks).toHaveLength(1);
    const block = state.blocks[0];

    expect(block.id).toBe("block-0");
    expect(block.mode).toBe("osc133");
    expect(block.status).toBe("done");
    expect(block.exitCode).toBe(0);
    expect(block.endedAt).not.toBeNull();

    // D2: "$ " → strip sigil → ""
    expect(block.command).toBe("");

    // Output bytes accumulated correctly.
    expect(block.output).toBeInstanceOf(Uint8Array);
    expect(new TextDecoder().decode(block.output)).toBe("foo\n");
  });

  it("extracts non-empty command from prompt bytes when sigil has preceding text", () => {
    // Prompt bytes "user@host:~$ " → strip sigil → "user@host:~"
    const promptBytes = outputBytesFromStr("user@host:~$ ");
    const events: BlockEventJs[] = [
      { kind: "PromptStart", mode: "osc133" },
      ...promptBytes,
      { kind: "CommandStart", mode: "osc133", command: null },
      { kind: "OutputStart", mode: "osc133" },
      { kind: "CommandEnd", mode: "osc133", exit_code: 0 },
    ];

    const state = feed(INITIAL_STATE, events);
    expect(state.blocks[0].command).toBe("user@host:~");
  });

  it("prefers event.command when provided and non-empty", () => {
    const events: BlockEventJs[] = [
      { kind: "PromptStart", mode: "osc133" },
      ...outputBytesFromStr("$ "),
      { kind: "CommandStart", mode: "osc133", command: "git status" },
      { kind: "OutputStart", mode: "osc133" },
      { kind: "CommandEnd", mode: "osc133", exit_code: 0 },
    ];
    const state = feed(INITIAL_STATE, events);
    expect(state.blocks[0].command).toBe("git status");
  });

  it("sets exitCode correctly for non-zero exit", () => {
    const events: BlockEventJs[] = [
      { kind: "PromptStart", mode: "osc133" },
      { kind: "CommandStart", mode: "osc133", command: null },
      { kind: "OutputStart", mode: "osc133" },
      { kind: "CommandEnd", mode: "osc133", exit_code: 127 },
    ];
    const state = feed(INITIAL_STATE, events);
    expect(state.blocks[0].exitCode).toBe(127);
  });
});

// ---------------------------------------------------------------------------
// Reducer: heuristic mode
// ---------------------------------------------------------------------------

describe("mirrorBlocksReducer — heuristic mode", () => {
  it("creates a heuristic block with mode=heuristic and null exitCode", () => {
    const events: BlockEventJs[] = [
      { kind: "PromptStart", mode: "heuristic" },
      { kind: "CommandStart", mode: "heuristic", command: null },
      { kind: "OutputStart", mode: "heuristic" },
      ...outputBytesFromStr("hello\n"),
      { kind: "CommandEnd", mode: "heuristic", exit_code: null },
    ];
    const state = feed(INITIAL_STATE, events);
    expect(state.blocks).toHaveLength(1);
    const block = state.blocks[0];
    expect(block.mode).toBe("heuristic");
    expect(block.exitCode).toBeNull();
    expect(block.status).toBe("done");
    expect(new TextDecoder().decode(block.output)).toBe("hello\n");
  });
});

// ---------------------------------------------------------------------------
// Reducer: multiple blocks
// ---------------------------------------------------------------------------

describe("mirrorBlocksReducer — multiple blocks", () => {
  it("accumulates two sequential blocks", () => {
    const block1Events: BlockEventJs[] = [
      { kind: "PromptStart", mode: "osc133" },
      { kind: "CommandStart", mode: "osc133", command: null },
      { kind: "OutputStart", mode: "osc133" },
      ...outputBytesFromStr("a"),
      { kind: "CommandEnd", mode: "osc133", exit_code: 0 },
    ];
    const block2Events: BlockEventJs[] = [
      { kind: "PromptStart", mode: "osc133" },
      { kind: "CommandStart", mode: "osc133", command: null },
      { kind: "OutputStart", mode: "osc133" },
      ...outputBytesFromStr("b"),
      { kind: "CommandEnd", mode: "osc133", exit_code: 1 },
    ];

    const s1 = feed(INITIAL_STATE, block1Events);
    const s2 = feed(s1, block2Events);

    expect(s2.blocks).toHaveLength(2);
    expect(s2.blocks[0].id).toBe("block-0");
    expect(s2.blocks[1].id).toBe("block-1");
    expect(new TextDecoder().decode(s2.blocks[0].output)).toBe("a");
    expect(new TextDecoder().decode(s2.blocks[1].output)).toBe("b");
    expect(s2.blocks[0].exitCode).toBe(0);
    expect(s2.blocks[1].exitCode).toBe(1);
    expect(s2.currentIndex).toBeNull();
  });

  it("currentIndex tracks the open block id", () => {
    // After PromptStart, the block is open.
    const s1 = feed(INITIAL_STATE, [{ kind: "PromptStart", mode: "osc133" }]);
    expect(s1.currentIndex).toBe(0);
    expect(s1.blocks[0].status).toBe("running");

    // After CommandEnd, currentIndex becomes null.
    const s2 = feed(s1, [
      { kind: "CommandStart", mode: "osc133", command: null },
      { kind: "OutputStart", mode: "osc133" },
      { kind: "CommandEnd", mode: "osc133", exit_code: 0 },
    ]);
    expect(s2.currentIndex).toBeNull();
    expect(s2.blocks[0].status).toBe("done");
  });
});

// ---------------------------------------------------------------------------
// Reducer: output byte accumulation across multiple feed calls
// ---------------------------------------------------------------------------

describe("mirrorBlocksReducer — incremental feeding", () => {
  it("accumulates output bytes across multiple dispatch calls", () => {
    // Simulate streaming: events arrive in separate dispatch calls.
    const s1 = feed(INITIAL_STATE, [
      { kind: "PromptStart", mode: "osc133" },
      { kind: "CommandStart", mode: "osc133", command: null },
      { kind: "OutputStart", mode: "osc133" },
    ]);
    const chunk1: BlockEventJs[] = outputBytesFromStr("he");
    const chunk2: BlockEventJs[] = outputBytesFromStr("llo");
    const s2 = feed(s1, chunk1);
    const s3 = feed(s2, chunk2);
    const s4 = feed(s3, [{ kind: "CommandEnd", mode: "osc133", exit_code: 0 }]);

    expect(new TextDecoder().decode(s4.blocks[0].output)).toBe("hello");
  });
});

// ---------------------------------------------------------------------------
// Reducer: defensive close on second PromptStart without CommandEnd
// ---------------------------------------------------------------------------

describe("mirrorBlocksReducer — defensive close", () => {
  it("closes a running block when a new PromptStart arrives without CommandEnd", () => {
    const events: BlockEventJs[] = [
      { kind: "PromptStart", mode: "osc133" },
      { kind: "CommandStart", mode: "osc133", command: null },
      { kind: "OutputStart", mode: "osc133" },
      // No CommandEnd — new PromptStart arrives (relay reconnect scenario).
      { kind: "PromptStart", mode: "osc133" },
      { kind: "CommandStart", mode: "osc133", command: null },
      { kind: "OutputStart", mode: "osc133" },
      { kind: "CommandEnd", mode: "osc133", exit_code: 0 },
    ];
    const state = feed(INITIAL_STATE, events);

    expect(state.blocks).toHaveLength(2);
    // First block should have been defensively closed.
    expect(state.blocks[0].status).toBe("done");
    // Second block should be the one that closed cleanly.
    expect(state.blocks[1].status).toBe("done");
    expect(state.blocks[1].exitCode).toBe(0);
    expect(state.currentIndex).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Reducer: RESET action clears all state
// ---------------------------------------------------------------------------

describe("mirrorBlocksReducer — RESET", () => {
  it("clears blocks and resets all fields to initial state", () => {
    // First build up some state with a completed block.
    const s1 = feed(INITIAL_STATE, [
      { kind: "PromptStart", mode: "osc133" },
      { kind: "CommandStart", mode: "osc133", command: null },
      { kind: "OutputStart", mode: "osc133" },
      ...outputBytesFromStr("hello"),
      { kind: "CommandEnd", mode: "osc133", exit_code: 0 },
    ]);
    expect(s1.blocks).toHaveLength(1);
    expect(s1.nextId).toBe(1);

    // Dispatch RESET.
    const s2 = mirrorBlocksReducer(s1, { type: "RESET" });

    expect(s2.blocks).toHaveLength(0);
    expect(s2.currentIndex).toBeNull();
    expect(s2.nextId).toBe(0);
    expect(s2.promptBytesBuffer).toBeNull();
    expect(s2.outputScratch).toBeNull();
    expect(s2.phase).toBeNull();
  });

  it("RESET on initial state is a no-op (returns empty state)", () => {
    const s = mirrorBlocksReducer(INITIAL_STATE, { type: "RESET" });
    expect(s.blocks).toHaveLength(0);
    expect(s.currentIndex).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Reducer: prompt field population (Fix 3)
// ---------------------------------------------------------------------------

describe("mirrorBlocksReducer — prompt field", () => {
  it("populates prompt with trimmed text from PromptStart→CommandStart window", () => {
    const events: BlockEventJs[] = [
      { kind: "PromptStart", mode: "osc133" },
      ...outputBytesFromStr("user@host:~$ "),
      { kind: "CommandStart", mode: "osc133", command: null },
      { kind: "OutputStart", mode: "osc133" },
      { kind: "CommandEnd", mode: "osc133", exit_code: 0 },
    ];
    const state = feed(INITIAL_STATE, events);
    // prompt is the trimmed raw text (before sigil stripping)
    expect(state.blocks[0].prompt).toBe("user@host:~$");
    // command has the sigil stripped
    expect(state.blocks[0].command).toBe("user@host:~");
  });

  it("prompt is empty string when no bytes arrive before CommandStart", () => {
    const events: BlockEventJs[] = [
      { kind: "PromptStart", mode: "osc133" },
      { kind: "CommandStart", mode: "osc133", command: null },
      { kind: "OutputStart", mode: "osc133" },
      { kind: "CommandEnd", mode: "osc133", exit_code: 0 },
    ];
    const state = feed(INITIAL_STATE, events);
    expect(state.blocks[0].prompt).toBe("");
  });
});
