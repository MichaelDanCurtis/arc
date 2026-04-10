"use client";

import {
  createContext,
  useContext,
  useState,
  useCallback,
  useEffect,
  type ReactNode,
} from "react";
import { apiClient } from "@/lib/api";
import {
  getAccessToken,
  setTokens,
  clearTokens,
} from "@/lib/auth";
import type { LoginRequest, AuthResponse, UserProfile } from "@/types/api";

interface AuthContextValue {
  user: UserProfile | null;
  isAuthenticated: boolean;
  loading: boolean;
  login: (email: string, password: string) => Promise<void>;
  register: (email: string, password: string, name?: string) => Promise<void>;
  logout: () => void;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<UserProfile | null>(null);
  const [loading, setLoading] = useState(true);

  // Check for existing token on mount
  useEffect(() => {
    const token = getAccessToken();
    if (token) {
      apiClient
        .get<UserProfile>("/api/users/me")
        .then(setUser)
        .catch(() => clearTokens())
        .finally(() => setLoading(false));
    } else {
      setLoading(false);
    }
  }, []);

  const login = useCallback(async (email: string, password: string) => {
    const body: LoginRequest = { email, password };
    const auth = await apiClient.post<AuthResponse>("/api/auth/login", body);
    setTokens(auth.access_token, auth.refresh_token);
    const profile = await apiClient.get<UserProfile>("/api/users/me");
    setUser(profile);
  }, []);

  const register = useCallback(
    async (email: string, password: string, name?: string) => {
      const auth = await apiClient.post<AuthResponse>("/api/auth/register", {
        email,
        password,
        name,
      });
      setTokens(auth.access_token, auth.refresh_token);
      const profile = await apiClient.get<UserProfile>("/api/users/me");
      setUser(profile);
    },
    [],
  );

  const logout = useCallback(() => {
    clearTokens();
    setUser(null);
  }, []);

  return (
    <AuthContext.Provider
      value={{
        user,
        isAuthenticated: user !== null,
        loading,
        login,
        register,
        logout,
      }}
    >
      {children}
    </AuthContext.Provider>
  );
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error("useAuth must be used within AuthProvider");
  return ctx;
}
