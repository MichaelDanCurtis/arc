/**
 * Tests for JsonRpcWsClient (JSON-RPC 2.0 over WebSocket).
 *
 * Uses a controllable in-memory MockWebSocket — no real server required.
 */

import {
  describe,
  it,
  expect,
  beforeEach,
  afterEach,
  vi,
  type MockInstance,
} from "vitest";
import {
  JsonRpcWsClient,
  JsonRpcError,
  SUBPROTOCOL,
  type JsonRpcWsConfig,
} from "@/lib/jsonrpc-ws";

// ---------------------------------------------------------------------------
// MockWebSocket
// ---------------------------------------------------------------------------

type WsEventType = "open" | "message" | "close" | "error";

interface MockMessageEvent {
  data: string;
}

class MockWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;

  readyState: number = MockWebSocket.CONNECTING;
  protocol: string = "";
  binaryType: string = "blob";

  onopen: ((e: Event) => void) | null = null;
  onmessage: ((e: MockMessageEvent) => void) | null = null;
  onclose: ((e: CloseEvent) => void) | null = null;
  onerror: ((e: Event) => void) | null = null;

  sentMessages: string[] = [];
  capturedProtocols: string[] = [];
  closeCalled = false;

  constructor(public url: string, protocols?: string | string[]) {
    if (Array.isArray(protocols)) {
      this.capturedProtocols = protocols;
    } else if (typeof protocols === "string") {
      this.capturedProtocols = [protocols];
    }
    // Store ref so tests can grab the latest instance
    MockWebSocket._lastInstance = this;
  }

  send(data: string): void {
    this.sentMessages.push(data);
  }

  close(): void {
    this.closeCalled = true;
    this.readyState = MockWebSocket.CLOSED;
  }

  /** Test helper: dispatch a lifecycle event */
  simulate(type: WsEventType, data?: string): void {
    switch (type) {
      case "open":
        this.readyState = MockWebSocket.OPEN;
        this.onopen?.(new Event("open"));
        break;
      case "message":
        this.onmessage?.({ data: data ?? "" });
        break;
      case "close": {
        this.readyState = MockWebSocket.CLOSED;
        // jsdom doesn't always export CloseEvent; cast a plain Event with the
        // close-specific fields tacked on. Sufficient for handler typing.
        const closeEvt = Object.assign(new Event("close"), {
          code: 1000,
          wasClean: true,
          reason: "",
        }) as unknown as CloseEvent;
        this.onclose?.(closeEvt);
        break;
      }
      case "error":
        this.onerror?.(new Event("error"));
        break;
    }
  }

  static _lastInstance: MockWebSocket | null = null;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Yield once via macrotask so any queued onopen/onmessage handlers run. */
function tick(): Promise<void> {
  return new Promise((r) => setTimeout(r, 0));
}

function makeConfig(overrides?: Partial<JsonRpcWsConfig>): JsonRpcWsConfig {
  return {
    url: "ws://localhost:9999/ws",
    token: "test-token",
    ...overrides,
  };
}

/** Open a client: returns both the settled client and its mock socket. */
async function openClient(
  overrides?: Partial<JsonRpcWsConfig>,
): Promise<{ client: JsonRpcWsClient; mock: MockWebSocket }> {
  const cfg = makeConfig(overrides);
  const connectPromise = JsonRpcWsClient.connect(cfg);

  await tick();
  const mock = MockWebSocket._lastInstance!;
  mock.protocol = SUBPROTOCOL;
  mock.simulate("open");

  const client = await connectPromise;
  return { client, mock };
}

// ---------------------------------------------------------------------------
// Test setup / teardown
// ---------------------------------------------------------------------------

beforeEach(() => {
  MockWebSocket._lastInstance = null;
  (global as unknown as Record<string, unknown>).WebSocket =
    MockWebSocket as unknown as typeof WebSocket;
});

afterEach(() => {
  delete (global as unknown as Record<string, unknown>).WebSocket;
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Test 1 — subprotocol negotiated
// ---------------------------------------------------------------------------

describe("subprotocol_negotiated", () => {
  it("resolves when onopen fires with correct protocol, state becomes connected", async () => {
    const cfg = makeConfig();
    const connectPromise = JsonRpcWsClient.connect(cfg);

    await tick();
    const mock = MockWebSocket._lastInstance!;

    // Verify subprotocol was requested
    expect(mock.capturedProtocols).toContain(SUBPROTOCOL);

    // Set negotiated protocol and fire open
    mock.protocol = SUBPROTOCOL;
    mock.simulate("open");

    const client = await connectPromise;
    expect(client.state).toBe("connected");
  });
});

// ---------------------------------------------------------------------------
// Test 2 — subprotocol mismatch rejects
// ---------------------------------------------------------------------------

describe("subprotocol_mismatch_rejects", () => {
  it("rejects when protocol is empty and closes the socket", async () => {
    const cfg = makeConfig();
    const connectPromise = JsonRpcWsClient.connect(cfg);

    await tick();
    const mock = MockWebSocket._lastInstance!;

    // Empty protocol — mismatch
    mock.protocol = "";
    mock.simulate("open");

    await expect(connectPromise).rejects.toThrow(/subprotocol/i);
    expect(mock.closeCalled).toBe(true);
  });

  it("rejects when protocol is a different value", async () => {
    const cfg = makeConfig();
    const connectPromise = JsonRpcWsClient.connect(cfg);

    await tick();
    const mock = MockWebSocket._lastInstance!;

    mock.protocol = "other-protocol";
    mock.simulate("open");

    await expect(connectPromise).rejects.toThrow(/subprotocol/i);
    expect(mock.closeCalled).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Test 2b — connect rejects on error before open
// ---------------------------------------------------------------------------

describe("connect_rejects_on_error_before_open", () => {
  it("rejects + closes when onerror fires before onopen", async () => {
    const cfg = makeConfig();
    const connectPromise = JsonRpcWsClient.connect(cfg);

    await tick();
    const mock = MockWebSocket._lastInstance!;

    // Simulate connection failure before open ever fires
    mock.simulate("error");

    await expect(connectPromise).rejects.toThrow(/connection failed/i);
    expect(mock.closeCalled).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Test 3 — request round-trip
// ---------------------------------------------------------------------------

describe("request_round_trip", () => {
  it("sends correct JSON-RPC frame and resolves with result", async () => {
    const { client, mock } = await openClient();

    const requestPromise = client.request("ping", { x: 1 });

    await tick();
    expect(mock.sentMessages).toHaveLength(1);

    const sent = JSON.parse(mock.sentMessages[0]) as Record<string, unknown>;
    expect(sent).toMatchObject({
      jsonrpc: "2.0",
      id: 1,
      method: "ping",
      params: { x: 1 },
    });

    // Simulate success response
    mock.simulate(
      "message",
      JSON.stringify({ jsonrpc: "2.0", id: 1, result: "pong" }),
    );

    const result = await requestPromise;
    expect(result).toBe("pong");
  });
});

// ---------------------------------------------------------------------------
// Test 4 — multiple requests have distinct ids
// ---------------------------------------------------------------------------

describe("multiple_requests_have_distinct_ids", () => {
  it("assigns ids 1 and 2, resolves each independently (out of order)", async () => {
    const { client, mock } = await openClient();

    const p1 = client.request("method.a", null);
    const p2 = client.request("method.b", null);

    await tick();
    expect(mock.sentMessages).toHaveLength(2);

    const msg1 = JSON.parse(mock.sentMessages[0]) as Record<string, unknown>;
    const msg2 = JSON.parse(mock.sentMessages[1]) as Record<string, unknown>;
    expect(msg1.id).toBe(1);
    expect(msg2.id).toBe(2);

    // Respond to id=2 first, then id=1
    mock.simulate(
      "message",
      JSON.stringify({ jsonrpc: "2.0", id: 2, result: "result-b" }),
    );
    mock.simulate(
      "message",
      JSON.stringify({ jsonrpc: "2.0", id: 1, result: "result-a" }),
    );

    expect(await p1).toBe("result-a");
    expect(await p2).toBe("result-b");
  });
});

// ---------------------------------------------------------------------------
// Test 5 — rpc error rejects
// ---------------------------------------------------------------------------

describe("rpc_error_rejects", () => {
  it("rejects with JsonRpcError carrying code and message", async () => {
    const { client, mock } = await openClient();

    const requestPromise = client.request("bad.method", null);

    await tick();

    mock.simulate(
      "message",
      JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        error: { code: -32602, message: "Invalid params" },
      }),
    );

    await expect(requestPromise).rejects.toThrow(JsonRpcError);
    await expect(requestPromise).rejects.toMatchObject({
      code: -32602,
      message: "Invalid params",
    });
  });
});

// ---------------------------------------------------------------------------
// Test 6 — notification dispatched to handler
// ---------------------------------------------------------------------------

describe("notification_dispatched_to_handler", () => {
  it("calls handler once with method and params", async () => {
    const { client, mock } = await openClient();

    const handler = vi.fn();
    client.onNotification(handler);

    mock.simulate(
      "message",
      JSON.stringify({
        jsonrpc: "2.0",
        method: "ai.chunk",
        params: { text: "hello" },
      }),
    );

    expect(handler).toHaveBeenCalledOnce();
    expect(handler).toHaveBeenCalledWith({
      method: "ai.chunk",
      params: { text: "hello" },
    });
  });
});

// ---------------------------------------------------------------------------
// Test 7 — unsubscribe stops handler
// ---------------------------------------------------------------------------

describe("unsubscribe_stops_handler", () => {
  it("does not call handler after dispose", async () => {
    const { client, mock } = await openClient();

    const handler = vi.fn();
    const dispose = client.onNotification(handler);

    // Fire one notification before unsubscribing — should be received
    mock.simulate(
      "message",
      JSON.stringify({ jsonrpc: "2.0", method: "evt.a", params: null }),
    );
    expect(handler).toHaveBeenCalledOnce();

    // Unsubscribe
    dispose();

    // Fire another notification — should NOT be received
    mock.simulate(
      "message",
      JSON.stringify({ jsonrpc: "2.0", method: "evt.b", params: null }),
    );
    expect(handler).toHaveBeenCalledOnce(); // still only 1 call
  });
});

// ---------------------------------------------------------------------------
// Test 8 — request timeout rejects
// ---------------------------------------------------------------------------

describe("request_timeout_rejects", () => {
  it("rejects after timeout with timeout error message", async () => {
    // Connect first (real timers), then switch to fake timers for the request
    const cfg = makeConfig({ requestTimeoutMs: 50 });
    const connectPromise = JsonRpcWsClient.connect(cfg);

    await tick();
    const mock = MockWebSocket._lastInstance!;
    mock.protocol = SUBPROTOCOL;
    mock.simulate("open");

    const client = await connectPromise;

    // Install fake timers AFTER connect resolves
    vi.useFakeTimers();
    try {
      const requestPromise = client.request("slow.method", null);

      // Advance past the 50ms timeout
      vi.advanceTimersByTime(51);

      await expect(requestPromise).rejects.toThrow(/timeout/i);
    } finally {
      vi.useRealTimers();
    }
  });
});

// ---------------------------------------------------------------------------
// Test 9 — close rejects pending
// ---------------------------------------------------------------------------

describe("close_rejects_pending", () => {
  it("rejects in-flight request when close() is called", async () => {
    const { client } = await openClient();

    const requestPromise = client.request("pending.method", null);

    await tick();

    client.close();

    await expect(requestPromise).rejects.toThrow(/shutdown/i);
  });
});

// ---------------------------------------------------------------------------
// Test 10 — malformed inbound frame ignored
// ---------------------------------------------------------------------------

describe("malformed_inbound_frame_ignored", () => {
  it("does not throw on non-JSON text, calls console.warn", async () => {
    const { mock } = await openClient();

    const warnSpy: MockInstance = vi.spyOn(console, "warn").mockImplementation(() => {});

    // Should not throw
    expect(() => {
      mock.simulate("message", "this is not json {{{{");
    }).not.toThrow();

    expect(warnSpy).toHaveBeenCalled();
  });
});
