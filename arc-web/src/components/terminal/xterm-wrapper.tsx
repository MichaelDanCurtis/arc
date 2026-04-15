"use client";

import { useEffect, useRef, useLayoutEffect, forwardRef, useImperativeHandle } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";

export interface XtermHandle {
  write: (data: Uint8Array | string) => void;
}

interface Props {
  onData?: (data: Uint8Array) => void;
  onResize?: (cols: number, rows: number) => void;
  readOnly?: boolean;
}

export const XtermWrapper = forwardRef<XtermHandle, Props>(function XtermWrapper(
  { onData, onResize, readOnly = false },
  ref,
) {
  const containerRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);

  // Stable refs for callbacks — updated every render so listeners installed once
  // always dispatch to the latest prop values without needing to reinstall.
  const onDataRef = useRef(onData);
  const onResizeRef = useRef(onResize);
  useLayoutEffect(() => {
    onDataRef.current = onData;
  });
  useLayoutEffect(() => {
    onResizeRef.current = onResize;
  });

  // Expose write method via imperative handle
  useImperativeHandle(
    ref,
    () => ({
      write: (data: Uint8Array | string) => {
        terminalRef.current?.write(data);
      },
    }),
    [],
  );

  // One-time mount effect — no prop deps so toggling readOnly/onData/onResize
  // never disposes the terminal (which would wipe scrollback).
  useEffect(() => {
    if (!containerRef.current) return;

    const term = new Terminal({
      cursorBlink: false, // Controlled separately by the readOnly effect below
      disableStdin: true, // Controlled separately by the readOnly effect below
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

    // Install data listener once — dispatches to current ref value.
    // disableStdin prevents xterm from emitting input when read-only,
    // so the listener simply won't fire; no guard needed here.
    term.onData((str) => {
      onDataRef.current?.(new TextEncoder().encode(str));
    });

    // Install resize listener once
    term.onResize(({ cols, rows }) => {
      onResizeRef.current?.(cols, rows);
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
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Live-toggle readOnly without reinitialising the terminal.
  // Runs after mount (terminalRef is populated) and on every readOnly change.
  useEffect(() => {
    const term = terminalRef.current;
    if (!term) return;
    term.options.disableStdin = readOnly;
    term.options.cursorBlink = !readOnly;
  }, [readOnly]);

  return (
    <div
      ref={containerRef}
      className="h-64 w-full rounded border bg-[#1e1e1e]"
    />
  );
});

XtermWrapper.displayName = "XtermWrapper";
