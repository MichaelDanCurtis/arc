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

// --- First-run setup ---

export interface SetupStatus {
  is_configured: boolean;
  owner_exists: boolean;
}

export interface SetupRequest {
  owner_name: string;
  owner_email: string;
  team_name: string;
  password: string;
}

export interface SetupResponse {
  owner_id: string;
  team_id: string;
  access_token: string;
  refresh_token: string;
  message: string;
}

// --- Shadow diff ---

export interface ShadowDiffResponse {
  session_id: string;
  diff: string;
}

// --- Plugin ---

export interface PluginEntry {
  id: string;
  team_id: string;
  name: string;
  version: string;
  manifest: Record<string, unknown>;
  source_url: string | null;
  enabled: boolean;
  installed_by: string | null;
  created_at: string;
  updated_at: string;
}

export interface PluginConfigEntry {
  config_key: string;
  config_val: unknown;
  updated_at: string;
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

// --- Marketplace ---

export interface MarketplacePlugin {
  id: string;
  name: string;
  slug: string;
  description: string | null;
  category: string | null;
  icon_url: string | null;
  homepage_url: string | null;
  repository_url: string | null;
  license: string | null;
  payment_type: string;
  price_cents: number;
  author_name: string | null;
  download_count: number;
  latest_version: string | null;
  updated_at: string;
}

export interface MarketplaceVersion {
  id: string;
  plugin_id: string;
  version: string;
  release_notes: string | null;
  platform: string;
  size_bytes: number | null;
  download_count: number;
  created_at: string;
}

export interface CategoriesResponse {
  categories: string[];
}

// --- Organization ---

export interface Organization {
  id: string;
  name: string;
  slug: string;
}
