"use client";

import { useState, useEffect, useRef, useCallback } from "react";
import { connectToRelay, type ArcWebSocket, type WsState } from "@/lib/ws";
import type { ArcToClient, ClientToArc } from "@/types/protocol";

export function useWebSocket(
  sessionId: string | null,
  onMessage: (msg: ArcToClient) => void,
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
    );
    wsRef.current = ws;

    return () => {
      ws.close();
      wsRef.current = null;
    };
  }, [sessionId]);

  const send = useCallback((msg: ClientToArc) => {
    wsRef.current?.send(msg);
  }, []);

  return { state, send };
}
