use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::middleware::AuthUser;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

async fn resolve_team_id(
    pool: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<Uuid, (StatusCode, String)> {
    let teams = crate::models::team::list_teams_for_user(pool, user_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?;
    teams
        .first()
        .map(|t| t.id)
        .ok_or((StatusCode::FORBIDDEN, "No team membership".to_string()))
}

// ---------------------------------------------------------------------------
// Response / request types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PluginEntry {
    pub id: Uuid,
    pub team_id: Uuid,
    pub name: String,
    pub version: String,
    pub manifest: serde_json::Value,
    pub source_url: Option<String>,
    pub enabled: bool,
    pub installed_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct RegisterPluginRequest {
    pub name: String,
    pub version: String,
    pub manifest: serde_json::Value,
    pub source_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PatchPluginRequest {
    pub enabled: Option<bool>,
    pub version: Option<String>,
    pub manifest: Option<serde_json::Value>,
    pub source_url: Option<String>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PluginConfigEntry {
    pub config_key: String,
    pub config_val: serde_json::Value,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct WriteConfigRequest {
    pub entries: Vec<ConfigKV>,
}

#[derive(Debug, Deserialize)]
pub struct ConfigKV {
    pub config_key: String,
    pub config_val: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct SyncPluginsRequest {
    pub installed: Vec<InstalledPlugin>,
}

#[derive(Debug, Deserialize)]
pub struct InstalledPlugin {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct SyncPluginsResponse {
    pub install: Vec<SyncInstallEntry>,
    pub remove: Vec<String>,
    pub config: Vec<SyncConfigEntry>,
}

#[derive(Debug, Serialize)]
pub struct SyncInstallEntry {
    pub name: String,
    pub version: String,
    pub source_url: String,
    pub manifest: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct SyncConfigEntry {
    pub plugin_name: String,
    pub config_key: String,
    pub config_val: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/plugins
///
/// List all plugins registered for the authenticated user's team.
pub async fn list_plugins(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
) -> Result<Json<Vec<PluginEntry>>, (StatusCode, String)> {
    let team_id = resolve_team_id(&state.pool, user_id).await?;

    let plugins = sqlx::query_as::<_, PluginEntry>(
        "SELECT id, team_id, name, version, manifest, source_url, enabled, installed_by, \
         created_at, updated_at \
         FROM plugin_registry \
         WHERE team_id = $1 \
         ORDER BY name",
    )
    .bind(team_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?;

    Ok(Json(plugins))
}

/// POST /api/plugins
///
/// Register (or update) a plugin for the authenticated user's team.
pub async fn register_plugin(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Json(req): Json<RegisterPluginRequest>,
) -> Result<(StatusCode, Json<PluginEntry>), (StatusCode, String)> {
    let team_id = resolve_team_id(&state.pool, user_id).await?;

    let entry = sqlx::query_as::<_, PluginEntry>(
        "INSERT INTO plugin_registry \
             (team_id, name, version, manifest, source_url, installed_by) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (team_id, name) DO UPDATE SET \
             version      = EXCLUDED.version, \
             manifest     = EXCLUDED.manifest, \
             source_url   = EXCLUDED.source_url, \
             installed_by = EXCLUDED.installed_by, \
             updated_at   = now() \
         RETURNING id, team_id, name, version, manifest, source_url, enabled, \
                   installed_by, created_at, updated_at",
    )
    .bind(team_id)
    .bind(&req.name)
    .bind(&req.version)
    .bind(&req.manifest)
    .bind(&req.source_url)
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?;

    Ok((StatusCode::CREATED, Json(entry)))
}

/// DELETE /api/plugins/:id
///
/// Remove a plugin. Returns 204 if deleted, 404 if not found.
pub async fn delete_plugin(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(plugin_id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, String)> {
    let team_id = resolve_team_id(&state.pool, user_id).await?;

    let result = sqlx::query(
        "DELETE FROM plugin_registry WHERE id = $1 AND team_id = $2",
    )
    .bind(plugin_id)
    .bind(team_id)
    .execute(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?;

    if result.rows_affected() == 0 {
        Err((StatusCode::NOT_FOUND, "Plugin not found".to_string()))
    } else {
        Ok(StatusCode::NO_CONTENT)
    }
}

/// PATCH /api/plugins/:id
///
/// Partially update a plugin (enable/disable, version, manifest, source_url).
pub async fn patch_plugin(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(plugin_id): Path<Uuid>,
    Json(req): Json<PatchPluginRequest>,
) -> Result<Json<PluginEntry>, (StatusCode, String)> {
    let team_id = resolve_team_id(&state.pool, user_id).await?;

    let entry = sqlx::query_as::<_, PluginEntry>(
        "UPDATE plugin_registry SET \
             enabled    = COALESCE($3, enabled), \
             version    = COALESCE($4, version), \
             manifest   = COALESCE($5, manifest), \
             source_url = COALESCE($6, source_url), \
             updated_at = now() \
         WHERE id = $1 AND team_id = $2 \
         RETURNING id, team_id, name, version, manifest, source_url, enabled, \
                   installed_by, created_at, updated_at",
    )
    .bind(plugin_id)
    .bind(team_id)
    .bind(req.enabled)
    .bind(req.version.as_deref())
    .bind(req.manifest.as_ref())
    .bind(req.source_url.as_deref())
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?
    .ok_or((StatusCode::NOT_FOUND, "Plugin not found".to_string()))?;

    Ok(Json(entry))
}

/// GET /api/plugins/:id/config
///
/// Return config entries for a plugin (after verifying team ownership).
pub async fn get_plugin_config(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(plugin_id): Path<Uuid>,
) -> Result<Json<Vec<PluginConfigEntry>>, (StatusCode, String)> {
    let team_id = resolve_team_id(&state.pool, user_id).await?;

    // Verify plugin belongs to team
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM plugin_registry WHERE id = $1 AND team_id = $2)",
    )
    .bind(plugin_id)
    .bind(team_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?;

    if !exists {
        return Err((StatusCode::NOT_FOUND, "Plugin not found".to_string()));
    }

    let entries = sqlx::query_as::<_, PluginConfigEntry>(
        "SELECT config_key, config_val, updated_at \
         FROM plugin_config \
         WHERE registry_id = $1 \
         ORDER BY config_key",
    )
    .bind(plugin_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?;

    Ok(Json(entries))
}

/// PUT /api/plugins/:id/config
///
/// Upsert config entries for a plugin.
pub async fn write_plugin_config(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(plugin_id): Path<Uuid>,
    Json(req): Json<WriteConfigRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let team_id = resolve_team_id(&state.pool, user_id).await?;

    // Verify plugin belongs to team
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM plugin_registry WHERE id = $1 AND team_id = $2)",
    )
    .bind(plugin_id)
    .bind(team_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?;

    if !exists {
        return Err((StatusCode::NOT_FOUND, "Plugin not found".to_string()));
    }

    for kv in &req.entries {
        sqlx::query(
            "INSERT INTO plugin_config (registry_id, config_key, config_val, updated_by) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (registry_id, config_key) DO UPDATE SET \
                 config_val = EXCLUDED.config_val, \
                 updated_by = EXCLUDED.updated_by, \
                 updated_at = now()",
        )
        .bind(plugin_id)
        .bind(&kv.config_key)
        .bind(&kv.config_val)
        .bind(user_id)
        .execute(&state.pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/plugins/sync
///
/// Compare client inventory against the team registry and return a diff:
///   install = plugins the client is missing or has at the wrong version (enabled, has source_url)
///   remove  = plugins the client has that are not in the registry (or disabled)
///   config  = all config entries for enabled plugins
pub async fn sync_plugins(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Json(req): Json<SyncPluginsRequest>,
) -> Result<Json<SyncPluginsResponse>, (StatusCode, String)> {
    let team_id = resolve_team_id(&state.pool, user_id).await?;

    // Fetch all registry entries for this team
    let registry = sqlx::query_as::<_, PluginEntry>(
        "SELECT id, team_id, name, version, manifest, source_url, enabled, installed_by, \
         created_at, updated_at \
         FROM plugin_registry \
         WHERE team_id = $1",
    )
    .bind(team_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?;

    // Build a map from name -> (version, source_url, enabled) for registry
    use std::collections::HashMap;
    let registry_map: HashMap<&str, &PluginEntry> =
        registry.iter().map(|p| (p.name.as_str(), p)).collect();

    // Build a set of client plugin names -> version
    let client_map: HashMap<&str, &str> = req
        .installed
        .iter()
        .map(|p| (p.name.as_str(), p.version.as_str()))
        .collect();

    // Determine installs: enabled registry plugins with source_url that are
    // missing from client, or version differs.
    let install: Vec<SyncInstallEntry> = registry
        .iter()
        .filter(|p| p.enabled)
        .filter_map(|p| {
            let source_url = p.source_url.as_deref()?; // skip if no source_url
            let needs_install = match client_map.get(p.name.as_str()) {
                None => true,
                Some(&client_ver) => client_ver != p.version.as_str(),
            };
            if needs_install {
                Some(SyncInstallEntry {
                    name: p.name.clone(),
                    version: p.version.clone(),
                    source_url: source_url.to_string(),
                    manifest: p.manifest.clone(),
                })
            } else {
                None
            }
        })
        .collect();

    // Determine removals: client plugins not in registry, or disabled in registry.
    let remove: Vec<String> = req
        .installed
        .iter()
        .filter(|p| {
            match registry_map.get(p.name.as_str()) {
                None => true,          // not in registry → remove
                Some(entry) => !entry.enabled, // disabled → remove
            }
        })
        .map(|p| p.name.clone())
        .collect();

    // Collect config for all enabled plugins
    let enabled_ids: Vec<Uuid> = registry
        .iter()
        .filter(|p| p.enabled)
        .map(|p| p.id)
        .collect();

    let mut config: Vec<SyncConfigEntry> = Vec::new();
    for entry in registry.iter().filter(|p| p.enabled) {
        if !enabled_ids.contains(&entry.id) {
            continue;
        }
        let cfg_rows = sqlx::query_as::<_, PluginConfigEntry>(
            "SELECT config_key, config_val, updated_at \
             FROM plugin_config WHERE registry_id = $1",
        )
        .bind(entry.id)
        .fetch_all(&state.pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?;

        for row in cfg_rows {
            config.push(SyncConfigEntry {
                plugin_name: entry.name.clone(),
                config_key: row.config_key,
                config_val: row.config_val,
            });
        }
    }

    Ok(Json(SyncPluginsResponse {
        install,
        remove,
        config,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_entry_serializes_correctly() {
        let entry = PluginEntry {
            id: Uuid::new_v4(),
            team_id: Uuid::new_v4(),
            name: "my-plugin".to_string(),
            version: "1.0.0".to_string(),
            manifest: serde_json::json!({"name": "my-plugin"}),
            source_url: Some("https://example.com/plugin.beaconplug".to_string()),
            enabled: true,
            installed_by: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["name"], "my-plugin");
        assert_eq!(json["version"], "1.0.0");
        assert_eq!(json["enabled"], true);
    }

    #[test]
    fn register_plugin_request_deserializes() {
        let json = r#"{"name":"test","version":"1.0.0","manifest":{},"source_url":null}"#;
        let req: RegisterPluginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "test");
        assert_eq!(req.version, "1.0.0");
        assert!(req.source_url.is_none());
    }

    #[test]
    fn patch_plugin_request_deserializes_partial() {
        let json = r#"{"enabled":false}"#;
        let req: PatchPluginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.enabled, Some(false));
        assert!(req.version.is_none());
        assert!(req.manifest.is_none());
    }

    #[test]
    fn sync_request_deserializes() {
        let json = r#"{"installed":[{"name":"foo","version":"1.0.0"}]}"#;
        let req: SyncPluginsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.installed.len(), 1);
        assert_eq!(req.installed[0].name, "foo");
    }

    #[test]
    fn write_config_request_deserializes() {
        let json = r#"{"entries":[{"config_key":"theme","config_val":"dark"}]}"#;
        let req: WriteConfigRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.entries.len(), 1);
        assert_eq!(req.entries[0].config_key, "theme");
    }
}
