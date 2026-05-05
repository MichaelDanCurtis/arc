"use client";

/**
 * /sessions/[id]/watch — View-only mirror of a remote PTY session.
 *
 * This is a sibling route to /sessions/[id] (the direct-attach page).
 * It does NOT modify the attach page; the two paths use separate wire modes:
 *   attach → arc.session.attach (takes PTY control)
 *   mirror → arc.session.subscribe (read-only observer stream)
 *
 * Effect-ordering note (arc.session.unsubscribe before ws.close):
 * React runs effect cleanups in the REVERSE order of registration.
 * useArcJsonRpc registers its cleanup effect first (via the hook call).
 * The "unsubscribe" useEffect below is declared AFTER useArcJsonRpc, so its
 * cleanup fires FIRST on unmount — sending arc.session.unsubscribe before the
 * hook's own ws.close() runs. This gives the relay a clean presence signal.
 *
 * C3.5 — isDriver detection heuristic:
 *   Browser has no myUserId concept (intentional deferral). We use a local
 *   pendingRequestMine flag: set true on click, cleared on any control_grant/revoke.
 *   If cleared while a grant arrived → assume grant-to-me → isDriver=true.
 *
 *   Known edge case: if two watchers click near-simultaneously and the other
 *   watcher wins, we show "You are driving" for one render before the relay's
 *   lease check drops our first input. The server-side lease check in
 *   arc-relay handle_session_input is the authority; this UI is a best-effort
 *   hint only.
 */

import { useParams, useRouter } from "next/navigation";
import { useRef, useCallback, useState, useEffect, useLayoutEffect } from "react";
import { useArcJsonRpc } from "@/hooks/use-arc-jsonrpc";
import { useMirrorBlocks } from "@/hooks/use-mirror-blocks";
import { WatchBlockList } from "@/components/watch/WatchBlockList";
import { XtermWrapper, type XtermHandle } from "@/components/terminal/xterm-wrapper";
import { ChevronRightIcon, ChevronLeftIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { b64ToBytes } from "@/lib/b64";
import { getAccessToken } from "@/lib/auth";
import { getWsUrl } from "@/lib/config";
import { ArcMethod, ArcNotif } from "@/types/arc-rpc";
import type {
  MirrorFrameNotif,
  MirrorControlGrantNotif,
  MirrorControlRevokeNotif,
  SessionParticipantNotif,
  SessionRevokedNotif,
  MirrorDiscontinuityNotif,
  SessionParticipant,
} from "@/types/arc-rpc";
import type { Notification } from "@/lib/jsonrpc-ws";

/** Minimal driver info tracked from arc.mirror.control_grant. */
interface DriverInfo {
  controller_id: string;
  controller_name?: string;
}

export default function WatchPage() {
  const { id } = useParams<{ id: string }>();
  const router = useRouter();
  const xtermRef = useRef<XtermHandle>(null);

  // Mirror state
  const [driver, setDriver] = useState<DriverInfo | null>(null);
  const [watchers, setWatchers] = useState<SessionParticipant[]>([]);
  const [revokedReason, setRevokedReason] = useState<string | null>(null);
  // pendingRequestMine: true after "Request Control" click, cleared on any control_grant/revoke
  const [pendingRequestMine, setPendingRequestMine] = useState(false);
  // isDriver: true when we believe the relay has granted us control
  const [isDriver, setIsDriver] = useState(false);

  // Block-list sidebar collapse state (plain component state — no localStorage persistence).
  const [blockListOpen, setBlockListOpen] = useState(true);

  // useMirrorBlocks: lazily loads WASM detector; returns feedBytes callback.
  const { blocks, currentBlockId, feedBytes } = useMirrorBlocks(id);

  // Track the highest seq received from arc.mirror.frame for discontinuity resume.
  const lastSeenSeqRef = useRef<number | null>(null);

  // Stable ref to `request` so the onNotification + cleanup callbacks can call it
  // without listing it as a dependency (avoiding stale closure or infinite re-render).
  const requestRef = useRef<(method: string, params: unknown) => Promise<unknown>>(
    () => Promise.reject(new Error("not connected")),
  );

  // Read token + URL once per render (stable values — only change on auth refresh).
  const token = getAccessToken();
  const wsUrl = getWsUrl();

  const onNotification = useCallback(
    (notif: Notification) => {
      const params = notif.params as Record<string, unknown> | null;

      // Only process envelopes for this session.
      if (params && "session_id" in params && params.session_id !== id) return;

      switch (notif.method) {
        case ArcNotif.MirrorFrame: {
          const p = params as unknown as MirrorFrameNotif;
          const chunk = b64ToBytes(p.data);
          // Write bytes to xterm for both live and replay frames.
          xtermRef.current?.write(chunk);
          // Feed the same bytes into the block detector for WatchBlockList.
          feedBytes(chunk);
          // Track highest seq for discontinuity resume.
          if (lastSeenSeqRef.current === null || p.seq > lastSeenSeqRef.current) {
            lastSeenSeqRef.current = p.seq;
          }
          break;
        }

        case ArcNotif.MirrorControlGrant: {
          const p = params as unknown as MirrorControlGrantNotif;
          const newDriver: DriverInfo = {
            controller_id: p.controller_id,
            controller_name: p.controller_name,
          };
          setDriver(newDriver);
          // Detection heuristic: if we had a pending request and a grant arrives,
          // assume the grant was to us.
          setPendingRequestMine((pending) => {
            setIsDriver(pending); // grant while pending → we are driver
            return false; // always clear pending
          });
          break;
        }

        case ArcNotif.MirrorControlRevoke: {
          setDriver(null);
          // Revoke while pending means denial — clear pending, do NOT set isDriver.
          setPendingRequestMine((pending) => {
            if (pending) setIsDriver(false);
            return false;
          });
          setIsDriver(false);
          break;
        }

        case ArcNotif.SessionParticipant: {
          const p = params as unknown as SessionParticipantNotif;
          setWatchers(p.participants);
          break;
        }

        case ArcNotif.SessionRevoked: {
          const p = params as unknown as SessionRevokedNotif;
          setRevokedReason(p.reason ?? "unknown reason");
          setIsDriver(false);
          break;
        }

        case ArcNotif.MirrorDiscontinuity: {
          // A gap was detected in the output stream. Re-subscribe from the
          // last seq we actually received so we don't miss any frames.
          requestRef.current(ArcMethod.SessionSubscribe, {
            session_id: id,
            since_seq: lastSeenSeqRef.current,
          }).catch(() => {
            // Best-effort: ignore if disconnected during discontinuity handling.
          });
          break;
        }

        case ArcNotif.MirrorHostInput:
          // Host-input attribution UI — future enhancement; silently consumed.
          break;

        default:
          break;
      }
    },
    [id, feedBytes],
  );

  // Hook call — registers its cleanup effect internally (ws.close on unmount).
  // This must come BEFORE the unsubscribe useEffect below.
  const { state: wsState, request } = useArcJsonRpc({
    url: wsUrl,
    token,
    enabled: !!token,
    onConnect: async () => {
      await requestRef.current(ArcMethod.SessionSubscribe, { session_id: id });
    },
    onNotification,
  });

  // Keep requestRef in sync with the latest request identity each render.
  useLayoutEffect(() => {
    requestRef.current = request;
  });

  // arc.session.unsubscribe cleanup — declared AFTER useArcJsonRpc so React runs
  // THIS cleanup first (reverse registration order), ensuring the frame is
  // sent before the websocket is closed by the hook's own cleanup.
  useEffect(() => {
    return () => {
      requestRef.current(ArcMethod.SessionUnsubscribe, { session_id: id }).catch(() => {
        // Best-effort: ignore rejection if connection is already closing.
      });
    };
  }, [id]);

  // ── Input handlers (used when isDriver) ────────────────────────────────

  const handleData = useCallback(
    (bytes: Uint8Array) => {
      import("@/lib/b64").then(({ bytesToB64 }) => {
        requestRef.current(ArcMethod.SessionInput, {
          session_id: id,
          data: bytesToB64(bytes),
        }).catch(() => {});
      });
    },
    [id],
  );

  const handleResize = useCallback(
    (cols: number, rows: number) => {
      requestRef.current(ArcMethod.SessionResize, {
        session_id: id,
        cols,
        rows,
      }).catch(() => {});
    },
    [id],
  );

  const handleReleaseControl = useCallback(() => {
    requestRef.current(ArcMethod.SessionRelease, {
      session_id: id,
      controller_id: driver?.controller_id ?? "",
    }).catch(() => {});
    setIsDriver(false);
  }, [id, driver]);

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
                  {driver.controller_name ?? driver.controller_id}
                </span>{" "}
                ({driver.controller_id})
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
              requestRef.current(ArcMethod.SessionControl, {
                session_id: id,
                requester_id: "",
              }).catch(() => {});
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
                    key={`${w.user_id}:${w.user_name ?? ""}`}
                    className="text-xs"
                  >
                    <span className="font-medium">{w.user_name ?? w.user_id}</span>
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
