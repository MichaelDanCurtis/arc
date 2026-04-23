-- 005_signing_keys.sql
ALTER TABLE users ADD COLUMN IF NOT EXISTS signing_public_key TEXT;

CREATE TABLE IF NOT EXISTS server_config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
