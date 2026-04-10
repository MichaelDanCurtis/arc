import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { BlockRenderer } from "@/components/blocks/block-renderer";
import type { Block } from "@/types/block";

function makeBlock(overrides: Partial<{ command: string; exitCode: number | null; output: string }>): Block {
  const encoder = new TextEncoder();
  return {
    header: {
      id: "blk-1",
      session_id: "sess-1",
      tab_id: "tab-1",
      command: overrides.command ?? "ls -la",
      cwd: "/home/user",
      started_at: Date.now(),
      env_snapshot: {},
      host_info: { hostname: "dev", os: "linux", arch: "x86_64", beacon_version: "0.1.0" },
    },
    output: encoder.encode(overrides.output ?? "file1.txt\nfile2.txt"),
    footer: {
      id: "blk-1",
      exit_code: overrides.exitCode ?? 0,
      ended_at: Date.now(),
      shadow_git_ref: null,
    },
  };
}

describe("BlockRenderer", () => {
  it("renders the command", () => {
    render(<BlockRenderer block={makeBlock({ command: "npm test" })} />);
    expect(screen.getByText("npm test")).toBeInTheDocument();
  });

  it("renders exit code 0 with success styling", () => {
    render(<BlockRenderer block={makeBlock({ exitCode: 0 })} />);
    const badge = screen.getByText("0");
    expect(badge).toBeInTheDocument();
  });

  it("renders exit code 1 with error styling", () => {
    render(<BlockRenderer block={makeBlock({ exitCode: 1 })} />);
    const badge = screen.getByText("1");
    expect(badge).toBeInTheDocument();
  });

  it("renders the working directory", () => {
    render(<BlockRenderer block={makeBlock({})} />);
    expect(screen.getByText("/home/user")).toBeInTheDocument();
  });
});
