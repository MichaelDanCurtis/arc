import { describe, it, expect } from "vitest";
import { encode as msgpackEncode, decode as msgpackDecode } from "@msgpack/msgpack";
import { encodeMessage, decodeMessage } from "@/lib/ws";
import type { ClientToArc, ArcToClient } from "@/types/protocol";

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
