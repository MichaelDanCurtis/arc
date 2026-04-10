import { describe, it, expect, beforeEach, vi } from "vitest";
import { apiClient } from "@/lib/api";
import { setTokens, clearTokens, getAccessToken } from "@/lib/auth";

// Mock fetch globally
const mockFetch = vi.fn();
vi.stubGlobal("fetch", mockFetch);

// Stub env
vi.stubEnv("NEXT_PUBLIC_ARC_RELAY_URL", "http://localhost:9900");

describe("apiClient", () => {
  beforeEach(() => {
    localStorage.clear();
    mockFetch.mockReset();
  });

  it("sends GET with auth header", async () => {
    setTokens("my-token", "my-refresh");
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: async () => ({ id: "1", email: "test@test.com", name: null }),
    });

    const result = await apiClient.get("/api/users/me");
    expect(result).toEqual({ id: "1", email: "test@test.com", name: null });
    expect(mockFetch).toHaveBeenCalledWith(
      "http://localhost:9900/api/users/me",
      expect.objectContaining({
        headers: expect.objectContaining({
          Authorization: "Bearer my-token",
        }),
      }),
    );
  });

  it("sends POST with JSON body", async () => {
    setTokens("my-token", "my-refresh");
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: async () => ({ user_id: "u1", access_token: "a", refresh_token: "r" }),
    });

    const result = await apiClient.post("/api/auth/login", {
      email: "test@test.com",
      password: "pass",
    });
    expect(result).toEqual({ user_id: "u1", access_token: "a", refresh_token: "r" });
    expect(mockFetch).toHaveBeenCalledWith(
      "http://localhost:9900/api/auth/login",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ email: "test@test.com", password: "pass" }),
      }),
    );
  });

  it("refreshes token on 401 and retries", async () => {
    setTokens("expired-token", "valid-refresh");

    // First call returns 401
    mockFetch.mockResolvedValueOnce({ ok: false, status: 401 });
    // Refresh call returns new tokens
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: async () => ({
        user_id: "u1",
        access_token: "new-access",
        refresh_token: "new-refresh",
      }),
    });
    // Retry returns success
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: async () => ({ id: "1", email: "test@test.com", name: null }),
    });

    const result = await apiClient.get("/api/users/me");
    expect(result).toEqual({ id: "1", email: "test@test.com", name: null });
    expect(getAccessToken()).toBe("new-access");
    expect(mockFetch).toHaveBeenCalledTimes(3);
  });

  it("throws on non-401 error", async () => {
    setTokens("my-token", "my-refresh");
    mockFetch.mockResolvedValueOnce({
      ok: false,
      status: 500,
      json: async () => ({ error: "internal" }),
    });

    await expect(apiClient.get("/api/sessions")).rejects.toThrow();
  });
});
