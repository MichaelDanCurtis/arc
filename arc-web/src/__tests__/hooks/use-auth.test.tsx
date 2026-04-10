import { describe, it, expect, beforeEach, vi } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { AuthProvider, useAuth } from "@/hooks/use-auth";
import type { ReactNode } from "react";

const mockFetch = vi.fn();
vi.stubGlobal("fetch", mockFetch);
vi.stubEnv("NEXT_PUBLIC_ARC_RELAY_URL", "http://localhost:9900");

function wrapper({ children }: { children: ReactNode }) {
  return <AuthProvider>{children}</AuthProvider>;
}

describe("useAuth", () => {
  beforeEach(() => {
    localStorage.clear();
    mockFetch.mockReset();
  });

  it("starts as unauthenticated", () => {
    const { result } = renderHook(() => useAuth(), { wrapper });
    expect(result.current.isAuthenticated).toBe(false);
    expect(result.current.user).toBeNull();
  });

  it("login stores tokens and sets user", async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: async () => ({
        user_id: "u1",
        access_token: "access-123",
        refresh_token: "refresh-456",
      }),
    });
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: async () => ({ id: "u1", email: "test@test.com", name: "Test" }),
    });

    const { result } = renderHook(() => useAuth(), { wrapper });

    await act(async () => {
      await result.current.login("test@test.com", "password");
    });

    expect(result.current.isAuthenticated).toBe(true);
    expect(result.current.user?.email).toBe("test@test.com");
    expect(localStorage.getItem("arc_access_token")).toBe("access-123");
  });

  it("logout clears tokens and user", async () => {
    localStorage.setItem("arc_access_token", "tok");
    localStorage.setItem("arc_refresh_token", "ref");

    const { result } = renderHook(() => useAuth(), { wrapper });

    act(() => {
      result.current.logout();
    });

    expect(result.current.isAuthenticated).toBe(false);
    expect(result.current.user).toBeNull();
    expect(localStorage.getItem("arc_access_token")).toBeNull();
  });
});
