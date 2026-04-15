"use client";

/**
 * /sessions/[id]/watch — View-only mirror of a remote PTY session.
 *
 * This is a sibling route to /sessions/[id] (the direct-attach page).
 * It does NOT modify the attach page; the two paths use separate wire modes:
 *   attach → AttachSession handshake (takes PTY control)
 *   mirror → SubscribeSession handshake (read-only observer stream)
 *
 * Effect-ordering note (UnsubscribeSession before ws.close):
 * React runs effect cleanups in the REVERSE order of registration.
 * useWebSocket registers its cleanup effect first (via the hook call).
 * The "unsubscribe" useEffect below is declared AFTER useWebSocket, so its
 * cleanup fires FIRST on unmount — sending UnsubscribeSession before the
 * hook's own ws.close() runs. This gives the relay a clean presence signal.
 *
 * Deferred:
 *   C3 — control UI (RequestControl / Grant / Deny / Revoke)
 *   C4 — BlockList for watchers (WASM block parsing)
 */

import { useParams, useRouter } from "next/navigation";
import { useRef, useCallback, useState, useEffect, useLayoutEffect } from "react";
import { useWebSocket } from "@/hooks/use-websocket";
import { XtermWrapper, type XtermHandle } from "@/components/terminal/xterm-wrapper";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { ArcToClient, DriverInfo, WatcherInfo, ClientToArc } from "@/types/protocol";

export default function WatchPage() {
  const { id } = useParams<{ id: string }>();
  const router = useRouter();
  const xtermRef = useRef<XtermHandle>(null);

  // Mirror state
  const [driver, setDriver] = useState<DriverInfo | null>(null);
  const [watchers, setWatchers] = useState<WatcherInfo[]>([]);
  const [revokedReason, setRevokedReason] = useState<string | null>(null);
  // Request Control pending state: true after click, cleared on any LeaseChanged
  const [controlPending, setControlPending] = useState(false);

  // Track the highest seq received from SessionOutput for discontinuity resume.
  const lastSeenSeqRef = useRef<number | null>(null);

  // Stable ref to `send` so the onMessage callback can call it without
  // listing it as a dependency (avoiding stale closure or infinite re-render).
  const sendRef = useRef<((msg: ClientToArc) => void)>(() => {});

  const onMessage = useCallback(
    (msg: ArcToClient) => {
      // Only process envelopes for this session.
      // All mirroring envelope types carry a session_id inside .data.
      if (msg.data !== null && "session_id" in msg.data) {
        if ((msg.data as { session_id: string }).session_id !== id) return;
      }

      switch (msg.kind) {
        case "SessionOutput": {
          // Write bytes to xterm for both live and replay frames.
          // replay:true is historical content — write it verbatim.
          // We intentionally do NOT show a "new activity" indicator for
          // replay frames since they represent already-completed history.
          xtermRef.current?.write(new Uint8Array(msg.data.bytes));
          // Track highest seq for discontinuity resume
          if (lastSeenSeqRef.current === null || msg.data.seq > lastSeenSeqRef.current) {
            lastSeenSeqRef.current = msg.data.seq;
          }
          break;
        }
        case "LeaseChanged": {
          setDriver(msg.data.driver);
          // Clear pending request state on any lease change (granted or denied)
          setControlPending(false);
          break;
        }
        case "PresenceChanged": {
          setWatchers(msg.data.watchers);
          break;
        }
        case "SessionRevoked": {
          setRevokedReason(msg.data.reason);
          break;
        }
        case "SessionDiscontinuity": {
          // A gap was detected in the output stream. Re-subscribe from the
          // last seq we actually received so we don't miss any frames.
          sendRef.current({
            kind: "SubscribeSession",
            data: {
              session_id: id,
              since_seq: lastSeenSeqRef.current,
            },
          });
          break;
        }
        // Non-mirror kinds (e.g. BlockOutputChunk, Ping) are intentionally dropped
        // on the view-only watch page.
        default:
          break;
      }
    },
    [id],
  );

  // Hook call — registers its cleanup effect internally (ws.close on unmount).
  // This must come BEFORE the unsubscribe useEffect below.
  const { state: wsState, send } = useWebSocket(id, onMessage, "mirror");

  // Keep sendRef in sync with the latest send identity.
  // useLayoutEffect runs synchronously after DOM mutations but before paint,
  // ensuring the ref is always current before any subsequent effects fire.
  useLayoutEffect(() => {
    sendRef.current = send;
  });

  // UnsubscribeSession cleanup — declared AFTER useWebSocket so React runs
  // THIS cleanup first (reverse registration order), ensuring the frame is
  // sent before the websocket is closed by the hook's own cleanup.
  useEffect(() => {
    return () => {
      sendRef.current({
        kind: "UnsubscribeSession",
        data: { session_id: id },
      });
    };
  }, [id]);

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <div className="flex items-center gap-3 border-b px-4 py-3">
        <Button
          variant="ghost"
          size="sm"
          onClick={() => router.push("/sessions")}
        >
          &larr; Sessions
        </Button>
        <h1 className="font-mono text-lg">{id}</h1>
        <span className="text-xs text-muted-foreground">(view only)</span>
        <span className="flex-1" />

        {/* Driver indicator */}
        <span
          className="text-sm text-muted-foreground"
          data-testid="driver-indicator"
        >
          {driver ? (
            <>
              Driver:{" "}
              <span className="font-medium text-foreground">
                {driver.device_label}
              </span>{" "}
              ({driver.user_id})
            </>
          ) : (
            "No driver"
          )}
        </span>

        {/* Request Control — only shown when someone else is driving */}
        {driver && (
          <Button
            variant="outline"
            size="sm"
            disabled={controlPending}
            onClick={() => {
              // Stuck-pending is a known v0 limitation; mirrors Phase 4.7 Mac pattern.
              // Relay's LeaseChanged signal is reliable in practice and clears this state.
              setControlPending(true);
              sendRef.current({ kind: "RequestControl", data: { session_id: id } });
            }}
          >
            {controlPending ? "Requesting..." : "Request Control"}
          </Button>
        )}
      </div>

      {/* SessionRevoked banner */}
      {revokedReason && (
        <div
          role="alert"
          className="flex items-center gap-3 border-b border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive"
          data-testid="revoked-banner"
        >
          <span className="flex-1">Session revoked: {revokedReason}</span>
          <Button
            variant="outline"
            size="sm"
            onClick={() => router.push("/sessions")}
          >
            Back to Sessions
          </Button>
        </div>
      )}

      {/* Main area: terminal + optional watchers sidebar */}
      <div className="flex flex-1 overflow-hidden">
        {/* Read-only xterm terminal */}
        <div className="flex min-w-0 flex-1 flex-col">
          <div className="flex-1">
            <XtermWrapper ref={xtermRef} readOnly={true} />
          </div>
        </div>

        {/* Watchers sidebar — shown when there is at least one watcher */}
        {watchers.length > 0 && (
          <aside
            className="w-48 shrink-0 border-l border-white/10 bg-muted/30 p-3"
            data-testid="watchers-sidebar"
          >
            <h2 className="mb-2 text-xs font-semibold uppercase tracking-wider text-muted-foreground">
              Watchers ({watchers.length})
            </h2>
            <ul className="space-y-1">
              {watchers.map((w) => (
                <li
                  key={`${w.user_id}:${w.device_label}`}
                  className="text-xs"
                >
                  <span className="font-medium">{w.device_label}</span>
                  <span className="ml-1 text-muted-foreground">
                    ({w.user_id})
                  </span>
                </li>
              ))}
            </ul>
          </aside>
        )}
      </div>

      {/* Status bar */}
      <div className="flex items-center gap-3 border-t px-4 py-2 text-sm text-muted-foreground">
        <span className="flex items-center gap-1.5">
          <span
            className={cn(
              "h-2 w-2 rounded-full",
              wsState === "connected"
                ? "bg-green-500"
                : wsState === "connecting"
                  ? "bg-yellow-500"
                  : "bg-gray-400",
            )}
          />
          {wsState}
        </span>
        <span className="flex-1" />
        <span className="text-xs opacity-60">read-only mirror</span>
      </div>
    </div>
  );
}
