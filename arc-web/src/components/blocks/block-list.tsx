"use client";

import { useRef, useCallback } from "react";
import { VariableSizeList as List } from "react-window";
import type { Block } from "@/types/block";
import { BlockRenderer } from "./block-renderer";

interface Props {
  blocks: Block[];
  height: number;
}

// Estimate height: header(32) + command(36) + output(~80) + borders(4)
const ESTIMATED_ROW_HEIGHT = 152;

export function BlockList({ blocks, height }: Props) {
  const listRef = useRef<List>(null);

  const getItemSize = useCallback(() => ESTIMATED_ROW_HEIGHT, []);

  if (blocks.length === 0) {
    return (
      <div className="flex items-center justify-center py-12 text-muted-foreground">
        No blocks yet
      </div>
    );
  }

  return (
    <List
      ref={listRef}
      height={height}
      width="100%"
      itemCount={blocks.length}
      itemSize={getItemSize}
      overscanCount={5}
    >
      {({ index, style }) => (
        <div style={style} className="px-2 py-1">
          <BlockRenderer block={blocks[index]} />
        </div>
      )}
    </List>
  );
}
