"use client";

import { useParams, useRouter } from "next/navigation";
import { useRef, useCallback } from "react";
import { useWebSocket } from "@/hooks/use-websocket";
import { useBlocks } from "@/hooks/use-blocks";
import { BlockList } from "@/components/blocks/block-list";
import { XtermWrapper, writeToXterm } from "@/components/terminal/xterm-wrapper";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { ControlBar } from "@/components/session/control-bar";
import type { ArcToClient } from "@/types/protocol";

export default function SessionAttachPage() {
  const { id } = useParams<{ id: string }>();
  const router = useRouter();
  const xtermContainerRef = useRef<HTMLDivElement>(null);
  const { blocks, replaying, replayTotal, replayLoaded, handleMessage } =
    useBlocks();

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
    },
    [handleMessage],
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

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <div className="flex items-center gap-3 border-b px-4 py-3">
        <Button variant="ghost" size="sm" onClick={() => router.push("/sessions")}>
          &larr; Sessions
        </Button>
        <h1 className="font-mono text-lg">{id}</h1>
      </div>

      {/* Replay progress */}
      {replaying && (
        <div className="border-b bg-muted px-4 py-2 text-sm text-muted-foreground">
          Loading history: {replayLoaded} / {replayTotal} blocks
        </div>
      )}

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
        <span className="flex-1" />
        <ControlBar
          isController={true}
          controllerName="you"
          pendingRequests={[]}
          onRequestControl={() => {}}
          onGrantControl={() => {}}
          onDenyControl={() => {}}
        />
      </div>
    </div>
  );
}
