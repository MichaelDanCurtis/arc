import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { SessionCard } from "@/components/session/session-card";
import type { SessionResponse } from "@/types/api";

describe("SessionCard", () => {
  const session: SessionResponse = {
    id: "sess-1",
    name: "my-project",
    state: "active",
    started_at: "2026-04-09T14:00:00Z",
    last_seen_at: "2026-04-09T14:30:00Z",
  };

  it("renders session name", () => {
    render(<SessionCard session={session} />);
    expect(screen.getByText("my-project")).toBeInTheDocument();
  });

  it("renders state badge", () => {
    render(<SessionCard session={session} />);
    expect(screen.getByText("active")).toBeInTheDocument();
  });

  it("renders unnamed session with id", () => {
    const unnamed = { ...session, name: null };
    render(<SessionCard session={unnamed} />);
    expect(screen.getByText("sess-1")).toBeInTheDocument();
  });
});
