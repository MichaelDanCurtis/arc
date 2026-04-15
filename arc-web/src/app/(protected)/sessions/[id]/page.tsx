"use client";

import { useParams, useRouter } from "next/navigation";
import { useRef, useCallback, useState } from "react";
import { useWebSocket } from "@/hooks/use-websocket";
import { useBlocks } from "@/hooks/use-blocks";
import { BlockList } from "@/components/blocks/block-list";
import { XtermWrapper, writeToXterm } from "@/components/terminal/xterm-wrapper";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { ShadowDiffPanel } from "@/components/session/shadow-diff-panel";
import type { ArcToClient, DriverInfo, WatcherInfo } from "@/types/protocol";

// A pending control request from a watcher.
interface ControlRequest {
  request_id: string;
  from_user: string;
  from_device: string;
}

export default function SessionAttachPage() {
  const { id } = useParams<{ id: string }>();
  const router = useRouter();
  const xtermContainerRef = useRef<HTMLDivElement>(null);
  const [showShadowDiff, setShowShadowDiff] = useState(false);
  const { blocks, replaying, replayTotal, replayLoaded, handleMessage } =
    useBlocks();

  // Mirror control state
  const [pendingRequests, setPendingRequests] = useState<ControlRequest[]>([]);
  const [currentDriver, setCurrentDriver] = useState<DriverInfo | null>(null);
  const [watchers, setWatchers] = useState<WatcherInfo[]>([]);

  const onMessage = useCallback(
    (msg: ArcToClient) => {
      handleMessage(msg);

      // Write live output chunks directly to xterm
      if (msg.kind === "BlockOutputChunk") {
        writeToXterm(
          xtermContainerRef.current,
          new Uint8Array(msg.data.bytes),
        );
      }

      // Mirror-mode events — only process envelopes for this session
      if (msg.data !== null && "session_id" in msg.data) {
        if ((msg.data as { session_id: string }).session_id !== id) return;
      }

      switch (msg.kind) {
        case "ControlRequested":
          setPendingRequests((prev) => [
            ...prev,
            {
              request_id: msg.data.request_id,
              from_user: msg.data.from_user,
              from_device: msg.data.from_device,
            },
          ]);
          break;
        case "LeaseChanged":
          setCurrentDriver(msg.data.driver);
          break;
        case "PresenceChanged":
          setWatchers(msg.data.watchers);
          break;
        default:
          break;
      }
    },
    [handleMessage, id],
  );

  const { state: wsState, send } = useWebSocket(id, onMessage);

  const handleInput = useCallback(
    (bytes: Uint8Array) => {
      send({
        kind: "SessionInput",
        data: { session_id: id, bytes },
      });
    },
    [send, id],
  );

  const handleResize = useCallback(
    (cols: number, rows: number) => {
      send({
        kind: "SessionResizeRequest",
        data: { session_id: id, cols, rows },
      });
    },
    [send, id],
  );

  const handleGrant = useCallback(
    (request_id: string) => {
      send({ kind: "ApproveControl", data: { session_id: id, request_id } });
      setPendingRequests((prev) => prev.filter((r) => r.request_id !== request_id));
    },
    [send, id],
  );

  const handleDeny = useCallback(
    (request_id: string) => {
      send({ kind: "DenyControl", data: { session_id: id, request_id } });
      setPendingRequests((prev) => prev.filter((r) => r.request_id !== request_id));
    },
    [send, id],
  );

  const handleRevoke = useCallback(() => {
    send({ kind: "RevokeControl", data: { session_id: id } });
  }, [send, id]);

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
                {currentDriver.device_label}
              </span>{" "}
              ({currentDriver.user_id})
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
          className="flex items-center gap-3 border-b border-yellow-500/30 bg-yellow-500/10 px-4 py-2 text-sm"
          data-testid="control-request-banner"
        >
          <span className="flex-1">
            <span className="font-medium">{req.from_device}</span>
            {" "}
            (<span>{req.from_user}</span>)
            {" wants to drive"}
          </span>
          <Button
            size="sm"
            onClick={() => handleGrant(req.request_id)}
          >
            Grant
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={() => handleDeny(req.request_id)}
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
          <div ref={xtermContainerRef}>
            <XtermWrapper
              onData={handleInput}
              onResize={handleResize}
              readOnly={false}
            />
          </div>
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
