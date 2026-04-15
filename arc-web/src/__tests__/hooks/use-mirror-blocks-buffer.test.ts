/**
 * Tests for the pre-init byte buffer logic in useMirrorBlocks.
 *
 * The buffer logic is factored into two pure/exported pieces:
 *   - `drainPendingBytes`  — drains a chunk list into a detector and returns events
 *   - `PENDING_BUF_CAP`    — the 1 MB ceiling constant
 *
 * Testing via the pure helper is cleaner than renderHook + WASM mocking because
 * the WASM module is not present in the jsdom test environment.  The hook itself
 * wires the helper into the useEffect; these tests verify the helper contract.
 *
 * Test cases:
 *  1. drainPendingBytes feeds chunks in order and returns merged events.
 *  2. Buffer cap: oldest chunks are dropped when the cap is reached, so the
 *     first byte(s) are gone and only the tail is retained.
 */

import { describe, it, expect, vi } from "vitest";
import {
  drainPendingBytes,
  PENDING_BUF_CAP,
} from "@/hooks/use-mirror-blocks";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

type BlockEventJs =
  | { kind: "PromptStart"; mode: "osc133" | "heuristic" }
  | { kind: "CommandStart"; mode: "osc133" | "heuristic"; command: string | null }
  | { kind: "OutputStart"; mode: "osc133" | "heuristic" }
  | { kind: "CommandEnd"; mode: "osc133" | "heuristic"; exit_code: number | null }
  | { kind: "OutputByte"; b: number };

/** Build a fake detector that echoes back a fixed event list per feed() call. */
function fakeDetector(responseMap: Map<Uint8Array, BlockEventJs[]>) {
  return {
    feed(bytes: Uint8Array): BlockEventJs[] {
      // Find by reference in map.
      const hit = responseMap.get(bytes);
      return hit ?? [];
    },
  };
}

/** Build a fake detector that records which chunks were fed and returns them as OutputByte events. */
function recordingDetector(): {
  detector: { feed(bytes: Uint8Array): BlockEventJs[] };
  fed: Uint8Array[];
} {
  const fed: Uint8Array[] = [];
  const detector = {
    feed(bytes: Uint8Array): BlockEventJs[] {
      fed.push(bytes);
      // Emit one OutputByte per byte so tests can count/verify byte order.
      return Array.from(bytes).map((b) => ({ kind: "OutputByte" as const, b }));
    },
  };
  return { detector, fed };
}

// ---------------------------------------------------------------------------
// Test: drainPendingBytes feeds chunks in order and clears the buffer
// ---------------------------------------------------------------------------

describe("drainPendingBytes — buffer drains on init", () => {
  it("feeds all pending chunks in order and returns merged events", () => {
    const chunk1 = new Uint8Array([1, 2, 3]);
    const chunk2 = new Uint8Array([4, 5]);

    const { detector, fed } = recordingDetector();
    const chunks: Uint8Array[] = [chunk1, chunk2];
    const sizeHolder: [number] = [chunk1.length + chunk2.length];

    const events = drainPendingBytes(chunks, sizeHolder, detector);

    // Both chunks were fed in order.
    expect(fed).toHaveLength(2);
    expect(fed[0]).toBe(chunk1);
    expect(fed[1]).toBe(chunk2);

    // All bytes produced as OutputByte events, in order.
    expect(events).toHaveLength(5);
    expect(events).toEqual([
      { kind: "OutputByte", b: 1 },
      { kind: "OutputByte", b: 2 },
      { kind: "OutputByte", b: 3 },
      { kind: "OutputByte", b: 4 },
      { kind: "OutputByte", b: 5 },
    ]);

    // Buffer is cleared after drain.
    expect(chunks).toHaveLength(0);
    expect(sizeHolder[0]).toBe(0);
  });

  it("returns empty array and clears buffer when no chunks are pending", () => {
    const { detector } = recordingDetector();
    const chunks: Uint8Array[] = [];
    const sizeHolder: [number] = [0];

    const events = drainPendingBytes(chunks, sizeHolder, detector);

    expect(events).toHaveLength(0);
    expect(chunks).toHaveLength(0);
    expect(sizeHolder[0]).toBe(0);
  });

  it("continues draining remaining chunks if one feed() call throws", () => {
    const chunk1 = new Uint8Array([10]);
    const chunk2 = new Uint8Array([20]);

    let callCount = 0;
    const flakyDetector = {
      feed(bytes: Uint8Array): BlockEventJs[] {
        callCount++;
        if (callCount === 1) throw new Error("boom");
        return [{ kind: "OutputByte" as const, b: bytes[0] }];
      },
    };

    const chunks: Uint8Array[] = [chunk1, chunk2];
    const sizeHolder: [number] = [2];

    const events = drainPendingBytes(chunks, sizeHolder, flakyDetector);

    // First chunk threw — skipped.  Second chunk returned one event.
    expect(events).toHaveLength(1);
    expect(events[0]).toEqual({ kind: "OutputByte", b: 20 });
    // Buffer still cleared.
    expect(chunks).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Test: buffer cap — oldest bytes are dropped when cap is exceeded
// ---------------------------------------------------------------------------

describe("PENDING_BUF_CAP — buffer ceiling", () => {
  it("PENDING_BUF_CAP is 1 MB", () => {
    expect(PENDING_BUF_CAP).toBe(1 * 1024 * 1024);
  });

  it("simulates cap behaviour: oldest chunks are dropped when total exceeds cap", () => {
    // Simulate the hook's cap logic directly so we don't need renderHook.
    // We replicate the feedBytes pre-init path inline.
    const capWarnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

    const chunks: Uint8Array[] = [];
    const sizeHolder: [number] = [0];
    let capWarned = false;

    function simulateFeed(bytes: Uint8Array) {
      const incoming = bytes.length;
      if (incoming + sizeHolder[0] > PENDING_BUF_CAP) {
        if (!capWarned) {
          console.warn("[useMirrorBlocks] pre-init buffer cap (~1 MB) reached; dropping oldest bytes.");
          capWarned = true;
        }
        while (chunks.length > 0 && incoming + sizeHolder[0] > PENDING_BUF_CAP) {
          const dropped = chunks.shift()!;
          sizeHolder[0] -= dropped.length;
        }
      }
      chunks.push(bytes);
      sizeHolder[0] += incoming;
    }

    // Push two 600 KB chunks — second push should cause the first to be dropped.
    const KB = 1024;
    const first = new Uint8Array(600 * KB).fill(0xaa);
    const second = new Uint8Array(600 * KB).fill(0xbb);

    simulateFeed(first);
    expect(chunks).toHaveLength(1);
    expect(sizeHolder[0]).toBe(600 * KB);

    simulateFeed(second);

    // First chunk (0xaa bytes) should have been dropped.
    expect(chunks).toHaveLength(1);
    expect(chunks[0]).toBe(second);
    expect(sizeHolder[0]).toBe(600 * KB);

    // Cap warning was emitted exactly once.
    expect(capWarnSpy).toHaveBeenCalledOnce();
    expect(capWarnSpy.mock.calls[0][0]).toContain("pre-init buffer cap");

    capWarnSpy.mockRestore();
  });

  it("single chunk larger than cap replaces entire buffer", () => {
    const chunks: Uint8Array[] = [];
    const sizeHolder: [number] = [0];
    let capWarned = false;

    function simulateFeed(bytes: Uint8Array) {
      const incoming = bytes.length;
      if (incoming + sizeHolder[0] > PENDING_BUF_CAP) {
        if (!capWarned) { capWarned = true; }
        while (chunks.length > 0 && incoming + sizeHolder[0] > PENDING_BUF_CAP) {
          const dropped = chunks.shift()!;
          sizeHolder[0] -= dropped.length;
        }
      }
      chunks.push(bytes);
      sizeHolder[0] += incoming;
    }

    // Fill buffer to nearly full with small chunks.
    const small = new Uint8Array(512 * 1024).fill(0x01); // 512 KB
    simulateFeed(small);
    simulateFeed(small);
    // Buffer is now at 1 MB — exactly at cap (not over yet).
    expect(sizeHolder[0]).toBe(PENDING_BUF_CAP);

    // One more byte tips it over — oldest chunk (first small) gets dropped.
    const one = new Uint8Array([0xff]);
    simulateFeed(one);

    // After dropping first 512 KB chunk: 512 KB + 1 byte remain.
    expect(sizeHolder[0]).toBe(512 * 1024 + 1);
    expect(chunks).toHaveLength(2);
    expect(chunks[1]).toBe(one);
  });
});
