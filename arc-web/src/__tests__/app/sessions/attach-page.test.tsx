/**
 * Tests for the /sessions/[id] attach page — JSON-RPC wire protocol.
 *
 * Strategy: mock useArcJsonRpc so tests can drive inbound notifications and
 * assert outbound JSON-RPC requests, without a real WebSocket or relay server.
 */
import { describe, it, expect, vi, beforeEach, type Mock } from "vitest";
import { render, screen, act, fireEvent } from "@testing-library/react";
import type { Notification } from "@/lib/jsonrpc-ws";

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

const mockPush = vi.fn();
vi.mock("next/navigation", () => ({
  useParams: () => ({ id: "sess-99" }),
  useRouter: () => ({ push: mockPush }),
}));

// Mock XtermWrapper as a forwardRef component — jsdom has no canvas.
// Expose a module-scoped write mock so tests can assert bytes were written.
import React from "react";
const mockXtermWrite = vi.fn();
vi.mock("@/components/terminal/xterm-wrapper", () => ({
  XtermWrapper: React.forwardRef((_props: unknown, ref: React.Ref<{ write: (data: Uint8Array | string) => void }>) => {
    React.useImperativeHandle(ref, () => ({ write: mockXtermWrite }), []);
    return <div data-testid="xterm-wrapper" />;
  }),
}));

// Mock heavy UI sub-components that are not under test here
vi.mock("@/components/blocks/block-list", () => ({
  BlockList: () => <div data-testid="block-list" />,
}));
vi.mock("@/components/session/shadow-diff-panel", () => ({
  ShadowDiffPanel: () => <div data-testid="shadow-diff-panel" />,
}));

// Mock atob/btoa for jsdom — provide a real implementation so b64ToBytes works.
if (typeof global.atob === "undefined") {
  global.atob = (s: string) => Buffer.from(s, "base64").toString("binary");
}
if (typeof global.btoa === "undefined") {
  global.btoa = (s: string) => Buffer.from(s, "binary").toString("base64");
}

// Capture onNotification and request from useArcJsonRpc.
let capturedOnNotification: ((notif: Notification) => void) | null = null;
let capturedRequest: Mock = vi.fn(() => Promise.resolve(null));

vi.mock("@/hooks/use-arc-jsonrpc", () => ({
  useArcJsonRpc: (options: {
    onNotification?: (notif: Notification) => void;
    onConnect?: (client: unknown) => void | Promise<void>;
    enabled?: boolean;
    url?: string;
    token?: string | null;
  }) => {
    capturedOnNotification = options.onNotification ?? null;
    return { state: "connected", request: capturedRequest };
  },
}));

// Mock getAccessToken so the page can construct the hook without localStorage.
vi.mock("@/lib/auth", () => ({
  getAccessToken: () => "test-token",
}));

// Mock getWsUrl so the page gets a deterministic URL.
vi.mock("@/lib/config", () => ({
  getWsUrl: () => "ws://localhost:9999/ws",
}));

// Import AFTER mocks are established
import AttachPage from "@/app/(protected)/sessions/[id]/page";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Push a JSON-RPC notification into the page's onNotification handler. */
function dispatchNotif(notif: Notification) {
  act(() => {
    capturedOnNotification!(notif);
  });
}

const SESSION_ID = "sess-99";
const OTHER_ID = "sess-other";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("SessionAttachPage — JSON-RPC mirror control events", () => {
  beforeEach(() => {
    capturedOnNotification = null;
    capturedRequest = vi.fn(() => Promise.resolve(null));
    mockPush.mockReset();
    mockXtermWrite.mockReset();
  });

  // ── arc.session.control_requested ────────────────────────────────────────

  it("renders a control-request banner when arc.session.control_requested arrives", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.session.control_requested",
      params: {
        session_id: SESSION_ID,
        request_id: "req-1",
        requester_id: "alice",
        requester_name: "Alice's Mac",
      },
    });
    expect(screen.getByText(/Alice's Mac/i)).toBeInTheDocument();
    expect(screen.getByText("alice")).toBeInTheDocument();
  });

  it("renders multiple pending requests simultaneously", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.session.control_requested",
      params: {
        session_id: SESSION_ID,
        request_id: "req-1",
        requester_id: "alice",
        requester_name: "Alice's Mac",
      },
    });
    dispatchNotif({
      method: "arc.session.control_requested",
      params: {
        session_id: SESSION_ID,
        request_id: "req-2",
        requester_id: "bob",
        requester_name: "Bob's PC",
      },
    });
    expect(screen.getByText(/Alice's Mac/i)).toBeInTheDocument();
    expect(screen.getByText(/Bob's PC/i)).toBeInTheDocument();
  });

  it("Grant sends arc.session.approve_control and removes request from banner", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.session.control_requested",
      params: {
        session_id: SESSION_ID,
        request_id: "req-1",
        requester_id: "alice",
        requester_name: "Alice's Mac",
      },
    });
    const grantBtn = screen.getByRole("button", { name: /grant/i });
    fireEvent.click(grantBtn);

    const calls = capturedRequest.mock.calls as [string, Record<string, unknown>][];
    const approve = calls.find(([method]) => method === "arc.session.approve_control");
    expect(approve).toBeDefined();
    expect(approve![1].session_id).toBe(SESSION_ID);
    expect(approve![1].request_id).toBe("req-1");
    expect(approve![1].requester_id).toBe("alice");

    // Request removed from UI
    expect(screen.queryByText(/Alice's Mac/i)).toBeNull();
  });

  it("Deny sends arc.session.deny_control and removes request from banner", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.session.control_requested",
      params: {
        session_id: SESSION_ID,
        request_id: "req-1",
        requester_id: "alice",
        requester_name: "Alice's Mac",
      },
    });
    const denyBtn = screen.getByRole("button", { name: /deny/i });
    fireEvent.click(denyBtn);

    const calls = capturedRequest.mock.calls as [string, Record<string, unknown>][];
    const deny = calls.find(([method]) => method === "arc.session.deny_control");
    expect(deny).toBeDefined();
    expect(deny![1].session_id).toBe(SESSION_ID);
    expect(deny![1].request_id).toBe("req-1");
    expect(deny![1].requester_id).toBe("alice");

    expect(screen.queryByText(/Alice's Mac/i)).toBeNull();
  });

  it("ignores arc.session.control_requested for a different session_id", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.session.control_requested",
      params: {
        session_id: OTHER_ID,
        request_id: "req-x",
        requester_id: "ghost",
        requester_name: "Ghost's Machine",
      },
    });
    expect(screen.queryByText(/Ghost's Machine/i)).toBeNull();
  });

  it("dedupes arc.session.control_requested by request_id", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.session.control_requested",
      params: {
        session_id: SESSION_ID,
        request_id: "req-dup",
        requester_id: "alice",
        requester_name: "Alice's Mac",
      },
    });
    dispatchNotif({
      method: "arc.session.control_requested",
      params: {
        session_id: SESSION_ID,
        request_id: "req-dup",
        requester_id: "alice-2",
        requester_name: "Alice's Other Mac",
      },
    });
    const banners = screen.getAllByTestId("control-request-banner");
    expect(banners.length).toBe(1);
  });

  it("correctly handles Grant for one of multiple pending requests", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.session.control_requested",
      params: { session_id: SESSION_ID, request_id: "req-1", requester_id: "alice", requester_name: "Alice's Mac" },
    });
    dispatchNotif({
      method: "arc.session.control_requested",
      params: { session_id: SESSION_ID, request_id: "req-2", requester_id: "bob", requester_name: "Bob's PC" },
    });

    // Grant only Alice's request (first Grant button in DOM)
    const grantBtns = screen.getAllByRole("button", { name: /grant/i });
    fireEvent.click(grantBtns[0]); // Alice's

    const calls = capturedRequest.mock.calls as [string, Record<string, unknown>][];
    const approve = calls.find(([method]) => method === "arc.session.approve_control");
    expect(approve![1].request_id).toBe("req-1");

    // Bob's request still visible
    expect(screen.getByText(/Bob's PC/i)).toBeInTheDocument();
    // Alice's gone
    expect(screen.queryByText(/Alice's Mac/i)).toBeNull();
  });

  // ── arc.mirror.control_grant ──────────────────────────────────────────────

  it("shows driver indicator when arc.mirror.control_grant arrives", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "alice",
        controller_name: "Alice's Mac",
      },
    });
    expect(screen.getByTestId("driver-indicator")).toBeInTheDocument();
    expect(screen.getByText(/Alice's Mac/i)).toBeInTheDocument();
  });

  it("hides driver indicator when arc.mirror.control_revoke arrives with null driver", () => {
    render(<AttachPage />);
    // First set a driver
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "alice",
        controller_name: "Alice's Mac",
      },
    });
    expect(screen.getByTestId("driver-indicator")).toBeInTheDocument();

    // Revoke the driver
    dispatchNotif({
      method: "arc.mirror.control_revoke",
      params: { session_id: SESSION_ID },
    });
    expect(screen.queryByTestId("driver-indicator")).toBeNull();
  });

  it("shows Revoke Control button when driver is non-null", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "alice",
        controller_name: "Alice's Mac",
      },
    });
    expect(screen.getByRole("button", { name: /revoke control/i })).toBeInTheDocument();
  });

  it("hides Revoke Control button when no driver", () => {
    render(<AttachPage />);
    expect(screen.queryByRole("button", { name: /revoke control/i })).toBeNull();
  });

  it("clicking Revoke Control sends arc.session.release", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "alice",
        controller_name: "Alice's Mac",
      },
    });
    const revokeBtn = screen.getByRole("button", { name: /revoke control/i });
    fireEvent.click(revokeBtn);

    const calls = capturedRequest.mock.calls as [string, Record<string, unknown>][];
    const revoke = calls.find(([method]) => method === "arc.session.release");
    expect(revoke).toBeDefined();
    expect(revoke![1].session_id).toBe(SESSION_ID);
  });

  it("ignores arc.mirror.control_grant for a different session_id", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: OTHER_ID,
        controller_id: "ghost",
        controller_name: "Ghost's PC",
      },
    });
    expect(screen.queryByTestId("driver-indicator")).toBeNull();
    expect(screen.queryByRole("button", { name: /revoke control/i })).toBeNull();
  });

  // ── arc.session.participant ───────────────────────────────────────────────

  it("shows watcher count when arc.session.participant arrives", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.session.participant",
      params: {
        session_id: SESSION_ID,
        participants: [
          { user_id: "u1", user_name: "Alice's Mac", role: "observer" },
          { user_id: "u2", user_name: "Bob's PC", role: "observer" },
        ],
      },
    });
    expect(screen.getByTestId("watcher-count")).toBeInTheDocument();
    expect(screen.getByText(/2 watching/i)).toBeInTheDocument();
  });

  it("hides watcher count when watchers list is empty", () => {
    render(<AttachPage />);
    expect(screen.queryByTestId("watcher-count")).toBeNull();
  });

  it("updates watcher count when arc.session.participant fires again", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.session.participant",
      params: {
        session_id: SESSION_ID,
        participants: [
          { user_id: "u1", user_name: "Alice's Mac", role: "observer" },
        ],
      },
    });
    expect(screen.getByText(/1 watching/i)).toBeInTheDocument();

    dispatchNotif({
      method: "arc.session.participant",
      params: {
        session_id: SESSION_ID,
        participants: [
          { user_id: "u1", user_name: "Alice's Mac", role: "observer" },
          { user_id: "u2", user_name: "Bob's PC", role: "observer" },
        ],
      },
    });
    expect(screen.getByText(/2 watching/i)).toBeInTheDocument();
  });

  it("ignores arc.session.participant for a different session_id", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.session.participant",
      params: {
        session_id: OTHER_ID,
        participants: [
          { user_id: "u99", user_name: "Ghost", role: "observer" },
        ],
      },
    });
    expect(screen.queryByTestId("watcher-count")).toBeNull();
  });

  // ── arc.session.gone ──────────────────────────────────────────────────────

  it("redirects to /sessions when arc.session.gone arrives", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.session.gone",
      params: { session_id: SESSION_ID },
    });
    expect(mockPush).toHaveBeenCalledWith("/sessions");
  });

  it("ignores arc.session.gone for a different session_id", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.session.gone",
      params: { session_id: OTHER_ID },
    });
    expect(mockPush).not.toHaveBeenCalled();
  });

  // ── arc.mirror.block_chunk → xterm write ──────────────────────────────────

  it("writes arc.mirror.block_chunk decoded bytes to xterm", () => {
    render(<AttachPage />);
    const bytes = new Uint8Array([0x68, 0x69]); // "hi"
    const b64 = btoa(String.fromCharCode(...bytes));
    dispatchNotif({
      method: "arc.mirror.block_chunk",
      params: {
        session_id: SESSION_ID,
        block_id: "blk-1",
        seq: 0,
        data: b64,
      },
    });
    expect(mockXtermWrite).toHaveBeenCalled();
    const arg = mockXtermWrite.mock.calls[0][0];
    expect(arg).toBeInstanceOf(Uint8Array);
    expect(arg).toEqual(new Uint8Array([0x68, 0x69]));
  });

  it("ignores arc.mirror.block_chunk for a different session_id", () => {
    render(<AttachPage />);
    dispatchNotif({
      method: "arc.mirror.block_chunk",
      params: {
        session_id: OTHER_ID,
        block_id: "blk-x",
        seq: 0,
        data: btoa("a"),
      },
    });
    expect(mockXtermWrite).not.toHaveBeenCalled();
  });

  // ── Unmount: arc.session.detach ───────────────────────────────────────────

  it("sends arc.session.detach on unmount", () => {
    const { unmount } = render(<AttachPage />);
    unmount();
    const calls = capturedRequest.mock.calls as [string, Record<string, unknown>][];
    const detach = calls.find(([method]) => method === "arc.session.detach");
    expect(detach).toBeDefined();
    expect(detach![1].session_id).toBe(SESSION_ID);
  });
});
