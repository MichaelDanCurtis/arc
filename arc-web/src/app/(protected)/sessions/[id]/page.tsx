"use client";

import { useParams, useRouter } from "next/navigation";
import { useRef, useCallback, useState, useEffect, useLayoutEffect } from "react";
import { useArcJsonRpc } from "@/hooks/use-arc-jsonrpc";
import { BlockList } from "@/components/blocks/block-list";
import { XtermWrapper, type XtermHandle } from "@/components/terminal/xterm-wrapper";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { ShadowDiffPanel } from "@/components/session/shadow-diff-panel";
import { b64ToBytes } from "@/lib/b64";
import { getAccessToken } from "@/lib/auth";
import { getWsUrl } from "@/lib/config";
import { ArcMethod, ArcNotif } from "@/types/arc-rpc";
import type {
  MirrorBlockChunkNotif,
  MirrorControlGrantNotif,
  SessionParticipantNotif,
  SessionGoneNotif,
  SessionControlRequestedNotif,
} from "@/types/arc-rpc";
import type { Block, BlockHeader, BlockFooter } from "@/types/block";
import type { Notification } from "@/lib/jsonrpc-ws";

// A pending control request from a watcher.
interface ControlRequest {
  request_id: string;
  requester_id: string;
  requester_name?: string;
}

// Driver info tracked from arc.mirror.control_grant.
interface DriverInfo {
  controller_id: string;
  controller_name?: string;
}

export default function SessionAttachPage() {
  const { id } = useParams<{ id: string }>();
  const router = useRouter();
  const xtermRef = useRef<XtermHandle>(null);
  const [showShadowDiff, setShowShadowDiff] = useState(false);

  // Block list state (managed locally — mirrors the relay's pre-decoded blocks)
  const [blocks, setBlocks] = useState<Block[]>([]);
  const liveBlockRef = useRef<{ header: BlockHeader; chunks: Uint8Array[] } | null>(null);
  const [replaying, setReplaying] = useState(false);
  const [replayTotal, setReplayTotal] = useState(0);
  const [replayLoaded, setReplayLoaded] = useState(0);

  // Mirror control state
  const [pendingRequests, setPendingRequests] = useState<ControlRequest[]>([]);
  const [currentDriver, setCurrentDriver] = useState<DriverInfo | null>(null);
  const [watchers, setWatchers] = useState<{ user_id: string; user_name?: string }[]>([]);

  // Stable ref to `request` so notification + cleanup callbacks don't become stale.
  const requestRef = useRef<(method: string, params: unknown) => Promise<unknown>>(
    () => Promise.reject(new Error("not connected")),
  );

  const token = getAccessToken();
  const wsUrl = getWsUrl();

  const onNotification = useCallback(
    (notif: Notification) => {
      const params = notif.params as Record<string, unknown> | null;

      // Only process envelopes for this session.
      if (params && "session_id" in params && params.session_id !== id) return;

      switch (notif.method) {
        // ── Live block stream (pre-decoded by relay) ──────────────────────
        case ArcNotif.MirrorBlockStart: {
          liveBlockRef.current = {
            header: (params as { header: BlockHeader }).header,
            chunks: [],
          };
          break;
        }

        case ArcNotif.MirrorBlockChunk: {
          const p = params as unknown as MirrorBlockChunkNotif;
          const bytes = b64ToBytes(p.data);
          // Write to xterm for live rendering
          xtermRef.current?.write(bytes);
          // Buffer for block assembly
          if (liveBlockRef.current) {
            liveBlockRef.current.chunks.push(bytes);
          }
          break;
        }

        case ArcNotif.MirrorBlockEnd: {
          const live = liveBlockRef.current;
          if (live) {
            const totalLen = live.chunks.reduce((sum, c) => sum + c.length, 0);
            const output = new Uint8Array(totalLen);
            let offset = 0;
            for (const chunk of live.chunks) {
              output.set(chunk, offset);
              offset += chunk.length;
            }
            const completedBlock: Block = {
              header: live.header,
              output,
              footer: (params as { footer: BlockFooter }).footer,
            };
            setBlocks((prev) => [...prev, completedBlock]);
            liveBlockRef.current = null;
          }
          break;
        }

        // ── Replay stream ─────────────────────────────────────────────────
        case ArcNotif.MirrorReplayBegin: {
          setReplaying(true);
          setReplayLoaded(0);
          setReplayTotal((params as { total?: number }).total ?? 0);
          break;
        }

        case ArcNotif.MirrorReplayBlock: {
          const block = (params as { block: Block }).block;
          setBlocks((prev) => [...prev, block]);
          setReplayLoaded((n) => n + 1);
          break;
        }

        case ArcNotif.MirrorReplayEnd: {
          setReplaying(false);
          break;
        }

        // ── Control events ────────────────────────────────────────────────
        case ArcNotif.SessionControlRequested: {
          const p = params as unknown as SessionControlRequestedNotif;
          setPendingRequests((prev) => {
            if (prev.some((r) => r.request_id === p.request_id)) return prev;
            return [...prev, {
              request_id: p.request_id,
              requester_id: p.requester_id,
              requester_name: p.requester_name,
            }];
          });
          break;
        }

        case ArcNotif.MirrorControlGrant: {
          const p = params as unknown as MirrorControlGrantNotif;
          setCurrentDriver({
            controller_id: p.controller_id,
            controller_name: p.controller_name,
          });
          break;
        }

        case ArcNotif.MirrorControlRevoke: {
          setCurrentDriver(null);
          break;
        }

        // ── Presence ──────────────────────────────────────────────────────
        case ArcNotif.SessionParticipant: {
          const p = params as unknown as SessionParticipantNotif;
          setWatchers(p.participants.map((w) => ({
            user_id: w.user_id,
            user_name: w.user_name,
          })));
          break;
        }

        // ── Session lifecycle ─────────────────────────────────────────────
        case ArcNotif.SessionGone: {
          // eslint-disable-next-line @typescript-eslint/no-unused-vars
          const _p = params as unknown as SessionGoneNotif;
          router.push("/sessions");
          break;
        }

        default:
          break;
      }
    },
    [id, router],
  );

  // Hook call — registers its own ws.close cleanup internally.
  // Must come BEFORE the detach useEffect so detach fires first on unmount.
  const { state: wsState, request } = useArcJsonRpc({
    url: wsUrl,
    token,
    enabled: !!token,
    onConnect: async () => {
      await requestRef.current(ArcMethod.SessionAttach, {
        session_id: id,
        viewer_id: "",
      });
    },
    onNotification,
  });

  // Keep requestRef in sync each render.
  useLayoutEffect(() => {
    requestRef.current = request;
  });

  // arc.session.detach cleanup — declared AFTER useArcJsonRpc so React runs
  // this cleanup first (reverse registration order), ensuring the frame is
  // sent before the WS is closed.
  useEffect(() => {
    return () => {
      requestRef.current(ArcMethod.SessionDetach, { session_id: id }).catch(() => {});
    };
  }, [id]);

  // ── Output handlers ───────────────────────────────────────────────────────

  const handleInput = useCallback(
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

  const handleGrant = useCallback(
    (req: ControlRequest) => {
      requestRef.current(ArcMethod.SessionApproveControl, {
        session_id: id,
        request_id: req.request_id,
        requester_id: req.requester_id,
      }).catch(() => {});
      setPendingRequests((prev) => prev.filter((r) => r.request_id !== req.request_id));
    },
    [id],
  );

  const handleDeny = useCallback(
    (req: ControlRequest) => {
      requestRef.current(ArcMethod.SessionDenyControl, {
        session_id: id,
        request_id: req.request_id,
        requester_id: req.requester_id,
      }).catch(() => {});
      setPendingRequests((prev) => prev.filter((r) => r.request_id !== req.request_id));
    },
    [id],
  );

  const handleRevoke = useCallback(() => {
    requestRef.current(ArcMethod.SessionRelease, {
      session_id: id,
      controller_id: currentDriver?.controller_id ?? "",
    }).catch(() => {});
  }, [id, currentDriver]);

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <div className="flex items-center gap-3 border-b px-4 py-3">
        <Button variant="ghost" size="sm" onClick={() => router.push("/sessions")}>
          &larr; Sessions
        </Button>
        <h1 className="font-mono text-lg">{id}</h1>
        <span className="flex-1" />

        {/* Watcher count badge */}
        {watchers.length > 0 && (
          <span
            className="text-xs text-muted-foreground"
            data-testid="watcher-count"
          >
            {watchers.length} watching
          </span>
        )}

        {/* Driver indicator + Revoke button */}
        {currentDriver && (
          <>
            <span
              className="text-sm text-muted-foreground"
              data-testid="driver-indicator"
            >
              Driver:{" "}
              <span className="font-medium text-foreground">
                {currentDriver.controller_name ?? currentDriver.controller_id}
              </span>{" "}
              ({currentDriver.controller_id})
            </span>
            <Button
              variant="destructive"
              size="sm"
              onClick={handleRevoke}
            >
              Revoke Control
            </Button>
          </>
        )}

        <Button
          variant={showShadowDiff ? "default" : "outline"}
          size="sm"
          onClick={() => setShowShadowDiff((v) => !v)}
          data-testid="shadow-diff-toggle"
        >
          Shadow Diff
        </Button>
      </div>

      {/* Replay progress */}
      {replaying && (
        <div className="border-b bg-muted px-4 py-2 text-sm text-muted-foreground">
          Loading history: {replayLoaded} / {replayTotal} blocks
        </div>
      )}

      {/* Control request banners — one per pending request */}
      {pendingRequests.map((req) => (
        <div
          key={req.request_id}
          role="alert"
          aria-live="polite"
          className="flex items-center gap-3 border-b border-yellow-500/30 bg-yellow-500/10 px-4 py-2 text-sm"
          data-testid="control-request-banner"
        >
          <span className="flex-1">
            <span className="font-medium">{req.requester_name ?? req.requester_id}</span>
            {" "}
            (<span>{req.requester_id}</span>)
            {" wants to drive"}
          </span>
          <Button
            size="sm"
            onClick={() => handleGrant(req)}
          >
            Grant
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={() => handleDeny(req)}
          >
            Deny
          </Button>
        </div>
      ))}

      {/* Main content area: terminal + optional shadow-diff side panel */}
      <div className="flex flex-1 overflow-hidden">
        <div className="flex min-w-0 flex-1 flex-col">
          {/* Block list (historical) */}
          <div className="flex-1 overflow-hidden">
            <BlockList blocks={blocks} height={400} />
          </div>

          {/* Live terminal (xterm.js) */}
          <XtermWrapper
            ref={xtermRef}
            onData={handleInput}
            onResize={handleResize}
            readOnly={false}
          />
        </div>

        {showShadowDiff && (
          <aside className="w-[420px] shrink-0 border-l border-white/10">
            <ShadowDiffPanel sessionId={id} />
          </aside>
        )}
      </div>

      {/* Status bar */}
      <div className="flex items-center gap-3 border-t px-4 py-2 text-sm">
        <span className="flex items-center gap-1.5">
          <span className={cn(
            "h-2 w-2 rounded-full",
            wsState === "connected" ? "bg-green-500" :
            wsState === "connecting" ? "bg-yellow-500" :
            "bg-gray-400"
          )} />
          {wsState}
        </span>
      </div>
    </div>
  );
}
