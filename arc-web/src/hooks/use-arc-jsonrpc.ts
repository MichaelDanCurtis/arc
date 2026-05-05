"use client";

import { useEffect, useRef, useState } from "react";
import {
  JsonRpcWsClient,
  type Notification,
  type WsState,
} from "@/lib/jsonrpc-ws";

export interface UseArcJsonRpcOptions {
  url: string;
  token: string | null;
  /** Called once after subprotocol-negotiated connect. Use to send initial method (attach/subscribe). */
  onConnect?: (client: JsonRpcWsClient) => void | Promise<void>;
  /** Called for every server-pushed notification. */
  onNotification?: (notif: Notification) => void;
  /** Disable auto-connect; pass false to defer until ready. Defaults to true. */
  enabled?: boolean;
}

export interface UseArcJsonRpcReturn {
  state: WsState;
  /** Sends a JSON-RPC request. Rejects with Error("not connected") if no client. */
  request: (method: string, params: unknown) => Promise<unknown>;
}

export function useArcJsonRpc(options: UseArcJsonRpcOptions): UseArcJsonRpcReturn {
  const { url, token, onConnect, onNotification, enabled = true } = options;

  const [state, setState] = useState<WsState>("disconnected");
  const clientRef = useRef<JsonRpcWsClient | null>(null);
  // Track the disposer returned by client.onNotification so we can clean it up
  const notifDisposerRef = useRef<(() => void) | null>(null);
  // Use a ref for onNotification to avoid re-triggering the effect when the
  // callback identity changes (common with inline arrow functions in pages)
  const onNotificationRef = useRef<((notif: Notification) => void) | undefined>(onNotification);
  const onConnectRef = useRef<((client: JsonRpcWsClient) => void | Promise<void>) | undefined>(onConnect);

  // Keep refs up-to-date without triggering reconnect
  useEffect(() => {
    onNotificationRef.current = onNotification;
  }, [onNotification]);
  useEffect(() => {
    onConnectRef.current = onConnect;
  }, [onConnect]);

  useEffect(() => {
    // Guard: do nothing if disabled or token is absent
    if (enabled === false || token === null) {
      return;
    }

    let cancelled = false;

    JsonRpcWsClient.connect({ url, token })
      .then((client) => {
        if (cancelled) {
          client.close();
          return;
        }

        clientRef.current = client;
        setState("connected");

        // Register notification fan-out
        const dispose = client.onNotification((notif) => {
          onNotificationRef.current?.(notif);
        });
        notifDisposerRef.current = dispose;

        // Invoke onConnect (fire-and-forget; errors are caller's concern)
        const result = onConnectRef.current?.(client);
        if (result && typeof result.then === "function") {
          result.then(undefined, (err: unknown) => {
            console.error("useArcJsonRpc: onConnect threw:", err);
          });
        }
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        console.error("useArcJsonRpc: connect failed:", err);
        setState("error");
      });

    return () => {
      cancelled = true;

      // Clean up notification handler first
      if (notifDisposerRef.current) {
        notifDisposerRef.current();
        notifDisposerRef.current = null;
      }

      // Close the client if it's open
      const client = clientRef.current;
      if (client) {
        client.close();
        clientRef.current = null;
      }

      setState("disconnected");
    };
    // url and token are the reconnect triggers; enabled is also a trigger
  }, [url, token, enabled]);

  const request = (method: string, params: unknown): Promise<unknown> => {
    const client = clientRef.current;
    if (!client) {
      return Promise.reject(new Error("not connected"));
    }
    return client.request(method, params);
  };

  return { state, request };
}
