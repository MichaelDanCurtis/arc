-- Users
CREATE TABLE IF NOT EXISTS users (
  id UUID PRIMARY KEY,
  email TEXT UNIQUE NOT NULL,
  name TEXT,
  password_hash TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Teams
CREATE TABLE IF NOT EXISTS teams (
  id UUID PRIMARY KEY,
  name TEXT NOT NULL,
  owner_id UUID NOT NULL REFERENCES users(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Team members
CREATE TABLE IF NOT EXISTS team_members (
  team_id UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK (role IN ('owner','admin','member')),
  PRIMARY KEY (team_id, user_id)
);

-- Sessions
CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY,
  host_id UUID NOT NULL,
  owner_id UUID NOT NULL REFERENCES users(id),
  team_id UUID REFERENCES teams(id),
  name TEXT,
  state TEXT NOT NULL CHECK (state IN ('active','suspended','ended')),
  metadata JSONB,
  started_at TIMESTAMPTZ NOT NULL,
  ended_at TIMESTAMPTZ,
  last_seen_at TIMESTAMPTZ NOT NULL
);

-- Session grants
CREATE TABLE IF NOT EXISTS session_grants (
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  grantee_user_id UUID NOT NULL REFERENCES users(id),
  can_write BOOLEAN NOT NULL DEFAULT FALSE,
  granted_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  revoked_at TIMESTAMPTZ,
  PRIMARY KEY (session_id, grantee_user_id)
);

-- Blocks
CREATE TABLE IF NOT EXISTS blocks (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  tab_id TEXT NOT NULL,
  command TEXT NOT NULL,
  output BYTEA,
  exit_code INTEGER,
  cwd TEXT,
  started_at TIMESTAMPTZ NOT NULL,
  ended_at TIMESTAMPTZ,
  shadow_git_ref TEXT,
  pinned BOOLEAN NOT NULL DEFAULT FALSE,
  host_info JSONB,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_blocks_session_time ON blocks(session_id, started_at);
CREATE INDEX IF NOT EXISTS idx_blocks_retention ON blocks(created_at) WHERE NOT pinned;

-- Audit log
CREATE TABLE IF NOT EXISTS audit_log (
  id BIGSERIAL PRIMARY KEY,
  actor_id UUID,
  action TEXT NOT NULL,
  target_type TEXT,
  target_id TEXT,
  metadata JSONB,
  at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_audit_at ON audit_log(at DESC);
