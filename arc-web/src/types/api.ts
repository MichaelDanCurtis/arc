// --- Auth ---

export interface RegisterRequest {
  email: string;
  password: string;
  name?: string;
}

export interface LoginRequest {
  email: string;
  password: string;
}

export interface RefreshRequest {
  refresh_token: string;
}

export interface AuthResponse {
  user_id: string;
  access_token: string;
  refresh_token: string;
}

// --- User ---

export interface UserProfile {
  id: string;
  email: string;
  name: string | null;
}

// --- Session ---

export interface SessionResponse {
  id: string;
  name: string | null;
  state: "active" | "suspended" | "ended";
  started_at: string; // RFC3339
  last_seen_at: string; // RFC3339
}

// --- Team ---

export interface CreateTeamRequest {
  name: string;
}

export interface TeamResponse {
  id: string;
  name: string;
  owner_id: string;
}

export interface MemberResponse {
  user_id: string;
  role: "owner" | "admin" | "member";
}

// --- Audit log ---

export interface AuditEntry {
  id: number;
  timestamp_ms: number;
  actor: string;
  action: string;
  resource: string;
  detail: string;
}

export interface AuditQuery {
  limit?: number;
  offset?: number;
  actor?: string;
  action?: string;
}
