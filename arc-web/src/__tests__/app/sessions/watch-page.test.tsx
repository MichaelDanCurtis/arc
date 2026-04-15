/**
 * Tests for the /sessions/[id]/watch view-only mirror page.
 *
 * Strategy: mock useWebSocket so tests can drive messages and inspect
 * what `send` was called with, without a real WebSocket.
 */
import { describe, it, expect, vi, beforeEach, type Mock } from "vitest";
import { render, screen, act, fireEvent } from "@testing-library/react";
import type { ArcToClient, ClientToArc } from "@/types/protocol";

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

// Mock next/navigation so useParams returns a stable id and useRouter works
const mockPush = vi.fn();
vi.mock("next/navigation", () => ({
  useParams: () => ({ id: "sess-42" }),
  useRouter: () => ({ push: mockPush }),
}));

// Mock XtermWrapper + writeToXterm so jsdom doesn't need a real canvas
vi.mock("@/components/terminal/xterm-wrapper", () => ({
  XtermWrapper: () => <div data-testid="xterm-wrapper" />,
  writeToXterm: vi.fn(),
}));

// Capture onMessage from useWebSocket so tests can drive inbound messages.
// Also capture send so tests can assert outbound frames.
let capturedOnMessage: ((msg: ArcToClient) => void) | null = null;
let capturedSend: Mock = vi.fn();

vi.mock("@/hooks/use-websocket", () => ({
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  useWebSocket: (_sessionId: string | null, onMessage: (msg: ArcToClient) => void, _mode: string) => {
    capturedOnMessage = onMessage;
    return { state: "connected", send: capturedSend };
  },
}));

// Import AFTER mocks are established
import WatchPage from "@/app/(protected)/sessions/[id]/watch/page";
import { writeToXterm } from "@/components/terminal/xterm-wrapper";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function dispatch(msg: ArcToClient) {
  act(() => {
    capturedOnMessage!(msg);
  });
}

const SESSION_ID = "sess-42";
const OTHER_ID = "sess-other";

// ---------------------------------------------------------------------------
// Test suite
// ---------------------------------------------------------------------------

describe("WatchPage", () => {
  beforeEach(() => {
    capturedOnMessage = null;
    capturedSend = vi.fn();
    mockPush.mockReset();
    vi.mocked(writeToXterm).mockReset();
  });

  // ── Navigation ──────────────────────────────────────────────────────────

  it("renders a Back link that navigates to /sessions", () => {
    render(<WatchPage />);
    const backBtn = screen.getByRole("button", { name: /sessions/i });
    fireEvent.click(backBtn);
    expect(mockPush).toHaveBeenCalledWith("/sessions");
  });

  // ── LeaseChanged ─────────────────────────────────────────────────────────

  it("shows driver label when LeaseChanged arrives with a driver", () => {
    render(<WatchPage />);
    dispatch({
      kind: "LeaseChanged",
      data: {
        session_id: SESSION_ID,
        driver: { user_id: "u1", device_label: "MacBook Pro", since: 1000 },
      },
    });
    expect(screen.getByText(/MacBook Pro/i)).toBeInTheDocument();
  });

  it("shows 'No driver' when LeaseChanged arrives with null driver", () => {
    render(<WatchPage />);
    dispatch({
      kind: "LeaseChanged",
      data: { session_id: SESSION_ID, driver: null },
    });
    expect(screen.getByText(/no driver/i)).toBeInTheDocument();
  });

  it("ignores LeaseChanged for a different session_id", () => {
    render(<WatchPage />);
    // First set a driver
    dispatch({
      kind: "LeaseChanged",
      data: {
        session_id: SESSION_ID,
        driver: { user_id: "u1", device_label: "MacBook Pro", since: 1000 },
      },
    });
    // Then receive a LeaseChanged for a different session — driver stays
    dispatch({
      kind: "LeaseChanged",
      data: {
        session_id: OTHER_ID,
        driver: { user_id: "u2", device_label: "ThinkPad", since: 2000 },
      },
    });
    // Still shows original driver, not ThinkPad
    expect(screen.queryByText(/ThinkPad/i)).toBeNull();
    expect(screen.getByText(/MacBook Pro/i)).toBeInTheDocument();
  });

  // ── PresenceChanged ───────────────────────────────────────────────────────

  it("renders each watcher when PresenceChanged arrives", () => {
    render(<WatchPage />);
    dispatch({
      kind: "PresenceChanged",
      data: {
        session_id: SESSION_ID,
        watchers: [
          { user_id: "u1", device_label: "Alice's Mac", can_drive: true },
          { user_id: "u2", device_label: "Bob's PC", can_drive: false },
        ],
      },
    });
    expect(screen.getByText(/Alice's Mac/i)).toBeInTheDocument();
    expect(screen.getByText(/Bob's PC/i)).toBeInTheDocument();
  });

  it("ignores PresenceChanged for a different session_id", () => {
    render(<WatchPage />);
    dispatch({
      kind: "PresenceChanged",
      data: {
        session_id: OTHER_ID,
        watchers: [
          { user_id: "u99", device_label: "Ghost", can_drive: false },
        ],
      },
    });
    expect(screen.queryByText(/Ghost/i)).toBeNull();
  });

  // ── SessionRevoked ────────────────────────────────────────────────────────

  it("shows revocation banner with the reason", () => {
    render(<WatchPage />);
    dispatch({
      kind: "SessionRevoked",
      data: { session_id: SESSION_ID, reason: "host stopped sharing" },
    });
    expect(screen.getByText(/host stopped sharing/i)).toBeInTheDocument();
  });

  it("revoked banner contains a link back to /sessions", () => {
    render(<WatchPage />);
    dispatch({
      kind: "SessionRevoked",
      data: { session_id: SESSION_ID, reason: "access revoked" },
    });
    const backLink = screen.getByRole("button", { name: /back to sessions/i });
    fireEvent.click(backLink);
    expect(mockPush).toHaveBeenCalledWith("/sessions");
  });

  it("ignores SessionRevoked for a different session_id", () => {
    render(<WatchPage />);
    dispatch({
      kind: "SessionRevoked",
      data: { session_id: OTHER_ID, reason: "oops" },
    });
    expect(screen.queryByText(/oops/i)).toBeNull();
  });

  // ── SessionDiscontinuity ──────────────────────────────────────────────────

  it("sends SubscribeSession with last seen seq on SessionDiscontinuity", () => {
    render(<WatchPage />);
    // Deliver some SessionOutput to establish last_seen_seq
    dispatch({
      kind: "SessionOutput",
      data: { session_id: SESSION_ID, bytes: new Uint8Array([0x61]), seq: 10, replay: false },
    });
    dispatch({
      kind: "SessionOutput",
      data: { session_id: SESSION_ID, bytes: new Uint8Array([0x62]), seq: 15, replay: false },
    });
    // Now discontinuity arrives
    dispatch({
      kind: "SessionDiscontinuity",
      data: { session_id: SESSION_ID, last_seq: 42 },
    });
    // send should have been called with SubscribeSession{ since_seq: 15 } (last seen)
    const calls: ClientToArc[] = capturedSend.mock.calls.map((c: ClientToArc[]) => c[0]);
    const resubscribe = calls.find(
      (c) => c.kind === "SubscribeSession",
    ) as Extract<ClientToArc, { kind: "SubscribeSession" }> | undefined;
    expect(resubscribe).toBeDefined();
    expect(resubscribe!.data.session_id).toBe(SESSION_ID);
    expect(resubscribe!.data.since_seq).toBe(15);
  });

  it("ignores SessionDiscontinuity for a different session_id", () => {
    render(<WatchPage />);
    dispatch({
      kind: "SessionDiscontinuity",
      data: { session_id: OTHER_ID, last_seq: 99 },
    });
    // Should not send anything
    expect(capturedSend).not.toHaveBeenCalled();
  });

  // ── SessionOutput ─────────────────────────────────────────────────────────

  it("writes SessionOutput bytes to xterm", () => {
    render(<WatchPage />);
    const bytes = new Uint8Array([0x68, 0x69]);
    dispatch({
      kind: "SessionOutput",
      data: { session_id: SESSION_ID, bytes, seq: 1, replay: false },
    });
    expect(writeToXterm).toHaveBeenCalled();
    const args = vi.mocked(writeToXterm).mock.calls[0];
    expect(args[1]).toBeInstanceOf(Uint8Array);
  });

  it("writes replay:true SessionOutput bytes to xterm (history)", () => {
    render(<WatchPage />);
    const bytes = new Uint8Array([0x61]);
    dispatch({
      kind: "SessionOutput",
      data: { session_id: SESSION_ID, bytes, seq: 5, replay: true },
    });
    expect(writeToXterm).toHaveBeenCalled();
  });

  it("ignores SessionOutput for a different session_id", () => {
    render(<WatchPage />);
    dispatch({
      kind: "SessionOutput",
      data: { session_id: OTHER_ID, bytes: new Uint8Array([0x61]), seq: 1, replay: false },
    });
    expect(writeToXterm).not.toHaveBeenCalled();
  });

  // ── Unmount / UnsubscribeSession ──────────────────────────────────────────

  it("sends UnsubscribeSession on unmount", () => {
    const { unmount } = render(<WatchPage />);
    unmount();
    const calls: ClientToArc[] = capturedSend.mock.calls.map((c: ClientToArc[]) => c[0]);
    const unsub = calls.find((c) => c.kind === "UnsubscribeSession") as
      | Extract<ClientToArc, { kind: "UnsubscribeSession" }>
      | undefined;
    expect(unsub).toBeDefined();
    expect(unsub!.data.session_id).toBe(SESSION_ID);
  });
});
