-- 004_marketplace.sql
-- Organizations
CREATE TABLE IF NOT EXISTS organizations (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL,
    slug            TEXT UNIQUE NOT NULL,
    domain          TEXT UNIQUE,
    allow_public    BOOLEAN NOT NULL DEFAULT true,
    idp_provider    TEXT,
    idp_config      JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Org memberships
CREATE TABLE IF NOT EXISTS org_memberships (
    user_id         UUID NOT NULL REFERENCES users(id),
    org_id          UUID NOT NULL REFERENCES organizations(id),
    role            TEXT NOT NULL DEFAULT 'member',
    UNIQUE(user_id, org_id)
);

-- Teams optionally belong to an org
ALTER TABLE teams ADD COLUMN IF NOT EXISTS org_id UUID REFERENCES organizations(id);

-- User OAuth columns
ALTER TABLE users ADD COLUMN IF NOT EXISTS avatar_url TEXT;
ALTER TABLE users ADD COLUMN IF NOT EXISTS oauth_provider TEXT;
ALTER TABLE users ADD COLUMN IF NOT EXISTS oauth_provider_id TEXT;

-- Make password_hash nullable (OAuth users won't have one)
ALTER TABLE users ALTER COLUMN password_hash DROP NOT NULL;

-- Marketplace plugin listings
CREATE TABLE IF NOT EXISTS marketplace_plugins (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    author_id       UUID NOT NULL REFERENCES users(id),
    name            TEXT UNIQUE NOT NULL,
    slug            TEXT UNIQUE NOT NULL,
    description     TEXT,
    category        TEXT,
    icon_url        TEXT,
    homepage_url    TEXT,
    repository_url  TEXT,
    license         TEXT,
    status          TEXT NOT NULL DEFAULT 'draft',
    price_cents     INTEGER NOT NULL DEFAULT 0,
    payment_type    TEXT NOT NULL DEFAULT 'free',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Versioned releases with per-platform binaries
CREATE TABLE IF NOT EXISTS marketplace_versions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    plugin_id       UUID NOT NULL REFERENCES marketplace_plugins(id) ON DELETE CASCADE,
    version         TEXT NOT NULL,
    release_notes   TEXT,
    archive_url     TEXT NOT NULL,
    archive_sha256  TEXT NOT NULL,
    signature       TEXT,
    countersig      TEXT,
    platform        TEXT NOT NULL,
    size_bytes      BIGINT,
    download_count  BIGINT NOT NULL DEFAULT 0,
    status          TEXT NOT NULL DEFAULT 'pending_review',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(plugin_id, version, platform)
);

-- Org plugin allowlist
CREATE TABLE IF NOT EXISTS org_plugin_allowlist (
    org_id          UUID NOT NULL REFERENCES organizations(id),
    plugin_id       UUID NOT NULL REFERENCES marketplace_plugins(id),
    added_by        UUID NOT NULL REFERENCES users(id),
    added_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(org_id, plugin_id)
);

-- Full-text search index for marketplace
CREATE INDEX IF NOT EXISTS idx_marketplace_plugins_search
    ON marketplace_plugins USING gin(to_tsvector('english', name || ' ' || COALESCE(description, '')));

CREATE INDEX IF NOT EXISTS idx_marketplace_plugins_category
    ON marketplace_plugins(category);

CREATE INDEX IF NOT EXISTS idx_marketplace_plugins_status
    ON marketplace_plugins(status);
