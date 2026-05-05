/**
 * Cross-cutting integration test: useArcJsonRpc hook against MockWebSocket.
 *
 * Drives the full request/notification round trip without mocking the hook
 * itself — exercises the ws layer end-to-end:
 *
 *   useArcJsonRpc → JsonRpcWsClient → MockWebSocket
 *
 * Scenario:
 *   1. Render hook with mock ws URL.
 *   2. Mock subprotocol = "arc.json-rpc".
 *   3. Wait for onConnect callback to fire.
 *   4. From onConnect, fire arc.session.subscribe → mock server responds {ok:true}.
 *   5. Mock server pushes 3 arc.mirror.frame notifications (seq 1, 2, 3).
 *   6. From outside onConnect, fire arc.session.control → mock server responds {ok:true}.
 *   7. Mock server pushes arc.mirror.control_grant notification.
 *
 * Assertions verified at the end of the test.
 */

import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useArcJsonRpc } from "@/hooks/use-arc-jsonrpc";
import { SUBPROTOCOL } from "@/lib/jsonrpc-ws";
import { bytesToB64 } from "@/lib/b64";
import type { Notification } from "@/lib/jsonrpc-ws";

// ---------------------------------------------------------------------------
// MockWebSocket — self-contained, does not share with other test files
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
    const closeEvt = Object.assign(new Event("close"), {
      code: 1000,
      wasClean: true,
      reason: "",
    }) as unknown as CloseEvent;
    this.onclose?.(closeEvt);
  }

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

/** Yield once via macrotask so queued handlers run. */
function tick(): Promise<void> {
  return new Promise((r) => setTimeout(r, 0));
}

/** Sample bytes for arc.mirror.frame data payloads. */
function sampleB64(seq: number): string {
  return bytesToB64(new Uint8Array([0x1b, 0x5b, seq, 0x48]));
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
});

// ---------------------------------------------------------------------------
// Integration scenario
// ---------------------------------------------------------------------------

describe("arc-jsonrpc full flow", () => {
  it("handles request+notification round trips in order, disconnects on unmount", async () => {
    const receivedNotifs: Notification[] = [];
    const resolvedResults: unknown[] = [];

    // -----------------------------------------------------------------------
    // Step 1: Render hook
    // -----------------------------------------------------------------------
    const { result, unmount } = renderHook(() =>
      useArcJsonRpc({
        url: "ws://arc-relay.test/ws",
        token: "int-test-token",
        onConnect: async (client) => {
          // Step 4: fire arc.session.subscribe from inside onConnect
          const subResult = await client.request("arc.session.subscribe", {
            session_id: "s1",
          });
          resolvedResults.push(subResult);
        },
        onNotification: (notif) => {
          receivedNotifs.push(notif);
        },
      }),
    );

    // Initially disconnected
    expect(result.current.state).toBe("disconnected");

    // -----------------------------------------------------------------------
    // Step 2 + 3: Simulate subprotocol negotiation + open
    // -----------------------------------------------------------------------
    await act(async () => {
      await tick();
      const mock = MockWebSocket._lastInstance!;
      expect(mock).not.toBeNull();
      expect(mock.capturedProtocols).toContain(SUBPROTOCOL);

      mock.protocol = SUBPROTOCOL;
      mock.simulate("open");
      await tick(); // let hook setState("connected") settle
    });

    expect(result.current.state).toBe("connected");

    // -----------------------------------------------------------------------
    // Step 5a: Mock server responds to arc.session.subscribe (id=1)
    //          The onConnect handler is awaiting this; deliver it now.
    // -----------------------------------------------------------------------
    const mock = MockWebSocket._lastInstance!;

    await act(async () => {
      mock.simulate(
        "message",
        JSON.stringify({
          jsonrpc: "2.0",
          id: 1,
          result: { ok: true },
        }),
      );
      await tick(); // let the subscribe promise resolve + resolvedResults.push run
    });

    expect(resolvedResults).toHaveLength(1);
    expect(resolvedResults[0]).toEqual({ ok: true });

    // -----------------------------------------------------------------------
    // Step 5b: Mock server pushes 3 arc.mirror.frame notifications
    // -----------------------------------------------------------------------
    act(() => {
      for (let seq = 1; seq <= 3; seq++) {
        mock.simulate(
          "message",
          JSON.stringify({
            jsonrpc: "2.0",
            method: "arc.mirror.frame",
            params: { session_id: "s1", seq, data: sampleB64(seq) },
          }),
        );
      }
    });

    expect(receivedNotifs).toHaveLength(3);

    // -----------------------------------------------------------------------
    // Step 6 + 9: Fire arc.session.control from outside onConnect, await result
    // -----------------------------------------------------------------------
    let controlResult: Promise<unknown> | undefined;

    act(() => {
      controlResult = result.current.request("arc.session.control", {
        session_id: "s1",
        requester_id: "v1",
      });
    });

    await act(async () => {
      await tick(); // let request be sent
    });

    // Respond to arc.session.control (id=2)
    act(() => {
      mock.simulate(
        "message",
        JSON.stringify({
          jsonrpc: "2.0",
          id: 2,
          result: { ok: true },
        }),
      );
    });

    const controlRes = await controlResult!;
    expect(controlRes).toEqual({ ok: true });
    resolvedResults.push(controlRes);

    // -----------------------------------------------------------------------
    // Step 10: Mock server pushes arc.mirror.control_grant
    // -----------------------------------------------------------------------
    act(() => {
      mock.simulate(
        "message",
        JSON.stringify({
          jsonrpc: "2.0",
          method: "arc.mirror.control_grant",
          params: { session_id: "s1", controller_id: "v1" },
        }),
      );
    });

    // -----------------------------------------------------------------------
    // Assertions
    // -----------------------------------------------------------------------

    // Both requests resolved with {ok:true}
    expect(resolvedResults[0]).toEqual({ ok: true });
    expect(resolvedResults[1]).toEqual({ ok: true });

    // 4 notifications total (3 frames + 1 grant)
    expect(receivedNotifs).toHaveLength(4);

    // Frames arrived in order seq 1, 2, 3
    expect(receivedNotifs[0].method).toBe("arc.mirror.frame");
    expect((receivedNotifs[0].params as { seq: number }).seq).toBe(1);
    expect(receivedNotifs[1].method).toBe("arc.mirror.frame");
    expect((receivedNotifs[1].params as { seq: number }).seq).toBe(2);
    expect(receivedNotifs[2].method).toBe("arc.mirror.frame");
    expect((receivedNotifs[2].params as { seq: number }).seq).toBe(3);

    // 4th notification is control_grant
    expect(receivedNotifs[3].method).toBe("arc.mirror.control_grant");
    expect(receivedNotifs[3].params).toEqual({
      session_id: "s1",
      controller_id: "v1",
    });

    // Hook state still connected
    expect(result.current.state).toBe("connected");

    // -----------------------------------------------------------------------
    // Unmount → ws.close() called; cleanup effect ran (sets "disconnected")
    // React 19 suppresses post-unmount setState, so we assert via closeCalled.
    // -----------------------------------------------------------------------
    act(() => {
      unmount();
    });

    // close() was called — confirms cleanup ran
    expect(mock.closeCalled).toBe(true);
  });
});
