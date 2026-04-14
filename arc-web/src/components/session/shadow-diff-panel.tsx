"use client";

import { useCallback, useState } from "react";
import { Button } from "@/components/ui/button";
import { apiClient, ApiError } from "@/lib/api";
import type { ShadowDiffResponse } from "@/types/api";

interface Props {
  sessionId: string;
}

type LoadState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "loaded"; diff: string }
  | { kind: "error"; message: string };

// Palette mirrors beacon-mac/Sources/BeaconMac/Views/ShadowDiffPanel.swift so
// the web and native surfaces look the same.
const COLORS = {
  add: "#A6E3A1",
  remove: "#F38BA8",
  hunk: "#CBA6F7",
  context: "#6C7086",
  bg: "#181825",
};

function colorForLine(line: string): string {
  if (line.startsWith("+++") || line.startsWith("---")) return COLORS.hunk;
  if (line.startsWith("@@")) return COLORS.hunk;
  if (line.startsWith("+")) return COLORS.add;
  if (line.startsWith("-")) return COLORS.remove;
  return COLORS.context;
}

export function ShadowDiffPanel({ sessionId }: Props) {
  const [state, setState] = useState<LoadState>({ kind: "idle" });

  const load = useCallback(async () => {
    setState({ kind: "loading" });
    try {
      const resp = await apiClient.get<ShadowDiffResponse>(
        `/api/sessions/${encodeURIComponent(sessionId)}/shadow-diff`,
      );
      setState({ kind: "loaded", diff: resp.diff });
    } catch (err) {
      const message =
        err instanceof ApiError
          ? `HTTP ${err.status}: ${err.message}`
          : err instanceof Error
            ? err.message
            : "Request failed";
      setState({ kind: "error", message });
    }
  }, [sessionId]);

  return (
    <div
      data-testid="shadow-diff-panel"
      className="flex h-full flex-col"
      style={{ backgroundColor: COLORS.bg }}
    >
      <div className="flex items-center gap-2 border-b border-white/10 px-3 py-2">
        <h2 className="text-sm font-medium text-white/80">Shadow Diff</h2>
        <span className="text-xs text-white/40">HEAD → working tree</span>
        <span className="flex-1" />
        <Button size="sm" variant="outline" onClick={load} disabled={state.kind === "loading"}>
          {state.kind === "loading" ? "Loading…" : "Refresh"}
        </Button>
      </div>

      <div className="flex-1 overflow-auto p-3 font-mono text-xs leading-5">
        {state.kind === "idle" && (
          <p className="text-white/40">
            Click Refresh to fetch the current diff from the host.
          </p>
        )}
        {state.kind === "loading" && (
          <p className="text-white/40">Fetching diff from host…</p>
        )}
        {state.kind === "error" && (
          <p style={{ color: COLORS.remove }} data-testid="shadow-diff-error">
            {state.message}
          </p>
        )}
        {state.kind === "loaded" &&
          (state.diff.length === 0 ? (
            <p className="text-white/40" data-testid="shadow-diff-empty">
              No changes detected.
            </p>
          ) : (
            <pre
              data-testid="shadow-diff-content"
              className="whitespace-pre"
              style={{ color: COLORS.context }}
            >
              {state.diff.split("\n").map((line, i) => (
                <span key={i} style={{ color: colorForLine(line) }}>
                  {line}
                  {"\n"}
                </span>
              ))}
            </pre>
          ))}
      </div>
    </div>
  );
}
