CREATE TABLE plugin_registry (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    team_id       UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    version       TEXT NOT NULL,
    manifest      JSONB NOT NULL,
    source_url    TEXT,
    enabled       BOOLEAN NOT NULL DEFAULT true,
    installed_by  UUID REFERENCES users(id),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(team_id, name)
);

CREATE TABLE plugin_config (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    registry_id   UUID NOT NULL REFERENCES plugin_registry(id) ON DELETE CASCADE,
    config_key    TEXT NOT NULL,
    config_val    JSONB NOT NULL,
    updated_by    UUID REFERENCES users(id),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(registry_id, config_key)
);

CREATE INDEX idx_plugin_registry_team ON plugin_registry(team_id);
CREATE INDEX idx_plugin_config_registry ON plugin_config(registry_id);
