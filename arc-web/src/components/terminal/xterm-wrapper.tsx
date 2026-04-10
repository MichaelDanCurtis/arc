"use client";

import { useEffect, useRef, useCallback } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";

interface Props {
  onData?: (data: Uint8Array) => void;
  onResize?: (cols: number, rows: number) => void;
  readOnly?: boolean;
}

export function XtermWrapper({ onData, onResize, readOnly = false }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);

  // Expose write method via stable callback
  const write = useCallback((data: Uint8Array | string) => {
    terminalRef.current?.write(data);
  }, []);

  useEffect(() => {
    if (!containerRef.current) return;

    const term = new Terminal({
      cursorBlink: !readOnly,
      disableStdin: readOnly,
      theme: {
        background: "#1e1e1e",
        foreground: "#d4d4d4",
        cursor: "#d4d4d4",
      },
      fontSize: 14,
      fontFamily: "'JetBrains Mono', 'Fira Code', monospace",
    });

    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);

    term.open(containerRef.current);

    // Try WebGL renderer, fall back to canvas
    try {
      term.loadAddon(new WebglAddon());
    } catch {
      // WebGL not available, canvas fallback is fine
    }

    fitAddon.fit();

    terminalRef.current = term;
    fitAddonRef.current = fitAddon;

    // Handle user input
    if (!readOnly && onData) {
      term.onData((str) => {
        onData(new TextEncoder().encode(str));
      });
    }

    // Handle resize
    term.onResize(({ cols, rows }) => {
      onResize?.(cols, rows);
    });

    // Observe container resize
    const observer = new ResizeObserver(() => {
      fitAddon.fit();
    });
    observer.observe(containerRef.current!);

    return () => {
      observer.disconnect();
      term.dispose();
      terminalRef.current = null;
      fitAddonRef.current = null;
    };
  }, [readOnly, onData, onResize]);

  // Attach write to the container element so parent can call it
  useEffect(() => {
    const el = containerRef.current;
    if (el) {
      (el as any).__xtermWrite = write;
    }
  }, [write]);

  return (
    <div
      ref={containerRef}
      className="h-64 w-full rounded border bg-[#1e1e1e]"
    />
  );
}

// Helper to write to an XtermWrapper via its container ref
export function writeToXterm(containerEl: HTMLDivElement | null, data: Uint8Array | string) {
  if (containerEl && (containerEl as any).__xtermWrite) {
    (containerEl as any).__xtermWrite(data);
  }
}
