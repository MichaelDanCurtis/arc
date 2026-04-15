import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { WatchBlockList } from "@/components/watch/WatchBlockList";
import type { WatchBlock } from "@/types/watch-block";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const encoder = new TextEncoder();

function makeBlock(overrides: Partial<WatchBlock> = {}): WatchBlock {
  return {
    id: "block-0",
    prompt: "user@host:~$",
    command: "ls -la",
    output: encoder.encode("file1.txt\nfile2.txt"),
    mode: "osc133",
    exitCode: 0,
    status: "done",
    startedAt: Date.now(),
    endedAt: Date.now(),
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Empty state
// ---------------------------------------------------------------------------

describe("WatchBlockList — empty state", () => {
  it("renders waiting message when blocks is empty", () => {
    render(<WatchBlockList blocks={[]} currentBlockId={null} />);
    expect(screen.getByText(/Waiting for first command/i)).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------
// D2 — em-dash fallback for empty command
// ---------------------------------------------------------------------------

describe("WatchBlockList — D2 em-dash fallback", () => {
  it("renders em-dash when command is empty string", () => {
    const block = makeBlock({ command: "" });
    render(<WatchBlockList blocks={[block]} currentBlockId={null} />);
    const commandEl = screen.getByTestId("command-text");
    expect(commandEl.textContent).toBe("—");
  });

  it("renders the actual command when non-empty", () => {
    const block = makeBlock({ command: "git status" });
    render(<WatchBlockList blocks={[block]} currentBlockId={null} />);
    const commandEl = screen.getByTestId("command-text");
    expect(commandEl.textContent).toBe("git status");
  });
});

// ---------------------------------------------------------------------------
// Mode badge
// ---------------------------------------------------------------------------

describe("WatchBlockList — mode badge", () => {
  it("renders 'osc133' badge text for osc133 mode", () => {
    const block = makeBlock({ mode: "osc133" });
    render(<WatchBlockList blocks={[block]} currentBlockId={null} />);
    expect(screen.getByText("osc133")).toBeInTheDocument();
  });

  it("renders 'heuristic' badge text for heuristic mode", () => {
    const block = makeBlock({ mode: "heuristic" });
    render(<WatchBlockList blocks={[block]} currentBlockId={null} />);
    expect(screen.getByText("heuristic")).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------
// D4 — exit code chip
// ---------------------------------------------------------------------------

describe("WatchBlockList — D4 exit code chip", () => {
  it("renders exit code 0 for osc133 block", () => {
    const block = makeBlock({ mode: "osc133", exitCode: 0 });
    render(<WatchBlockList blocks={[block]} currentBlockId={null} />);
    expect(screen.getByText("0")).toBeInTheDocument();
  });

  it("renders exit code 1 badge for osc133 block with error", () => {
    const block = makeBlock({ mode: "osc133", exitCode: 1 });
    render(<WatchBlockList blocks={[block]} currentBlockId={null} />);
    expect(screen.getByText("1")).toBeInTheDocument();
  });

  it("renders em-dash for heuristic block regardless of exitCode", () => {
    const block = makeBlock({ mode: "heuristic", exitCode: null });
    render(<WatchBlockList blocks={[block]} currentBlockId={null} />);
    // em-dash in the exit code chip area
    const emDashes = screen.getAllByText("—");
    // At least one em-dash: either from empty command or exit code chip.
    // Here command is 'ls -la' so the em-dash must be from the exit code chip.
    expect(emDashes.length).toBeGreaterThanOrEqual(1);
  });

  it("renders em-dash for osc133 block with null exitCode (still running)", () => {
    const block = makeBlock({ mode: "osc133", exitCode: null, status: "running" });
    render(<WatchBlockList blocks={[block]} currentBlockId={null} />);
    // null exitCode → em-dash even for osc133
    expect(screen.getByText("—")).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------
// Running state
// ---------------------------------------------------------------------------

describe("WatchBlockList — running indicator", () => {
  it("renders running indicator when block is the currentBlockId", () => {
    const block = makeBlock({ id: "block-0", status: "running" });
    render(<WatchBlockList blocks={[block]} currentBlockId="block-0" />);
    const indicator = screen.getByLabelText("running");
    expect(indicator).toBeInTheDocument();
  });

  it("does NOT render running indicator for a completed block", () => {
    const block = makeBlock({ id: "block-0", status: "done", exitCode: 0 });
    render(<WatchBlockList blocks={[block]} currentBlockId={null} />);
    expect(screen.queryByLabelText("running")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Output rendering
// ---------------------------------------------------------------------------

describe("WatchBlockList — output area", () => {
  it("renders decoded output text in a pre element", () => {
    const block = makeBlock({ output: encoder.encode("hello world") });
    render(<WatchBlockList blocks={[block]} currentBlockId={null} />);
    const pre = screen.getByTestId("output-pre");
    expect(pre.textContent).toBe("hello world");
  });

  it("does not render output area when output is empty", () => {
    const block = makeBlock({ output: new Uint8Array(0) });
    render(<WatchBlockList blocks={[block]} currentBlockId={null} />);
    expect(screen.queryByTestId("output-pre")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Multi-block list
// ---------------------------------------------------------------------------

describe("WatchBlockList — multi-block", () => {
  it("renders all blocks in order", () => {
    const blocks = [
      makeBlock({ id: "block-0", command: "echo first" }),
      makeBlock({ id: "block-1", command: "echo second" }),
    ];
    render(<WatchBlockList blocks={blocks} currentBlockId={null} />);
    const items = screen.getAllByTestId("watch-block");
    expect(items).toHaveLength(2);
  });

  it("marks only the currentBlockId with running indicator", () => {
    const blocks = [
      makeBlock({ id: "block-0", command: "done cmd", status: "done" }),
      makeBlock({ id: "block-1", command: "running cmd", status: "running" }),
    ];
    render(<WatchBlockList blocks={blocks} currentBlockId="block-1" />);
    const indicators = screen.getAllByLabelText("running");
    expect(indicators).toHaveLength(1);
  });
});
