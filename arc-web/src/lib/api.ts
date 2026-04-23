import { getAccessToken, getRefreshToken, setTokens, clearTokens } from "./auth";
import { getRelayUrl } from "./config";
import type { RefreshRequest, AuthResponse } from "@/types/api";

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const baseUrl = getRelayUrl();
  const token = getAccessToken();

  const isFormData = options.body instanceof FormData;
  const headers: Record<string, string> = {
    ...(isFormData ? {} : { "Content-Type": "application/json" }),
    ...((options.headers as Record<string, string>) || {}),
  };
  if (token) {
    headers["Authorization"] = `Bearer ${token}`;
  }

  const res = await fetch(`${baseUrl}${path}`, { ...options, headers });

  if (res.ok) {
    return res.json();
  }

  // Try token refresh on 401
  if (res.status === 401) {
    const refreshToken = getRefreshToken();
    if (refreshToken) {
      const refreshed = await tryRefresh(baseUrl, refreshToken);
      if (refreshed) {
        // Retry with new token
        headers["Authorization"] = `Bearer ${refreshed.access_token}`;
        const retry = await fetch(`${baseUrl}${path}`, { ...options, headers });
        if (retry.ok) {
          return retry.json();
        }
      }
    }
    clearTokens();
    throw new ApiError(401, "Authentication failed");
  }

  throw new ApiError(res.status, `API error: ${res.status}`);
}

async function tryRefresh(
  baseUrl: string,
  refreshToken: string,
): Promise<AuthResponse | null> {
  try {
    const body: RefreshRequest = { refresh_token: refreshToken };
    const res = await fetch(`${baseUrl}/api/auth/refresh`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!res.ok) return null;
    const data: AuthResponse = await res.json();
    setTokens(data.access_token, data.refresh_token);
    return data;
  } catch {
    return null;
  }
}

export const apiClient = {
  get<T>(path: string): Promise<T> {
    return request<T>(path);
  },
  post<T>(path: string, body?: unknown): Promise<T> {
    return request<T>(path, {
      method: "POST",
      body: body ? JSON.stringify(body) : undefined,
    });
  },
  patch<T>(path: string, body?: unknown): Promise<T> {
    return request<T>(path, {
      method: "PATCH",
      body: body ? JSON.stringify(body) : undefined,
    });
  },
  del<T>(path: string): Promise<T> {
    return request<T>(path, { method: "DELETE" });
  },
  upload<T>(path: string, formData: FormData): Promise<T> {
    return request<T>(path, {
      method: "POST",
      body: formData,
      headers: {},
    });
  },
};
