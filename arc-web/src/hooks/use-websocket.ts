"use client";

import { useState, useEffect, useRef, useCallback } from "react";
import { connectToRelay, type ArcWebSocket, type WsState, type ConnectMode } from "@/lib/ws";
import type { ArcToClient, ClientToArc } from "@/types/protocol";

export function useWebSocket(
  sessionId: string | null,
  onMessage: (msg: ArcToClient) => void,
  mode: ConnectMode = "attach",
) {
  const [state, setState] = useState<WsState>("disconnected");
  const wsRef = useRef<ArcWebSocket | null>(null);
  const onMessageRef = useRef(onMessage);
  onMessageRef.current = onMessage;

  useEffect(() => {
    if (!sessionId) return;

    const ws = connectToRelay(
      sessionId,
      (msg) => onMessageRef.current(msg),
      setState,
      mode,
    );
    wsRef.current = ws;

    return () => {
      ws.close();
      wsRef.current = null;
    };
  }, [sessionId, mode]);

  const send = useCallback((msg: ClientToArc) => {
    wsRef.current?.send(msg);
  }, []);

  return { state, send };
}
