"use client";

import { useState, useCallback, useRef } from "react";
import type { Block, BlockHeader, BlockFooter } from "@/types/block";
import type { ArcToClient } from "@/types/protocol";

interface BlocksState {
  blocks: Block[];
  replaying: boolean;
  replayTotal: number;
  replayLoaded: number;
}

export function useBlocks() {
  const [state, setState] = useState<BlocksState>({
    blocks: [],
    replaying: false,
    replayTotal: 0,
    replayLoaded: 0,
  });

  // Track the current live block being built from streaming chunks
  const liveBlockRef = useRef<{
    header: BlockHeader;
    chunks: Uint8Array[];
  } | null>(null);

  const handleMessage = useCallback((msg: ArcToClient) => {
    switch (msg.kind) {
      case "ReplayBegin":
        setState((prev) => ({
          ...prev,
          replaying: true,
          replayTotal: msg.data.total_blocks,
          replayLoaded: 0,
        }));
        break;

      case "ReplayBlock":
        setState((prev) => ({
          ...prev,
          blocks: [...prev.blocks, msg.data.block],
          replayLoaded: prev.replayLoaded + 1,
        }));
        break;

      case "ReplayEnd":
        setState((prev) => ({ ...prev, replaying: false }));
        break;

      case "BlockStart":
        liveBlockRef.current = {
          header: msg.data.header,
          chunks: [],
        };
        break;

      case "BlockOutputChunk":
        if (liveBlockRef.current) {
          liveBlockRef.current.chunks.push(new Uint8Array(msg.data.bytes));
        }
        break;

      case "BlockEnd": {
        const live = liveBlockRef.current;
        if (live) {
          // Merge chunks into single output
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
            footer: msg.data.footer,
          };

          setState((prev) => ({
            ...prev,
            blocks: [...prev.blocks, completedBlock],
          }));
          liveBlockRef.current = null;
        }
        break;
      }
    }
  }, []);

  return {
    blocks: state.blocks,
    replaying: state.replaying,
    replayTotal: state.replayTotal,
    replayLoaded: state.replayLoaded,
    handleMessage,
    liveBlock: liveBlockRef,
  };
}
