-- Deviation from plan (sanctioned): The original plan calls for
--   ALTER TABLE session_grants ALTER COLUMN grantee_user_id DROP NOT NULL
-- but the existing table has PRIMARY KEY (session_id, grantee_user_id).
-- PostgreSQL does NOT permit NULL values in PK columns, so the DROP NOT NULL
-- would fail while grantee_user_id is still part of the PK.
--
-- Fix applied:
--   1. Drop the composite PK (session_id, grantee_user_id).
--   2. Add a surrogate UUID PK column `id` so rows retain a stable primary key.
--   3. Then proceed with the rest of the plan verbatim.
--
-- This matches the sanctioned deviation documented in
-- 2026-04-14-session-mirroring.md / Task 2.1.

-- Step 1: Drop old composite primary key (IF EXISTS so partial re-apply is safe).
ALTER TABLE session_grants DROP CONSTRAINT IF EXISTS session_grants_pkey;

-- Step 2: Add surrogate primary key (split for idempotency: ADD COLUMN IF NOT EXISTS,
-- then conditionally ADD PRIMARY KEY via pg_constraint guard).
ALTER TABLE session_grants ADD COLUMN IF NOT EXISTS id UUID DEFAULT gen_random_uuid();
DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM pg_constraint
    WHERE conname = 'session_grants_pkey'
  ) THEN
    ALTER TABLE session_grants ADD PRIMARY KEY (id);
  END IF;
END$$;

-- Extend session_grants to support email + team targets and per-grant control flag.
ALTER TABLE session_grants ADD COLUMN IF NOT EXISTS target_email TEXT;
ALTER TABLE session_grants ADD COLUMN IF NOT EXISTS target_team_id UUID REFERENCES teams(id);
ALTER TABLE session_grants ADD COLUMN IF NOT EXISTS can_request_control BOOLEAN NOT NULL DEFAULT TRUE;

-- grantee_user_id is now nullable (email/team grants have no direct user reference).
ALTER TABLE session_grants ALTER COLUMN grantee_user_id DROP NOT NULL;

-- Enforce exactly one of the three target columns is set.
-- Guard with pg_constraint check because ADD CONSTRAINT has no IF NOT EXISTS.
DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM pg_constraint
    WHERE conname = 'one_grant_target'
  ) THEN
    ALTER TABLE session_grants ADD CONSTRAINT one_grant_target CHECK (
      (grantee_user_id IS NOT NULL)::int +
      (target_email   IS NOT NULL)::int +
      (target_team_id IS NOT NULL)::int = 1
    );
  END IF;
END$$;

CREATE INDEX IF NOT EXISTS idx_session_grants_email ON session_grants (lower(target_email)) WHERE revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_session_grants_team ON session_grants (target_team_id) WHERE revoked_at IS NULL;

-- Optional refresh-token revocation (deferred but schema lands now).
CREATE TABLE IF NOT EXISTS refresh_tokens (
  id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  token_hash  TEXT NOT NULL UNIQUE,
  issued_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  revoked_at  TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user ON refresh_tokens (user_id) WHERE revoked_at IS NULL;
