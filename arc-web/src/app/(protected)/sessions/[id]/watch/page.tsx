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
 * C3.5 — isDriver detection heuristic:
 *   Browser has no myUserId concept (intentional deferral). We use a local
 *   pendingRequestMine flag: set true on click, cleared on any LeaseChanged.
 *   If cleared while driver is non-null → assume grant-to-me → isDriver=true.
 *
 *   Known edge case: if two watchers click near-simultaneously and the other
 *   watcher wins, we show "You are driving" for one render before the relay's
 *   lease check drops our first input. The server-side lease check in
 *   arc-relay handle_session_input is the authority; this UI is a best-effort
 *   hint only.
 */

import { useParams, useRouter } from "next/navigation";
import { useRef, useCallback, useState, useEffect, useLayoutEffect } from "react";
import { useWebSocket } from "@/hooks/use-websocket";
import { useMirrorBlocks } from "@/hooks/use-mirror-blocks";
import { WatchBlockList } from "@/components/watch/WatchBlockList";
import { XtermWrapper, type XtermHandle } from "@/components/terminal/xterm-wrapper";
import { ChevronRightIcon, ChevronLeftIcon } from "lucide-react";
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
  // pendingRequestMine: true after "Request Control" click, cleared on any LeaseChanged
  const [pendingRequestMine, setPendingRequestMine] = useState(false);
  // isDriver: true when we believe the relay has granted us control
  const [isDriver, setIsDriver] = useState(false);

  // Block-list sidebar collapse state (plain component state — watchers sidebar
  // uses the same pattern: no localStorage persistence).
  const [blockListOpen, setBlockListOpen] = useState(true);

  // useMirrorBlocks: lazily loads WASM detector; returns feedBytes callback.
  const { blocks, currentBlockId, feedBytes } = useMirrorBlocks(id);

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
          const chunk = new Uint8Array(msg.data.bytes);
          // Write bytes to xterm for both live and replay frames.
          xtermRef.current?.write(chunk);
          // Feed the same bytes into the block detector for WatchBlockList.
          feedBytes(chunk);
          // Track highest seq for discontinuity resume
          if (lastSeenSeqRef.current === null || msg.data.seq > lastSeenSeqRef.current) {
            lastSeenSeqRef.current = msg.data.seq;
          }
          break;
        }
        case "LeaseChanged": {
          const newDriver = msg.data.driver;
          setDriver(newDriver);
          // Detection heuristic: if we had a pending request and the new driver
          // is non-null, assume the grant was to us.
          setPendingRequestMine((pending) => {
            // Single decisive expression: become driver iff we had a pending
            // request AND the relay now shows a non-null driver (implicitly us).
            // Every other case — denial (null driver while pending), or a lease
            // change we did not request — sets isDriver=false unambiguously.
            setIsDriver(pending && newDriver !== null);
            return false; // always clear pending
          });
          break;
        }
        case "PresenceChanged": {
          setWatchers(msg.data.watchers);
          break;
        }
        case "SessionRevoked": {
          setRevokedReason(msg.data.reason);
          setIsDriver(false);
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
    [id, feedBytes],
  );

  // Hook call — registers its cleanup effect internally (ws.close on unmount).
  // This must come BEFORE the unsubscribe useEffect below.
  const { state: wsState, send } = useWebSocket(id, onMessage, "mirror");

  // Keep sendRef in sync with the latest send identity.
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

  // ── Input handlers (used when isDriver) ────────────────────────────────

  const handleData = useCallback(
    (bytes: Uint8Array) => {
      sendRef.current({
        kind: "SessionInput",
        data: { session_id: id, bytes },
      });
    },
    [id],
  );

  const handleResize = useCallback(
    (cols: number, rows: number) => {
      sendRef.current({
        kind: "SessionResizeRequest",
        data: { session_id: id, cols, rows },
      });
    },
    [id],
  );

  const handleReleaseControl = useCallback(() => {
    send({ kind: "RevokeControl", data: { session_id: id } });
    setIsDriver(false);
  }, [send, id]);

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
        {!isDriver && (
          <span className="text-xs text-muted-foreground">(view only)</span>
        )}
        <span className="flex-1" />

        {/* Driver indicator */}
        {isDriver ? (
          <span
            className="text-sm font-medium text-primary"
            data-testid="driver-indicator"
            aria-live="polite"
          >
            You are driving
          </span>
        ) : (
          <span
            className="text-sm text-muted-foreground"
            data-testid="driver-indicator"
            aria-live="polite"
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
        )}

        {/* Control button — conditional on isDriver state */}
        {isDriver ? (
          <Button
            variant="destructive"
            size="sm"
            onClick={handleReleaseControl}
          >
            Release Control
          </Button>
        ) : driver ? (
          <Button
            variant="outline"
            size="sm"
            disabled={pendingRequestMine}
            onClick={() => {
              setPendingRequestMine(true);
              sendRef.current({ kind: "RequestControl", data: { session_id: id } });
            }}
          >
            {pendingRequestMine ? "Requesting..." : "Request Control"}
          </Button>
        ) : null}
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

      {/* Main area: terminal + block-list sidebar (D5 split-pane) */}
      <div className="flex flex-1 overflow-hidden">
        {/* xterm terminal — readOnly toggles live without reinit */}
        <div className="flex min-w-0 flex-1 flex-col">
          {/* Watchers sub-bar — shown when there is at least one watcher */}
          {watchers.length > 0 && (
            <div
              className="flex items-center gap-2 border-b border-white/10 bg-muted/20 px-3 py-1.5"
              data-testid="watchers-sidebar"
            >
              <span className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                Watchers ({watchers.length})
              </span>
              <ul className="flex flex-wrap gap-x-3">
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
            </div>
          )}
          <div className="flex-1">
            <XtermWrapper
              ref={xtermRef}
              readOnly={!isDriver}
              onData={isDriver ? handleData : undefined}
              onResize={isDriver ? handleResize : undefined}
            />
          </div>
        </div>

        {/* Block-list sidebar (D5) — always present, collapsible via chevron */}
        <aside
          className={cn(
            "flex shrink-0 flex-col border-l border-white/10 bg-muted/30 transition-all",
            blockListOpen ? "w-72" : "w-8",
          )}
          aria-label="Mirror blocks"
          data-testid="block-list-sidebar"
        >
          {/* Collapse toggle button — mimics watchers-list header style */}
          <div className="flex items-center border-b border-white/10 px-2 py-1.5">
            {blockListOpen && (
              <h2 className="flex-1 text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                Blocks
              </h2>
            )}
            <Button
              variant="ghost"
              size="sm"
              className="h-5 w-5 p-0 text-muted-foreground"
              aria-label={blockListOpen ? "Collapse mirror blocks" : "Expand mirror blocks"}
              aria-expanded={blockListOpen}
              onClick={() => setBlockListOpen((o) => !o)}
              data-testid="block-list-toggle"
            >
              {blockListOpen
                ? <ChevronRightIcon className="size-4" aria-hidden="true" />
                : <ChevronLeftIcon className="size-4" aria-hidden="true" />}
            </Button>
          </div>
          {blockListOpen && (
            <div className="flex-1 overflow-y-auto">
              <WatchBlockList blocks={blocks} currentBlockId={currentBlockId} />
            </div>
          )}
        </aside>
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
        <span className="text-xs opacity-60">
          {isDriver ? "driving" : "read-only mirror"}
        </span>
      </div>
    </div>
  );
}
