import { describe, it, expect } from "vitest";
import { encode as msgpackEncode, decode as msgpackDecode } from "@msgpack/msgpack";
import { encodeMessage, decodeMessage } from "@/lib/ws";
import type { ClientToArc, ArcToClient, DriverInfo, WatcherInfo } from "@/types/protocol";

describe("WebSocket codec", () => {
  it("encodes AttachSession to MessagePack", () => {
    const msg: ClientToArc = {
      kind: "AttachSession",
      data: { session_id: "sess-1", since_block: null },
    };
    const bytes = encodeMessage(msg);
    expect(bytes).toBeInstanceOf(Uint8Array);

    // Decode and verify structure
    const decoded = msgpackDecode(bytes) as ClientToArc;
    expect(decoded.kind).toBe("AttachSession");
    expect(decoded.data).toEqual({ session_id: "sess-1", since_block: null });
  });

  it("encodes SessionInput with bytes", () => {
    const msg: ClientToArc = {
      kind: "SessionInput",
      data: { session_id: "sess-1", bytes: new Uint8Array([0x68, 0x69]) },
    };
    const bytes = encodeMessage(msg);
    const decoded = msgpackDecode(bytes) as ClientToArc;
    expect(decoded.kind).toBe("SessionInput");
  });

  it("decodes AttachAck from MessagePack", () => {
    const original: ArcToClient = {
      kind: "AttachAck",
      data: {
        session_id: "sess-1",
        current_cols: 120,
        current_rows: 40,
        resume_from: null,
      },
    };
    const bytes = msgpackEncode(original);
    const decoded = decodeMessage(new Uint8Array(bytes));
    expect(decoded.kind).toBe("AttachAck");
    if (decoded.kind === "AttachAck") {
      expect(decoded.data.current_cols).toBe(120);
      expect(decoded.data.current_rows).toBe(40);
    }
  });

  it("decodes ReplayBlock with Block data", () => {
    const original: ArcToClient = {
      kind: "ReplayBlock",
      data: {
        session_id: "sess-1",
        block: {
          header: {
            id: "blk-1",
            session_id: "sess-1",
            tab_id: "tab-1",
            command: "ls -la",
            cwd: "/home/user",
            started_at: Date.now(),
            env_snapshot: {},
            host_info: {
              hostname: "dev-machine",
              os: "linux",
              arch: "x86_64",
              beacon_version: "0.1.0",
            },
          },
          output: new Uint8Array([0x68, 0x65, 0x6c, 0x6c, 0x6f]),
          footer: { id: "blk-1", exit_code: 0, ended_at: Date.now(), shadow_git_ref: null },
        },
      },
    };
    const bytes = msgpackEncode(original);
    const decoded = decodeMessage(new Uint8Array(bytes));
    expect(decoded.kind).toBe("ReplayBlock");
    if (decoded.kind === "ReplayBlock") {
      expect(decoded.data.block.header.command).toBe("ls -la");
      expect(decoded.data.block.footer?.exit_code).toBe(0);
    }
  });

  it("encodes Pong", () => {
    const msg: ClientToArc = { kind: "Pong", data: null };
    const bytes = encodeMessage(msg);
    const decoded = msgpackDecode(bytes) as ClientToArc;
    expect(decoded.kind).toBe("Pong");
  });
});

describe("Mirror envelope codec", () => {
  // --- ClientToArc mirror variants ---

  it("round-trips SubscribeSession (since_seq: null)", () => {
    const msg: ClientToArc = {
      kind: "SubscribeSession",
      data: { session_id: "sess-42", since_seq: null },
    };
    const bytes = encodeMessage(msg);
    const decoded = msgpackDecode(bytes) as ClientToArc;
    expect(decoded.kind).toBe("SubscribeSession");
    if (decoded.kind === "SubscribeSession") {
      expect(decoded.data.session_id).toBe("sess-42");
      expect(decoded.data.since_seq).toBeNull();
    }
  });

  it("round-trips SubscribeSession (since_seq: non-null)", () => {
    const msg: ClientToArc = {
      kind: "SubscribeSession",
      data: { session_id: "sess-99", since_seq: 1234 },
    };
    const bytes = encodeMessage(msg);
    const decoded = msgpackDecode(bytes) as ClientToArc;
    expect(decoded.kind).toBe("SubscribeSession");
    if (decoded.kind === "SubscribeSession") {
      expect(decoded.data.since_seq).toBe(1234);
    }
  });

  it("round-trips UnsubscribeSession", () => {
    const msg: ClientToArc = {
      kind: "UnsubscribeSession",
      data: { session_id: "sess-1" },
    };
    const bytes = encodeMessage(msg);
    const decoded = msgpackDecode(bytes) as ClientToArc;
    expect(decoded.kind).toBe("UnsubscribeSession");
  });

  it("round-trips RequestControl", () => {
    const msg: ClientToArc = {
      kind: "RequestControl",
      data: { session_id: "sess-1" },
    };
    const bytes = encodeMessage(msg);
    const decoded = msgpackDecode(bytes) as ClientToArc;
    expect(decoded.kind).toBe("RequestControl");
  });

  it("round-trips ApproveControl", () => {
    const msg: ClientToArc = {
      kind: "ApproveControl",
      data: { session_id: "sess-1", request_id: "req-abc" },
    };
    const bytes = encodeMessage(msg);
    const decoded = msgpackDecode(bytes) as ClientToArc;
    expect(decoded.kind).toBe("ApproveControl");
    if (decoded.kind === "ApproveControl") {
      expect(decoded.data.request_id).toBe("req-abc");
    }
  });

  it("round-trips DenyControl", () => {
    const msg: ClientToArc = {
      kind: "DenyControl",
      data: { session_id: "sess-1", request_id: "req-abc" },
    };
    const bytes = encodeMessage(msg);
    const decoded = msgpackDecode(bytes) as ClientToArc;
    expect(decoded.kind).toBe("DenyControl");
  });

  it("round-trips RevokeControl", () => {
    const msg: ClientToArc = {
      kind: "RevokeControl",
      data: { session_id: "sess-1" },
    };
    const bytes = encodeMessage(msg);
    const decoded = msgpackDecode(bytes) as ClientToArc;
    expect(decoded.kind).toBe("RevokeControl");
  });

  it("round-trips PushOutput", () => {
    const msg: ClientToArc = {
      kind: "PushOutput",
      data: { session_id: "sess-1", bytes: new Uint8Array([0x61, 0x62]), seq: 7 },
    };
    const bytes = encodeMessage(msg);
    const decoded = msgpackDecode(bytes) as ClientToArc;
    expect(decoded.kind).toBe("PushOutput");
    if (decoded.kind === "PushOutput") {
      expect(decoded.data.seq).toBe(7);
    }
  });

  it("round-trips OutputDiscontinuity", () => {
    const msg: ClientToArc = {
      kind: "OutputDiscontinuity",
      data: { session_id: "sess-1", last_seq: 999 },
    };
    const bytes = encodeMessage(msg);
    const decoded = msgpackDecode(bytes) as ClientToArc;
    expect(decoded.kind).toBe("OutputDiscontinuity");
    if (decoded.kind === "OutputDiscontinuity") {
      expect(decoded.data.last_seq).toBe(999);
    }
  });

  // --- ArcToClient mirror variants ---

  it("round-trips SessionOutput (replay: false)", () => {
    const original: ArcToClient = {
      kind: "SessionOutput",
      data: {
        session_id: "sess-1",
        bytes: new Uint8Array([0x68, 0x65, 0x6c, 0x6c, 0x6f]),
        seq: 42,
        replay: false,
      },
    };
    const bytes = msgpackEncode(original);
    const decoded = decodeMessage(new Uint8Array(bytes));
    expect(decoded.kind).toBe("SessionOutput");
    if (decoded.kind === "SessionOutput") {
      expect(decoded.data.seq).toBe(42);
      expect(decoded.data.replay).toBe(false);
    }
  });

  it("round-trips SessionOutput (replay: true)", () => {
    const original: ArcToClient = {
      kind: "SessionOutput",
      data: {
        session_id: "sess-1",
        bytes: new Uint8Array([0x77]),
        seq: 1,
        replay: true,
      },
    };
    const bytes = msgpackEncode(original);
    const decoded = decodeMessage(new Uint8Array(bytes));
    expect(decoded.kind).toBe("SessionOutput");
    if (decoded.kind === "SessionOutput") {
      expect(decoded.data.replay).toBe(true);
    }
  });

  it("round-trips SessionDiscontinuity", () => {
    const original: ArcToClient = {
      kind: "SessionDiscontinuity",
      data: { session_id: "sess-1", last_seq: 500 },
    };
    const bytes = msgpackEncode(original);
    const decoded = decodeMessage(new Uint8Array(bytes));
    expect(decoded.kind).toBe("SessionDiscontinuity");
    if (decoded.kind === "SessionDiscontinuity") {
      expect(decoded.data.last_seq).toBe(500);
    }
  });

  it("round-trips LeaseChanged with a non-null DriverInfo", () => {
    const driver: DriverInfo = {
      user_id: "user-alice",
      device_label: "MacBook Pro",
      since: 1713196800,
    };
    const original: ArcToClient = {
      kind: "LeaseChanged",
      data: { session_id: "sess-1", driver },
    };
    const bytes = msgpackEncode(original);
    const decoded = decodeMessage(new Uint8Array(bytes));
    expect(decoded.kind).toBe("LeaseChanged");
    if (decoded.kind === "LeaseChanged") {
      expect(decoded.data.driver).not.toBeNull();
      expect(decoded.data.driver?.user_id).toBe("user-alice");
      expect(decoded.data.driver?.device_label).toBe("MacBook Pro");
      expect(decoded.data.driver?.since).toBe(1713196800);
    }
  });

  it("round-trips LeaseChanged with null driver", () => {
    const original: ArcToClient = {
      kind: "LeaseChanged",
      data: { session_id: "sess-1", driver: null },
    };
    const bytes = msgpackEncode(original);
    const decoded = decodeMessage(new Uint8Array(bytes));
    expect(decoded.kind).toBe("LeaseChanged");
    if (decoded.kind === "LeaseChanged") {
      expect(decoded.data.driver).toBeNull();
    }
  });

  it("round-trips ControlRequested", () => {
    const original: ArcToClient = {
      kind: "ControlRequested",
      data: {
        session_id: "sess-1",
        request_id: "req-xyz",
        from_user: "user-bob",
        from_device: "ThinkPad X1",
      },
    };
    const bytes = msgpackEncode(original);
    const decoded = decodeMessage(new Uint8Array(bytes));
    expect(decoded.kind).toBe("ControlRequested");
    if (decoded.kind === "ControlRequested") {
      expect(decoded.data.request_id).toBe("req-xyz");
      expect(decoded.data.from_user).toBe("user-bob");
    }
  });

  it("round-trips PresenceChanged with empty watchers list", () => {
    const original: ArcToClient = {
      kind: "PresenceChanged",
      data: { session_id: "sess-1", watchers: [] },
    };
    const bytes = msgpackEncode(original);
    const decoded = decodeMessage(new Uint8Array(bytes));
    expect(decoded.kind).toBe("PresenceChanged");
    if (decoded.kind === "PresenceChanged") {
      expect(decoded.data.watchers).toEqual([]);
    }
  });

  it("round-trips PresenceChanged with populated watchers", () => {
    const watchers: WatcherInfo[] = [
      { user_id: "user-alice", device_label: "MacBook Pro", can_drive: true },
      { user_id: "user-bob", device_label: "ThinkPad", can_drive: false },
    ];
    const original: ArcToClient = {
      kind: "PresenceChanged",
      data: { session_id: "sess-1", watchers },
    };
    const bytes = msgpackEncode(original);
    const decoded = decodeMessage(new Uint8Array(bytes));
    expect(decoded.kind).toBe("PresenceChanged");
    if (decoded.kind === "PresenceChanged") {
      expect(decoded.data.watchers).toHaveLength(2);
      expect(decoded.data.watchers[0].can_drive).toBe(true);
      expect(decoded.data.watchers[1].can_drive).toBe(false);
    }
  });

  it("round-trips HostInput", () => {
    const original: ArcToClient = {
      kind: "HostInput",
      data: {
        session_id: "sess-1",
        bytes: new Uint8Array([0x6c, 0x73]),
        from_user: "user-alice",
      },
    };
    const bytes = msgpackEncode(original);
    const decoded = decodeMessage(new Uint8Array(bytes));
    expect(decoded.kind).toBe("HostInput");
    if (decoded.kind === "HostInput") {
      expect(decoded.data.from_user).toBe("user-alice");
    }
  });

  it("round-trips SessionRevoked", () => {
    const original: ArcToClient = {
      kind: "SessionRevoked",
      data: { session_id: "sess-1", reason: "host stopped sharing" },
    };
    const bytes = msgpackEncode(original);
    const decoded = decodeMessage(new Uint8Array(bytes));
    expect(decoded.kind).toBe("SessionRevoked");
    if (decoded.kind === "SessionRevoked") {
      expect(decoded.data.reason).toBe("host stopped sharing");
    }
  });
});
