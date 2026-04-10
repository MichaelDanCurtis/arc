import { encode as msgpackEncode, decode as msgpackDecode } from "@msgpack/msgpack";
import type { ClientToArc, ArcToClient } from "@/types/protocol";
import { getAccessToken } from "./auth";
import { getWsUrl } from "./config";

export function encodeMessage(msg: ClientToArc): Uint8Array {
  return msgpackEncode(msg) as Uint8Array;
}

export function decodeMessage(bytes: Uint8Array): ArcToClient {
  return msgpackDecode(bytes) as ArcToClient;
}

export type WsState = "connecting" | "connected" | "disconnected" | "error";

export interface ArcWebSocket {
  send(msg: ClientToArc): void;
  close(): void;
  state: WsState;
}

export function connectToRelay(
  sessionId: string,
  onMessage: (msg: ArcToClient) => void,
  onStateChange: (state: WsState) => void,
): ArcWebSocket {
  const token = getAccessToken();
  if (!token) {
    onStateChange("error");
    return { send() {}, close() {}, state: "error" };
  }

  const url = `${getWsUrl()}?token=${encodeURIComponent(token)}`;
  const ws = new WebSocket(url);
  ws.binaryType = "arraybuffer";

  let currentState: WsState = "connecting";
  const setState = (s: WsState) => {
    currentState = s;
    onStateChange(s);
  };

  ws.onopen = () => {
    setState("connected");
    // Send AttachSession immediately
    const attach: ClientToArc = {
      kind: "AttachSession",
      data: { session_id: sessionId, since_block: null },
    };
    ws.send(encodeMessage(attach));
  };

  ws.onmessage = (event) => {
    const data = new Uint8Array(event.data as ArrayBuffer);
    const msg = decodeMessage(data);

    // Auto-respond to Ping with Pong
    if (msg.kind === "Ping") {
      ws.send(encodeMessage({ kind: "Pong", data: null }));
      return;
    }

    onMessage(msg);
  };

  ws.onclose = () => setState("disconnected");
  ws.onerror = () => setState("error");

  return {
    send(msg: ClientToArc) {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(encodeMessage(msg));
      }
    },
    close() {
      ws.close();
    },
    get state() {
      return currentState;
    },
  };
}
