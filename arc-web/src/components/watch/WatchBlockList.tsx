"use client";

/**
 * WatchBlockList — renders the append-only list of WatchBlocks from the
 * mirror stream. Used by the /watch page (C4.5) to display commands and
 * output observed from a mirrored session in real time.
 *
 * Design decisions honoured:
 * - D2: empty `command` field renders an em-dash fallback.
 * - D4: exit code chip hidden (em-dash) for heuristic blocks; shown for osc133.
 * - Output rendered as plain UTF-8 via TextDecoder inside a <pre> — no ANSI
 *   parsing in this commit (deferred).
 * - No virtualisation — deferred.
 */

import { useMemo } from "react";
import type { WatchBlock } from "@/types/watch-block";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

// ---------------------------------------------------------------------------
// Output decoder — lazy, non-fatal UTF-8.
// A fresh TextDecoder is created per call so a future { stream: true } usage
// elsewhere cannot corrupt shared decoder state.
// ---------------------------------------------------------------------------

function decodeOutput(bytes: Uint8Array): string {
  return new TextDecoder("utf-8", { fatal: false }).decode(bytes);
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

/** Small pill showing which detector produced the block boundaries. */
function ModeBadge({ mode }: { mode: WatchBlock["mode"] }) {
  return (
    <Badge
      variant="outline"
      className={cn(
        "font-mono text-xs",
        mode === "osc133"
          ? "border-blue-500 text-blue-500"
          : "border-yellow-500 text-yellow-500",
      )}
    >
      {mode === "osc133" ? "osc133" : "heuristic"}
    </Badge>
  );
}

/** Exit-code chip. Only shown for osc133 blocks (D4). */
function ExitCodeChip({
  exitCode,
  mode,
}: {
  exitCode: number | null;
  mode: WatchBlock["mode"];
}) {
  // D4: heuristic blocks never have a reliable exit code — show em-dash.
  if (mode === "heuristic" || exitCode === null) {
    return (
      <span
        data-testid="exit-code-chip"
        className="font-mono text-xs text-muted-foreground"
      >
        —
      </span>
    );
  }

  const isError = exitCode !== 0;
  return (
    <Badge
      data-testid="exit-code-chip"
      variant="outline"
      className={cn(
        "font-mono text-xs",
        isError
          ? "border-destructive text-destructive"
          : "border-green-600 text-green-600",
      )}
    >
      {exitCode}
    </Badge>
  );
}

/** Subtle animated dot shown while a block is still running. */
function RunningIndicator() {
  return (
    <span
      aria-label="running"
      className="inline-block h-2 w-2 animate-pulse rounded-full bg-blue-400"
    />
  );
}

// ---------------------------------------------------------------------------
// WatchBlockItem — renders a single block.
// ---------------------------------------------------------------------------

interface WatchBlockItemProps {
  block: WatchBlock;
  isCurrentBlock: boolean;
}

function WatchBlockItem({ block, isCurrentBlock }: WatchBlockItemProps) {
  const outputText = useMemo(() => decodeOutput(block.output), [block.output]);

  // D2: render em-dash when command is empty.
  const commandDisplay = block.command.length > 0 ? block.command : "—";

  const time = new Date(block.startedAt).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });

  return (
    <div
      className={cn(
        "rounded border bg-card",
        isCurrentBlock && "ring-1 ring-blue-400/50",
      )}
      data-block-id={block.id}
      data-testid="watch-block"
    >
      {/* Header row: timestamp, running indicator, mode badge, exit code */}
      <div className="flex items-center gap-2 px-3 py-1.5 text-xs text-muted-foreground">
        <span className="font-mono">{time}</span>
        {isCurrentBlock && <RunningIndicator />}
        <ModeBadge mode={block.mode} />
        <ExitCodeChip exitCode={block.exitCode} mode={block.mode} />
      </div>

      {/* Prompt line — raw decoded prompt text (may be empty for heuristic blocks) */}
      {block.prompt.length > 0 && (
        <div className="border-t px-3 py-1 font-mono text-xs text-muted-foreground">
          {block.prompt}
        </div>
      )}

      {/* Command line */}
      <div className="border-t px-3 py-1.5 font-mono text-sm">
        <span className="text-muted-foreground">$ </span>
        <span data-testid="command-text">{commandDisplay}</span>
      </div>

      {/* Output area — plain UTF-8 pre, no ANSI parsing yet (deferred) */}
      {outputText.length > 0 && (
        <div className="border-t bg-neutral-900 px-3 py-2">
          <pre
            className="overflow-x-auto font-mono text-sm text-neutral-200 whitespace-pre-wrap"
            data-testid="output-pre"
          >
            {outputText}
          </pre>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// WatchBlockList — public component.
// ---------------------------------------------------------------------------

export interface WatchBlockListProps {
  blocks: WatchBlock[];
  currentBlockId: string | null;
}

export function WatchBlockList({ blocks, currentBlockId }: WatchBlockListProps) {
  if (blocks.length === 0) {
    return (
      <div className="flex items-center justify-center py-12 text-muted-foreground">
        Waiting for first command…
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2 overflow-y-auto px-2 py-2">
      {blocks.map((block) => (
        <WatchBlockItem
          key={block.id}
          block={block}
          isCurrentBlock={block.id === currentBlockId}
        />
      ))}
    </div>
  );
}
