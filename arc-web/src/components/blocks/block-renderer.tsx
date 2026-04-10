"use client";

import { useState, useMemo } from "react";
import type { Block } from "@/types/block";
import { BlockHeaderBar } from "./block-header";
import { ansiToHtml } from "@/lib/ansi";

interface Props {
  block: Block;
}

export function BlockRenderer({ block }: Props) {
  const [collapsed, setCollapsed] = useState(false);

  const outputHtml = useMemo(() => ansiToHtml(block.output), [block.output]);

  return (
    <div className="rounded border bg-card">
      <BlockHeaderBar header={block.header} footer={block.footer} />

      <div
        className="cursor-pointer border-t px-3 py-1.5 font-mono text-sm"
        onClick={() => setCollapsed(!collapsed)}
      >
        <span className="text-muted-foreground">$ </span>
        {block.header.command}
      </div>

      {!collapsed && outputHtml && (
        <div
          className="border-t bg-[#1e1e1e] px-3 py-2 font-mono text-sm text-[#d4d4d4] overflow-x-auto"
          dangerouslySetInnerHTML={{ __html: outputHtml }}
        />
      )}
    </div>
  );
}
