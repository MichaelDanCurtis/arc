/**
 * JsonRpcWsClient — JSON-RPC 2.0 over WebSocket (browser/jsdom).
 *
 * Parallel additive client to ws.ts (msgpack). Do NOT modify ws.ts.
 *
 * Wire protocol:
 *   subprotocol: "arc.json-rpc"  (Sec-WebSocket-Protocol header)
 *   frames: JSON text (not binary)
 *
 * Usage contract:
 *   Always obtain a client via `JsonRpcWsClient.connect()`. The static method
 *   resolves only after subprotocol negotiation succeeds. Calling `request()`
 *   before the promise resolves is not possible via this API.
 */

export const SUBPROTOCOL = "arc.json-rpc";

export type WsState = "connecting" | "connected" | "disconnected" | "error";

export interface JsonRpcWsConfig {
  /** Full ws URL, e.g. "ws://localhost:1234/ws" */
  url: string;
  /** Bearer token — appended as ?token=<...> if not already present in url */
  token: string;
  /** Per-request timeout in ms. Default 30000. */
  requestTimeoutMs?: number;
}

export interface Notification {
  method: string;
  params: unknown;
}

// ---------------------------------------------------------------------------
// Wire-level message shapes (internal)
// ---------------------------------------------------------------------------

interface RpcRequest {
  jsonrpc: "2.0";
  id: number;
  method: string;
  params: unknown;
}

interface RpcResponseSuccess {
  jsonrpc: "2.0";
  id: number;
  result: unknown;
}

interface RpcResponseError {
  jsonrpc: "2.0";
  id: number;
  error: { code: number; message: string; data?: unknown };
}

type RpcResponse = RpcResponseSuccess | RpcResponseError;

interface RpcNotification {
  jsonrpc: "2.0";
  method: string;
  params?: unknown;
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

/**
 * Append `?token=<...>` (or `&token=<...>`) to `url` if no `token` query param
 * is already present. Robust to URL-parse failures: falls back to a
 * `?token=`/`&token=` substring check so we never crash on weird inputs.
 */
function appendTokenIfMissing(url: string, token: string): string {
  let hasToken = false;
  try {
    hasToken = new URL(url).searchParams.has("token");
  } catch {
    hasToken = url.includes("?token=") || url.includes("&token=");
  }
  if (hasToken) return url;
  const sep = url.includes("?") ? "&" : "?";
  return `${url}${sep}token=${encodeURIComponent(token)}`;
}

// ---------------------------------------------------------------------------
// JsonRpcError
// ---------------------------------------------------------------------------

export class JsonRpcError extends Error {
  code: number;
  data?: unknown;

  constructor(code: number, message: string, data?: unknown) {
    super(message);
    this.name = "JsonRpcError";
    this.code = code;
    this.data = data;
  }
}

// ---------------------------------------------------------------------------
// Pending request entry
// ---------------------------------------------------------------------------

interface PendingEntry {
  resolve: (value: unknown) => void;
  reject: (reason: unknown) => void;
  timer: ReturnType<typeof setTimeout>;
}

// ---------------------------------------------------------------------------
// JsonRpcWsClient
// ---------------------------------------------------------------------------

export class JsonRpcWsClient {
  private _ws: WebSocket;
  private _state: WsState = "connecting";
  private _nextId = 1;
  private _pending = new Map<number, PendingEntry>();
  private _notifHandlers: Array<(n: Notification) => void> = [];
  private _requestTimeoutMs: number;
  private _closed = false;

  private constructor(ws: WebSocket, requestTimeoutMs: number) {
    this._ws = ws;
    this._requestTimeoutMs = requestTimeoutMs;
  }

  /**
   * Open a WebSocket connection with subprotocol negotiation.
   * Resolves once `onopen` fires and `ws.protocol === SUBPROTOCOL`.
   * Rejects if the subprotocol is not accepted, or if `onerror` fires
   * before `onopen` (connection refused / DNS / network failure).
   */
  static connect(cfg: JsonRpcWsConfig): Promise<JsonRpcWsClient> {
    const url = appendTokenIfMissing(cfg.url, cfg.token);

    const ws = new WebSocket(url, [SUBPROTOCOL]);
    const timeoutMs = cfg.requestTimeoutMs ?? 30_000;
    const client = new JsonRpcWsClient(ws, timeoutMs);

    return new Promise<JsonRpcWsClient>((resolve, reject) => {
      let settled = false;

      ws.onopen = () => {
        if (settled) return;
        if (ws.protocol !== SUBPROTOCOL) {
          settled = true;
          ws.close();
          reject(
            new Error(
              `WebSocket subprotocol mismatch: expected "${SUBPROTOCOL}", got "${ws.protocol}"`,
            ),
          );
          return;
        }
        settled = true;
        client._state = "connected";
        resolve(client);
      };

      ws.onmessage = (event: MessageEvent) => {
        if (typeof event.data !== "string") {
          console.warn("JsonRpcWsClient: ignoring non-string inbound frame");
          return;
        }
        client._handleInbound(event.data);
      };

      ws.onclose = () => {
        if (!settled) {
          settled = true;
          reject(new Error("WebSocket closed before connection established"));
          return;
        }
        client._state = "disconnected";
      };

      ws.onerror = () => {
        if (!settled) {
          settled = true;
          ws.close();
          reject(new Error("WebSocket connection failed"));
          return;
        }
        client._state = "error";
      };
    });
  }

  // --------------------------------------------------------------------------
  // Public API
  // --------------------------------------------------------------------------

  /**
   * Issue a JSON-RPC request and await the matching response.
   * Rejects immediately if the client is not connected.
   */
  request(method: string, params: unknown): Promise<unknown> {
    if (this._state !== "connected") {
      return Promise.reject(
        new Error(`JsonRpcWsClient: cannot send request in state "${this._state}"`),
      );
    }

    const id = this._nextId++;
    const frame: RpcRequest = { jsonrpc: "2.0", id, method, params };

    return new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        this._pending.delete(id);
        reject(new Error(`JsonRpcWsClient: request timeout (method="${method}", id=${id})`));
      }, this._requestTimeoutMs);

      this._pending.set(id, { resolve, reject, timer });
      this._ws.send(JSON.stringify(frame));
    });
  }

  /**
   * Subscribe to server-pushed notifications (frames with no `id`).
   * Returns a disposer function that removes this handler.
   */
  onNotification(handler: (n: Notification) => void): () => void {
    this._notifHandlers.push(handler);
    return () => {
      this._notifHandlers = this._notifHandlers.filter((h) => h !== handler);
    };
  }

  /**
   * Close the connection. Rejects all pending requests with a Shutdown error.
   * Idempotent — safe to call multiple times.
   */
  close(): void {
    if (this._closed) return;
    this._closed = true;
    // Flip state synchronously so any caller racing with the async onclose
    // event sees a non-"connected" state and request() rejects fast.
    this._state = "disconnected";

    // Reject all in-flight requests. Snapshot then clear to avoid
    // delete-during-iteration ambiguity.
    const entries = [...this._pending.values()];
    this._pending.clear();
    for (const entry of entries) {
      clearTimeout(entry.timer);
      entry.reject(new Error("JsonRpcWsClient: shutdown"));
    }

    this._ws.close();
  }

  /** Current connection state. */
  get state(): WsState {
    return this._state;
  }

  // --------------------------------------------------------------------------
  // Internal
  // --------------------------------------------------------------------------

  private _handleInbound(text: string): void {
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(text) as Record<string, unknown>;
    } catch {
      console.warn("JsonRpcWsClient: failed to parse inbound frame:", text);
      return;
    }

    // Response frame: has numeric `id` field
    if (typeof msg.id === "number") {
      const resp = msg as unknown as RpcResponse;
      const entry = this._pending.get(resp.id);
      if (!entry) {
        console.warn(
          `JsonRpcWsClient: received response for unknown id=${resp.id}`,
        );
        return;
      }
      clearTimeout(entry.timer);
      this._pending.delete(resp.id);

      if ("error" in resp) {
        const { code, message, data } = resp.error;
        entry.reject(new JsonRpcError(code, message, data));
      } else {
        entry.resolve((resp as RpcResponseSuccess).result);
      }
      return;
    }

    // Notification frame: has `method` but no `id`
    if (typeof msg.method === "string") {
      const notif = msg as unknown as RpcNotification;
      const n: Notification = {
        method: notif.method,
        params: notif.params ?? null,
      };
      for (const h of this._notifHandlers) {
        h(n);
      }
      return;
    }

    console.warn(
      "JsonRpcWsClient: inbound frame has neither id nor method, ignoring:",
      text,
    );
  }
}
