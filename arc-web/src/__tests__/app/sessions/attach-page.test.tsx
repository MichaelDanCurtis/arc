/**
 * Tests for the /sessions/[id] attach page — mirror-mode control events.
 *
 * Strategy: mock useWebSocket so tests can drive inbound messages and assert
 * outbound frames, without a real WebSocket or relay server.
 */
import { describe, it, expect, vi, beforeEach, type Mock } from "vitest";
import { render, screen, act, fireEvent } from "@testing-library/react";
import type { ArcToClient, ClientToArc } from "@/types/protocol";

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

const mockPush = vi.fn();
vi.mock("next/navigation", () => ({
  useParams: () => ({ id: "sess-99" }),
  useRouter: () => ({ push: mockPush }),
}));

// Mock XtermWrapper + writeToXterm — jsdom has no canvas
vi.mock("@/components/terminal/xterm-wrapper", () => ({
  XtermWrapper: () => <div data-testid="xterm-wrapper" />,
  writeToXterm: vi.fn(),
}));

// Mock heavy UI sub-components that are not under test here
vi.mock("@/components/blocks/block-list", () => ({
  BlockList: () => <div data-testid="block-list" />,
}));
vi.mock("@/components/session/shadow-diff-panel", () => ({
  ShadowDiffPanel: () => <div data-testid="shadow-diff-panel" />,
}));
vi.mock("@/components/session/control-bar", () => ({
  ControlBar: () => <div data-testid="control-bar-stub" />,
}));

// Capture onMessage and send from useWebSocket
let capturedOnMessage: ((msg: ArcToClient) => void) | null = null;
let capturedSend: Mock = vi.fn();

vi.mock("@/hooks/use-websocket", () => ({
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  useWebSocket: (_sessionId: string | null, onMessage: (msg: ArcToClient) => void) => {
    capturedOnMessage = onMessage;
    return { state: "connected", send: capturedSend };
  },
}));

// Minimal useBlocks stub — the block replay path is not under test here
vi.mock("@/hooks/use-blocks", () => ({
  useBlocks: () => ({
    blocks: [],
    replaying: false,
    replayTotal: 0,
    replayLoaded: 0,
    handleMessage: vi.fn(),
  }),
}));

// Import AFTER mocks
import AttachPage from "@/app/(protected)/sessions/[id]/page";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function dispatch(msg: ArcToClient) {
  act(() => {
    capturedOnMessage!(msg);
  });
}

const SESSION_ID = "sess-99";
const OTHER_ID = "sess-other";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("SessionAttachPage — mirror control events", () => {
  beforeEach(() => {
    capturedOnMessage = null;
    capturedSend = vi.fn();
    mockPush.mockReset();
  });

  // ── ControlRequested ───────────────────────────────────────────────────────

  it("renders a control-request banner when ControlRequested arrives", () => {
    render(<AttachPage />);
    dispatch({
      kind: "ControlRequested",
      data: {
        session_id: SESSION_ID,
        request_id: "req-1",
        from_user: "alice",
        from_device: "Alice's Mac",
      },
    });
    expect(screen.getByText(/Alice's Mac/i)).toBeInTheDocument();
    // from_user appears in a <span> by itself — check with exact text
    expect(screen.getByText("alice")).toBeInTheDocument();
  });

  it("renders multiple pending requests simultaneously", () => {
    render(<AttachPage />);
    dispatch({
      kind: "ControlRequested",
      data: {
        session_id: SESSION_ID,
        request_id: "req-1",
        from_user: "alice",
        from_device: "Alice's Mac",
      },
    });
    dispatch({
      kind: "ControlRequested",
      data: {
        session_id: SESSION_ID,
        request_id: "req-2",
        from_user: "bob",
        from_device: "Bob's PC",
      },
    });
    expect(screen.getByText(/Alice's Mac/i)).toBeInTheDocument();
    expect(screen.getByText(/Bob's PC/i)).toBeInTheDocument();
  });

  it("Grant sends ApproveControl and removes request from banner", () => {
    render(<AttachPage />);
    dispatch({
      kind: "ControlRequested",
      data: {
        session_id: SESSION_ID,
        request_id: "req-1",
        from_user: "alice",
        from_device: "Alice's Mac",
      },
    });
    const grantBtn = screen.getByRole("button", { name: /grant/i });
    fireEvent.click(grantBtn);

    // Sent correct frame
    const calls: ClientToArc[] = capturedSend.mock.calls.map((c: ClientToArc[]) => c[0]);
    const approve = calls.find(
      (c) => c.kind === "ApproveControl",
    ) as Extract<ClientToArc, { kind: "ApproveControl" }> | undefined;
    expect(approve).toBeDefined();
    expect(approve!.data.session_id).toBe(SESSION_ID);
    expect(approve!.data.request_id).toBe("req-1");

    // Request removed from UI
    expect(screen.queryByText(/Alice's Mac/i)).toBeNull();
  });

  it("Deny sends DenyControl and removes request from banner", () => {
    render(<AttachPage />);
    dispatch({
      kind: "ControlRequested",
      data: {
        session_id: SESSION_ID,
        request_id: "req-1",
        from_user: "alice",
        from_device: "Alice's Mac",
      },
    });
    const denyBtn = screen.getByRole("button", { name: /deny/i });
    fireEvent.click(denyBtn);

    const calls: ClientToArc[] = capturedSend.mock.calls.map((c: ClientToArc[]) => c[0]);
    const deny = calls.find(
      (c) => c.kind === "DenyControl",
    ) as Extract<ClientToArc, { kind: "DenyControl" }> | undefined;
    expect(deny).toBeDefined();
    expect(deny!.data.session_id).toBe(SESSION_ID);
    expect(deny!.data.request_id).toBe("req-1");

    expect(screen.queryByText(/Alice's Mac/i)).toBeNull();
  });

  it("ignores ControlRequested for a different session_id", () => {
    render(<AttachPage />);
    dispatch({
      kind: "ControlRequested",
      data: {
        session_id: OTHER_ID,
        request_id: "req-x",
        from_user: "ghost",
        from_device: "Ghost's Machine",
      },
    });
    expect(screen.queryByText(/Ghost's Machine/i)).toBeNull();
  });

  it("correctly handles Grant for one of multiple pending requests", () => {
    render(<AttachPage />);
    dispatch({
      kind: "ControlRequested",
      data: { session_id: SESSION_ID, request_id: "req-1", from_user: "alice", from_device: "Alice's Mac" },
    });
    dispatch({
      kind: "ControlRequested",
      data: { session_id: SESSION_ID, request_id: "req-2", from_user: "bob", from_device: "Bob's PC" },
    });

    // Grant only Alice's request (first Grant button in DOM)
    const grantBtns = screen.getAllByRole("button", { name: /grant/i });
    fireEvent.click(grantBtns[0]); // Alice's

    const calls: ClientToArc[] = capturedSend.mock.calls.map((c: ClientToArc[]) => c[0]);
    const approve = calls.find(
      (c) => c.kind === "ApproveControl",
    ) as Extract<ClientToArc, { kind: "ApproveControl" }> | undefined;
    expect(approve!.data.request_id).toBe("req-1");

    // Bob's request still visible
    expect(screen.getByText(/Bob's PC/i)).toBeInTheDocument();
    // Alice's gone
    expect(screen.queryByText(/Alice's Mac/i)).toBeNull();
  });

  // ── LeaseChanged ───────────────────────────────────────────────────────────

  it("shows driver indicator when LeaseChanged arrives with a non-null driver", () => {
    render(<AttachPage />);
    dispatch({
      kind: "LeaseChanged",
      data: {
        session_id: SESSION_ID,
        driver: { user_id: "alice", device_label: "Alice's Mac", since: 1000 },
      },
    });
    expect(screen.getByTestId("driver-indicator")).toBeInTheDocument();
    expect(screen.getByText(/Alice's Mac/i)).toBeInTheDocument();
  });

  it("hides driver indicator when LeaseChanged arrives with null driver", () => {
    render(<AttachPage />);
    // First set a driver
    dispatch({
      kind: "LeaseChanged",
      data: {
        session_id: SESSION_ID,
        driver: { user_id: "alice", device_label: "Alice's Mac", since: 1000 },
      },
    });
    expect(screen.getByTestId("driver-indicator")).toBeInTheDocument();

    // Clear the driver
    dispatch({
      kind: "LeaseChanged",
      data: { session_id: SESSION_ID, driver: null },
    });
    expect(screen.queryByTestId("driver-indicator")).toBeNull();
  });

  it("shows Revoke Control button when driver is non-null", () => {
    render(<AttachPage />);
    dispatch({
      kind: "LeaseChanged",
      data: {
        session_id: SESSION_ID,
        driver: { user_id: "alice", device_label: "Alice's Mac", since: 1000 },
      },
    });
    expect(screen.getByRole("button", { name: /revoke control/i })).toBeInTheDocument();
  });

  it("hides Revoke Control button when driver is null", () => {
    render(<AttachPage />);
    dispatch({
      kind: "LeaseChanged",
      data: { session_id: SESSION_ID, driver: null },
    });
    expect(screen.queryByRole("button", { name: /revoke control/i })).toBeNull();
  });

  it("clicking Revoke Control sends RevokeControl", () => {
    render(<AttachPage />);
    dispatch({
      kind: "LeaseChanged",
      data: {
        session_id: SESSION_ID,
        driver: { user_id: "alice", device_label: "Alice's Mac", since: 1000 },
      },
    });
    const revokeBtn = screen.getByRole("button", { name: /revoke control/i });
    fireEvent.click(revokeBtn);

    const calls: ClientToArc[] = capturedSend.mock.calls.map((c: ClientToArc[]) => c[0]);
    const revoke = calls.find(
      (c) => c.kind === "RevokeControl",
    ) as Extract<ClientToArc, { kind: "RevokeControl" }> | undefined;
    expect(revoke).toBeDefined();
    expect(revoke!.data.session_id).toBe(SESSION_ID);
  });

  it("ignores LeaseChanged for a different session_id", () => {
    render(<AttachPage />);
    dispatch({
      kind: "LeaseChanged",
      data: {
        session_id: OTHER_ID,
        driver: { user_id: "ghost", device_label: "Ghost's PC", since: 1000 },
      },
    });
    expect(screen.queryByTestId("driver-indicator")).toBeNull();
    expect(screen.queryByRole("button", { name: /revoke control/i })).toBeNull();
  });

  // ── PresenceChanged ────────────────────────────────────────────────────────

  it("shows watcher count when PresenceChanged arrives", () => {
    render(<AttachPage />);
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
    expect(screen.getByTestId("watcher-count")).toBeInTheDocument();
    expect(screen.getByText(/2 watching/i)).toBeInTheDocument();
  });

  it("hides watcher count when watchers list is empty", () => {
    render(<AttachPage />);
    // Initially no watchers — no badge
    expect(screen.queryByTestId("watcher-count")).toBeNull();
  });

  it("updates watcher count when PresenceChanged fires again", () => {
    render(<AttachPage />);
    dispatch({
      kind: "PresenceChanged",
      data: {
        session_id: SESSION_ID,
        watchers: [
          { user_id: "u1", device_label: "Alice's Mac", can_drive: true },
        ],
      },
    });
    expect(screen.getByText(/1 watching/i)).toBeInTheDocument();

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
    expect(screen.getByText(/2 watching/i)).toBeInTheDocument();
  });

  it("ignores PresenceChanged for a different session_id", () => {
    render(<AttachPage />);
    dispatch({
      kind: "PresenceChanged",
      data: {
        session_id: OTHER_ID,
        watchers: [
          { user_id: "u99", device_label: "Ghost", can_drive: false },
        ],
      },
    });
    expect(screen.queryByTestId("watcher-count")).toBeNull();
  });
});
