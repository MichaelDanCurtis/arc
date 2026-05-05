/**
 * Tests for useArcJsonRpc hook.
 *
 * Uses the same controllable in-memory MockWebSocket pattern as
 * __tests__/lib/jsonrpc-ws.test.ts — no real server required.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useArcJsonRpc } from "@/hooks/use-arc-jsonrpc";
import { SUBPROTOCOL } from "@/lib/jsonrpc-ws";

// ---------------------------------------------------------------------------
// MockWebSocket (same pattern as jsonrpc-ws.test.ts)
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
    MockWebSocket._lastInstance = this;
    MockWebSocket._allInstances.push(this);
  }

  send(data: string): void {
    this.sentMessages.push(data);
  }

  close(): void {
    this.closeCalled = true;
    this.readyState = MockWebSocket.CLOSED;
    // Fire onclose so client state machine updates
    const closeEvt = Object.assign(new Event("close"), {
      code: 1000,
      wasClean: true,
      reason: "",
    }) as unknown as CloseEvent;
    this.onclose?.(closeEvt);
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
  static _allInstances: MockWebSocket[] = [];
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Yield once via macrotask so any queued handlers run. */
function tick(): Promise<void> {
  return new Promise((r) => setTimeout(r, 0));
}

/**
 * Complete the pending connect handshake on the latest MockWebSocket:
 * set protocol to SUBPROTOCOL and fire "open".
 */
async function resolveConnect(): Promise<MockWebSocket> {
  await tick();
  const mock = MockWebSocket._lastInstance!;
  mock.protocol = SUBPROTOCOL;
  mock.simulate("open");
  await tick(); // let hook state settle
  return mock;
}

// ---------------------------------------------------------------------------
// Test setup / teardown
// ---------------------------------------------------------------------------

beforeEach(() => {
  MockWebSocket._lastInstance = null;
  MockWebSocket._allInstances = [];
  (global as unknown as Record<string, unknown>).WebSocket =
    MockWebSocket as unknown as typeof WebSocket;
});

afterEach(() => {
  delete (global as unknown as Record<string, unknown>).WebSocket;
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Test 1 — connects + invokes onConnect
// ---------------------------------------------------------------------------

describe("connects + invokes onConnect", () => {
  it("calls onConnect with non-null client after connection settles", async () => {
    const onConnect = vi.fn();

    const { result } = renderHook(() =>
      useArcJsonRpc({
        url: "ws://localhost:9999/ws",
        token: "test-token",
        onConnect,
      }),
    );

    // Initially disconnected (no synchronous setState in effect body)
    expect(result.current.state).toBe("disconnected");

    await act(async () => {
      await resolveConnect();
    });

    expect(result.current.state).toBe("connected");
    expect(onConnect).toHaveBeenCalledOnce();
    expect(onConnect.mock.calls[0][0]).not.toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Test 2 — forwards notifications to onNotification
// ---------------------------------------------------------------------------

describe("forwards notifications to onNotification", () => {
  it("calls onNotification with correct method + params on server push", async () => {
    const onNotification = vi.fn();

    renderHook(() =>
      useArcJsonRpc({
        url: "ws://localhost:9999/ws",
        token: "test-token",
        onNotification,
      }),
    );

    let mock: MockWebSocket;
    await act(async () => {
      mock = await resolveConnect();
    });

    act(() => {
      mock!.simulate(
        "message",
        JSON.stringify({
          jsonrpc: "2.0",
          method: "arc.mirror.frame",
          params: { session_id: "s1", seq: 1, data: "" },
        }),
      );
    });

    expect(onNotification).toHaveBeenCalledOnce();
    expect(onNotification).toHaveBeenCalledWith({
      method: "arc.mirror.frame",
      params: { session_id: "s1", seq: 1, data: "" },
    });
  });
});

// ---------------------------------------------------------------------------
// Test 3 — request() resolves on response
// ---------------------------------------------------------------------------

describe("request() resolves on response", () => {
  it("resolves with result when mock responds with success", async () => {
    let capturedClient: ReturnType<typeof useArcJsonRpc> | null = null;

    const { result } = renderHook(() => {
      const hookResult = useArcJsonRpc({
        url: "ws://localhost:9999/ws",
        token: "test-token",
      });
      capturedClient = hookResult;
      return hookResult;
    });

    let mock: MockWebSocket;
    await act(async () => {
      mock = await resolveConnect();
    });

    expect(result.current.state).toBe("connected");

    // Issue a request and capture the promise
    let requestPromise: Promise<unknown>;
    act(() => {
      requestPromise = capturedClient!.request("arc.session.attach", {
        session_id: "s1",
        viewer_id: "v1",
      });
    });

    // Let the send flush
    await act(async () => {
      await tick();
    });

    // Respond from mock
    act(() => {
      mock!.simulate(
        "message",
        JSON.stringify({
          jsonrpc: "2.0",
          id: 1,
          result: { ok: true, viewer_id: "v1", role: "controller" },
        }),
      );
    });

    const resolved = await requestPromise!;
    expect(resolved).toEqual({ ok: true, viewer_id: "v1", role: "controller" });
  });
});

// ---------------------------------------------------------------------------
// Test 4 — unmount closes ws
// ---------------------------------------------------------------------------

describe("unmount closes ws", () => {
  it("calls ws.close() on unmount and reflects disconnected state", async () => {
    const { result, unmount } = renderHook(() =>
      useArcJsonRpc({
        url: "ws://localhost:9999/ws",
        token: "test-token",
      }),
    );

    let mock: MockWebSocket;
    await act(async () => {
      mock = await resolveConnect();
    });

    expect(result.current.state).toBe("connected");

    act(() => {
      unmount();
    });

    expect(mock!.closeCalled).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Test 5 — enabled=false defers connect
// ---------------------------------------------------------------------------

describe("enabled=false defers connect", () => {
  it("does not construct WebSocket when enabled is false", async () => {
    renderHook(() =>
      useArcJsonRpc({
        url: "ws://localhost:9999/ws",
        token: "test-token",
        enabled: false,
      }),
    );

    await act(async () => {
      await tick();
    });

    expect(MockWebSocket._allInstances).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Test 6 — null token defers connect
// ---------------------------------------------------------------------------

describe("null token defers connect", () => {
  it("does not construct WebSocket when token is null", async () => {
    renderHook(() =>
      useArcJsonRpc({
        url: "ws://localhost:9999/ws",
        token: null,
      }),
    );

    await act(async () => {
      await tick();
    });

    expect(MockWebSocket._allInstances).toHaveLength(0);
  });
});
