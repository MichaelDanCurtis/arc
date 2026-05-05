/**
 * Tests for the /sessions/[id]/watch view-only mirror page.
 *
 * Strategy: mock useArcJsonRpc so tests can drive notifications and inspect
 * what `request` was called with, without a real WebSocket.
 */
import { describe, it, expect, vi, beforeEach, type Mock } from "vitest";
import { render, screen, act, fireEvent } from "@testing-library/react";
import type { Notification } from "@/lib/jsonrpc-ws";

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

// Mock next/navigation so useParams returns a stable id and useRouter works
const mockPush = vi.fn();
vi.mock("next/navigation", () => ({
  useParams: () => ({ id: "sess-42" }),
  useRouter: () => ({ push: mockPush }),
}));

// Mock XtermWrapper as a forwardRef component — jsdom has no canvas.
// Expose a module-scoped write mock so tests can assert bytes were written.
// Also capture latest props for readOnly assertions.
import React from "react";
const mockXtermWrite = vi.fn();

interface MockXtermProps {
  readOnly?: boolean;
  onData?: (data: Uint8Array) => void;
  onResize?: (cols: number, rows: number) => void;
}
let mockXtermProps: MockXtermProps = {};

vi.mock("@/components/terminal/xterm-wrapper", () => ({
  XtermWrapper: React.forwardRef((props: MockXtermProps, ref: React.Ref<{ write: (data: Uint8Array | string) => void }>) => {
    React.useImperativeHandle(ref, () => ({ write: mockXtermWrite }), []);
    // Capture latest props for assertions (runs on every render)
    mockXtermProps = props;
    return <div data-testid="xterm-wrapper" />;
  }),
}));

// Mock atob/btoa for jsdom — provide a real implementation so b64ToBytes works.
// jsdom includes these, but vitest running in Node may not. Provide a safe shim.
if (typeof global.atob === "undefined") {
  global.atob = (s: string) => Buffer.from(s, "base64").toString("binary");
}
if (typeof global.btoa === "undefined") {
  global.btoa = (s: string) => Buffer.from(s, "binary").toString("base64");
}

// Capture onNotification from useArcJsonRpc so tests can drive inbound notifs.
// Also capture request so tests can assert outbound JSON-RPC calls.
let capturedOnNotification: ((notif: Notification) => void) | null = null;
let capturedRequest: Mock = vi.fn();

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
import WatchPage from "@/app/(protected)/sessions/[id]/watch/page";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Push a JSON-RPC notification into the page's onNotification handler. */
function dispatchNotif(notif: Notification) {
  act(() => {
    capturedOnNotification!(notif);
  });
}

const SESSION_ID = "sess-42";
const OTHER_ID = "sess-other";

// ---------------------------------------------------------------------------
// Test suite
// ---------------------------------------------------------------------------

describe("WatchPage", () => {
  beforeEach(() => {
    capturedOnNotification = null;
    capturedRequest = vi.fn();
    mockPush.mockReset();
    mockXtermWrite.mockReset();
    mockXtermProps = {};
  });

  // ── Navigation ──────────────────────────────────────────────────────────

  it("renders a Back link that navigates to /sessions", () => {
    render(<WatchPage />);
    const backBtn = screen.getByRole("button", { name: /sessions/i });
    fireEvent.click(backBtn);
    expect(mockPush).toHaveBeenCalledWith("/sessions");
  });

  // ── arc.mirror.control_grant ─────────────────────────────────────────────

  it("shows driver label when arc.mirror.control_grant arrives", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    expect(screen.getByText(/MacBook Pro/i)).toBeInTheDocument();
  });

  it("shows 'No driver' when arc.mirror.control_revoke arrives", () => {
    render(<WatchPage />);
    // First establish a driver
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    // Then revoke
    dispatchNotif({
      method: "arc.mirror.control_revoke",
      params: { session_id: SESSION_ID },
    });
    expect(screen.getByText(/no driver/i)).toBeInTheDocument();
  });

  it("ignores arc.mirror.control_grant for a different session_id", () => {
    render(<WatchPage />);
    // First set a driver for our session
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    // Then receive a grant for a different session — driver name stays
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: OTHER_ID,
        controller_id: "u2",
        controller_name: "ThinkPad",
      },
    });
    // Still shows original driver, not ThinkPad
    expect(screen.queryByText(/ThinkPad/i)).toBeNull();
    expect(screen.getByText(/MacBook Pro/i)).toBeInTheDocument();
  });

  // ── arc.session.participant ───────────────────────────────────────────────

  it("renders each watcher when arc.session.participant arrives", () => {
    render(<WatchPage />);
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
    expect(screen.getByText(/Alice's Mac/i)).toBeInTheDocument();
    expect(screen.getByText(/Bob's PC/i)).toBeInTheDocument();
  });

  it("ignores arc.session.participant for a different session_id", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.session.participant",
      params: {
        session_id: OTHER_ID,
        participants: [
          { user_id: "u99", user_name: "Ghost", role: "observer" },
        ],
      },
    });
    expect(screen.queryByText(/Ghost/i)).toBeNull();
  });

  // ── arc.session.revoked ────────────────────────────────────────────────────

  it("shows revocation banner with the reason", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.session.revoked",
      params: { session_id: SESSION_ID, reason: "host stopped sharing" },
    });
    expect(screen.getByText(/host stopped sharing/i)).toBeInTheDocument();
  });

  it("revoked banner contains a link back to /sessions", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.session.revoked",
      params: { session_id: SESSION_ID, reason: "access revoked" },
    });
    const backLink = screen.getByRole("button", { name: /back to sessions/i });
    fireEvent.click(backLink);
    expect(mockPush).toHaveBeenCalledWith("/sessions");
  });

  it("ignores arc.session.revoked for a different session_id", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.session.revoked",
      params: { session_id: OTHER_ID, reason: "oops" },
    });
    expect(screen.queryByText(/oops/i)).toBeNull();
  });

  // ── arc.mirror.discontinuity ──────────────────────────────────────────────

  it("sends arc.session.subscribe with last seen seq on arc.mirror.discontinuity", () => {
    render(<WatchPage />);
    // Deliver some arc.mirror.frame to establish last_seen_seq
    dispatchNotif({
      method: "arc.mirror.frame",
      params: { session_id: SESSION_ID, data: btoa("a"), seq: 10, replay: false },
    });
    dispatchNotif({
      method: "arc.mirror.frame",
      params: { session_id: SESSION_ID, data: btoa("b"), seq: 15, replay: false },
    });
    // Now discontinuity arrives
    dispatchNotif({
      method: "arc.mirror.discontinuity",
      params: { session_id: SESSION_ID },
    });
    // request should have been called with arc.session.subscribe{ since_seq: 15 } (last seen)
    const calls = capturedRequest.mock.calls as [string, Record<string, unknown>][];
    const resubscribe = calls.find(([method]) => method === "arc.session.subscribe");
    expect(resubscribe).toBeDefined();
    expect(resubscribe![1].session_id).toBe(SESSION_ID);
    expect(resubscribe![1].since_seq).toBe(15);
  });

  it("ignores arc.mirror.discontinuity for a different session_id", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.mirror.discontinuity",
      params: { session_id: OTHER_ID },
    });
    // Should not request arc.session.subscribe
    const calls = capturedRequest.mock.calls as [string, unknown][];
    const resubscribeCalls = calls.filter(([method]) => method === "arc.session.subscribe");
    expect(resubscribeCalls).toHaveLength(0);
  });

  it("resubscribes with since_seq null when discontinuity arrives before any frame", () => {
    render(<WatchPage />);
    // No arc.mirror.frame delivered — lastSeenSeqRef is still null
    dispatchNotif({
      method: "arc.mirror.discontinuity",
      params: { session_id: SESSION_ID },
    });
    const calls = capturedRequest.mock.calls as [string, Record<string, unknown>][];
    const resubscribe = calls.find(([method]) => method === "arc.session.subscribe");
    expect(resubscribe).toBeDefined();
    expect(resubscribe![1].session_id).toBe(SESSION_ID);
    expect(resubscribe![1].since_seq).toBeNull();
  });

  it("tracks max seq across out-of-order arc.mirror.frame frames", () => {
    render(<WatchPage />);
    // Deliver seq 15 first, then seq 10 (out of order)
    dispatchNotif({
      method: "arc.mirror.frame",
      params: { session_id: SESSION_ID, data: btoa("a"), seq: 15, replay: false },
    });
    dispatchNotif({
      method: "arc.mirror.frame",
      params: { session_id: SESSION_ID, data: btoa("b"), seq: 10, replay: false },
    });
    // Discontinuity should resubscribe from 15 (the max), not 10 (the most recent)
    dispatchNotif({
      method: "arc.mirror.discontinuity",
      params: { session_id: SESSION_ID },
    });
    const calls = capturedRequest.mock.calls as [string, Record<string, unknown>][];
    const resubscribe = calls.find(([method]) => method === "arc.session.subscribe");
    expect(resubscribe).toBeDefined();
    expect(resubscribe![1].since_seq).toBe(15);
  });

  // ── arc.mirror.frame ─────────────────────────────────────────────────────

  it("writes arc.mirror.frame decoded bytes to xterm", () => {
    render(<WatchPage />);
    // "hi" in base64 is "aGk="
    const bytes = new Uint8Array([0x68, 0x69]);
    const b64 = btoa(String.fromCharCode(...bytes));
    dispatchNotif({
      method: "arc.mirror.frame",
      params: { session_id: SESSION_ID, data: b64, seq: 1, replay: false },
    });
    expect(mockXtermWrite).toHaveBeenCalled();
    const args = mockXtermWrite.mock.calls[0];
    expect(args[0]).toBeInstanceOf(Uint8Array);
    expect(args[0]).toEqual(new Uint8Array([0x68, 0x69]));
  });

  it("writes replay:true arc.mirror.frame bytes to xterm (history)", () => {
    render(<WatchPage />);
    const bytes = new Uint8Array([0x61]);
    const b64 = btoa(String.fromCharCode(...bytes));
    dispatchNotif({
      method: "arc.mirror.frame",
      params: { session_id: SESSION_ID, data: b64, seq: 5, replay: true },
    });
    expect(mockXtermWrite).toHaveBeenCalled();
  });

  it("ignores arc.mirror.frame for a different session_id", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.mirror.frame",
      params: { session_id: OTHER_ID, data: btoa("a"), seq: 1, replay: false },
    });
    expect(mockXtermWrite).not.toHaveBeenCalled();
  });

  // ── Unmount / arc.session.unsubscribe ─────────────────────────────────────

  it("sends arc.session.unsubscribe on unmount", () => {
    const { unmount } = render(<WatchPage />);
    unmount();
    const calls = capturedRequest.mock.calls as [string, Record<string, unknown>][];
    const unsub = calls.find(([method]) => method === "arc.session.unsubscribe");
    expect(unsub).toBeDefined();
    expect(unsub![1].session_id).toBe(SESSION_ID);
  });

  // ── Request Control ───────────────────────────────────────────────────────

  it("hides Request Control button when driver is null", () => {
    render(<WatchPage />);
    // No arc.mirror.control_grant delivered — driver is null by default
    expect(screen.queryByRole("button", { name: /request control/i })).toBeNull();
  });

  it("shows Request Control button when driver is non-null", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    expect(screen.getByRole("button", { name: /request control/i })).toBeInTheDocument();
  });

  it("clicking Request Control sends arc.session.control request", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    const btn = screen.getByRole("button", { name: /request control/i });
    fireEvent.click(btn);

    const calls = capturedRequest.mock.calls as [string, Record<string, unknown>][];
    const req = calls.find(([method]) => method === "arc.session.control");
    expect(req).toBeDefined();
    expect(req![1].session_id).toBe(SESSION_ID);
  });

  it("enters pending state after clicking Request Control", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    const btn = screen.getByRole("button", { name: /request control/i });
    fireEvent.click(btn);

    // Button should now be disabled and show "Requesting..."
    const pendingBtn = screen.getByRole("button", { name: /requesting/i });
    expect(pendingBtn).toBeDisabled();
  });

  it("clears pending state when arc.mirror.control_grant arrives after request", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    // Click to enter pending
    fireEvent.click(screen.getByRole("button", { name: /request control/i }));
    expect(screen.getByRole("button", { name: /requesting/i })).toBeDisabled();

    // Grant arrives while pending → isDriver=true
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u2",
        controller_name: "Other Device",
      },
    });

    // Per spec: pendingRequestMine===true AND driver!==null → isDriver=true.
    expect(screen.getByText(/you are driving/i)).toBeInTheDocument();
    // Release Control button should appear
    expect(screen.getByRole("button", { name: /release control/i })).toBeInTheDocument();
  });

  it("back-to-back clicks are idempotent — only one arc.session.control sent", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    const btn = screen.getByRole("button", { name: /request control/i });
    fireEvent.click(btn);
    // Second click hits disabled button — should not fire again
    const disabledBtn = screen.getByRole("button", { name: /requesting/i });
    fireEvent.click(disabledBtn);

    const calls = capturedRequest.mock.calls as [string, unknown][];
    const reqCalls = calls.filter(([method]) => method === "arc.session.control");
    expect(reqCalls).toHaveLength(1);
  });

  // ── C3.5: isDriver / interactive mode ────────────────────────────────────

  it("shows 'You are driving' and Release Control button after control is granted", () => {
    render(<WatchPage />);
    // Set a driver so Request Control button appears
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    // Click Request Control (sets pendingRequestMine=true)
    fireEvent.click(screen.getByRole("button", { name: /request control/i }));
    // Another grant arrives while pending → grant-to-me
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    expect(screen.getByText(/you are driving/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /release control/i })).toBeInTheDocument();
    // Request Control button should NOT be present
    expect(screen.queryByRole("button", { name: /request control/i })).toBeNull();
  });

  it("renders XtermWrapper with readOnly=false when isDriver is true", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    fireEvent.click(screen.getByRole("button", { name: /request control/i }));
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    // After grant, xterm should not be read-only
    expect(mockXtermProps.readOnly).toBe(false);
  });

  it("clicking Release Control sends arc.session.release request", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    fireEvent.click(screen.getByRole("button", { name: /request control/i }));
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    // Now release control
    fireEvent.click(screen.getByRole("button", { name: /release control/i }));

    const calls = capturedRequest.mock.calls as [string, Record<string, unknown>][];
    const revoke = calls.find(([method]) => method === "arc.session.release");
    expect(revoke).toBeDefined();
    expect(revoke![1].session_id).toBe(SESSION_ID);
  });

  it("clicking Release Control optimistically clears isDriver", () => {
    render(<WatchPage />);
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    fireEvent.click(screen.getByRole("button", { name: /request control/i }));
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    // Now release
    fireEvent.click(screen.getByRole("button", { name: /release control/i }));
    // "You are driving" should disappear
    expect(screen.queryByText(/you are driving/i)).toBeNull();
    // Release Control button should disappear (driver still non-null, so Request Control appears)
    expect(screen.queryByRole("button", { name: /release control/i })).toBeNull();
  });

  it("arc.mirror.control_grant without pending request does NOT flip to driver mode", () => {
    render(<WatchPage />);
    // Receive arc.mirror.control_grant WITHOUT having clicked Request Control
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    // isDriver should remain false — "You are driving" should NOT appear
    expect(screen.queryByText(/you are driving/i)).toBeNull();
    // Request Control should be visible (not in driver mode)
    expect(screen.getByRole("button", { name: /request control/i })).toBeInTheDocument();
    // xterm should be read-only
    expect(mockXtermProps.readOnly).toBe(true);
  });

  it("arc.session.revoked resets isDriver to false", () => {
    render(<WatchPage />);
    // Grant control
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    fireEvent.click(screen.getByRole("button", { name: /request control/i }));
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    expect(screen.getByText(/you are driving/i)).toBeInTheDocument();

    // Session revoked
    dispatchNotif({
      method: "arc.session.revoked",
      params: { session_id: SESSION_ID, reason: "host closed session" },
    });
    // Should no longer be in driver mode
    expect(screen.queryByText(/you are driving/i)).toBeNull();
  });

  it("arc.mirror.control_revoke while isDriver resets to view-only", () => {
    render(<WatchPage />);
    // Grant control
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    fireEvent.click(screen.getByRole("button", { name: /request control/i }));
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    expect(screen.getByText(/you are driving/i)).toBeInTheDocument();

    // Relay revokes lease
    dispatchNotif({
      method: "arc.mirror.control_revoke",
      params: { session_id: SESSION_ID },
    });
    expect(screen.queryByText(/you are driving/i)).toBeNull();
    expect(mockXtermProps.readOnly).toBe(true);
  });

  it("XtermWrapper receives readOnly=true in default view-only state", () => {
    render(<WatchPage />);
    expect(mockXtermProps.readOnly).toBe(true);
  });

  // ── Finding (1): arc.mirror.control_revoke while pending must NOT grant isDriver ──

  it("arc.mirror.control_revoke while pendingRequestMine does NOT set isDriver", () => {
    render(<WatchPage />);
    // Set a driver so Request Control button appears
    dispatchNotif({
      method: "arc.mirror.control_grant",
      params: {
        session_id: SESSION_ID,
        controller_id: "u1",
        controller_name: "MacBook Pro",
      },
    });
    // Click Request Control — sets pendingRequestMine=true
    fireEvent.click(screen.getByRole("button", { name: /request control/i }));
    // Relay denies by sending revoke while we are pending
    dispatchNotif({
      method: "arc.mirror.control_revoke",
      params: { session_id: SESSION_ID },
    });
    // isDriver must NOT become true — xterm stays read-only
    expect(mockXtermProps.readOnly).toBe(true);
    // "Release Control" must not appear
    expect(screen.queryByRole("button", { name: /release control/i })).toBeNull();
  });
});
