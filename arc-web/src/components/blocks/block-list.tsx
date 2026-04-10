"use client";

import type { Block } from "@/types/block";
import { BlockRenderer } from "./block-renderer";

interface Props {
  blocks: Block[];
  height: number;
}

export function BlockList({ blocks, height }: Props) {
  if (blocks.length === 0) {
    return (
      <div className="flex items-center justify-center py-12 text-muted-foreground">
        No blocks yet
      </div>
    );
  }

  return (
    <div
      className="overflow-y-auto"
      style={{ height }}
    >
      {blocks.map((block) => (
        <div key={block.header.id} className="px-2 py-1">
          <BlockRenderer block={block} />
        </div>
      ))}
    </div>
  );
}
