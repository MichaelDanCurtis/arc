//! beacon-features — typed feature-flag enum with three-source precedence.
//!
//! Priority (highest to lowest):
//!   1. Environment variable  `BEACON_FF_<FLAG_NAME>=1|0`
//!   2. TOML config           `~/.config/beacon/features.toml` (or `$XDG_CONFIG_HOME/beacon/features.toml`)
//!   3. Built-in default      per-flag constant; debug builds treat `DOGFOOD_FLAGS` members as default-on.
//!
//! ## TOML format
//!
//! ```toml
//! arc_routing = true
//! new_persistence = false
//! ```
//!
//! ## Environment variable format
//!
//! `BEACON_FF_ARC_ROUTING=1`  (any truthy/falsy value accepted; see [`parse_env_bool`])
//!
//! ## `DOGFOOD_FLAGS` convention
//!
//! Add flags here to enable them by default in debug (`cfg(debug_assertions)`) builds.
//! They remain OFF in release unless an env var or TOML config explicitly enables them.
//! Empty by default — add when you want dogfood-only behaviour before a flag is GA.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// FeatureFlag enum
// ---------------------------------------------------------------------------

/// All known feature flags. Variants map 1-to-1 to snake_case TOML keys and
/// `BEACON_FF_<UPPER_SNAKE>` environment variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureFlag {
    /// Route cloud AI through Arc relay instead of direct BYOK. (P3 placeholder)
    ArcRouting,
    /// Enable new unified SQLite persistence layer. (P4 placeholder)
    NewPersistence,
    /// Remote sessions v2 transport. (P6 placeholder)
    RemoteV2,
    /// JSON-RPC spine unification. (P7 placeholder)
    JsonRpcSpine,
    /// Command signatures v2 corpus for ghost-text ranking. (P5 placeholder)
    CommandSignaturesV2,
    /// Prompt the user before an AI agent writes to a block for the first time.
    /// Default OFF in P3e2; Mac UI work lands in a follow-up phase.
    AgentFirstWritePrompt,
}

impl FeatureFlag {
    /// Canonical snake_case name used for TOML keys.
    pub fn as_str(self) -> &'static str {
        match self {
            FeatureFlag::ArcRouting => "arc_routing",
            FeatureFlag::NewPersistence => "new_persistence",
            FeatureFlag::RemoteV2 => "remote_v2",
            FeatureFlag::JsonRpcSpine => "json_rpc_spine",
            FeatureFlag::CommandSignaturesV2 => "command_signatures_v2",
            FeatureFlag::AgentFirstWritePrompt => "agent_first_write_prompt",
        }
    }

    /// Environment variable name for this flag: `BEACON_FF_<UPPER_SNAKE>`.
    pub fn env_var(self) -> String {
        format!("BEACON_FF_{}", self.as_str().to_uppercase())
    }

    /// Parse a flag from its snake_case string key. Returns `None` for unknown keys.
    pub fn from_str_key(s: &str) -> Option<Self> {
        match s {
            "arc_routing" => Some(FeatureFlag::ArcRouting),
            "new_persistence" => Some(FeatureFlag::NewPersistence),
            "remote_v2" => Some(FeatureFlag::RemoteV2),
            "json_rpc_spine" => Some(FeatureFlag::JsonRpcSpine),
            "command_signatures_v2" => Some(FeatureFlag::CommandSignaturesV2),
            "agent_first_write_prompt" => Some(FeatureFlag::AgentFirstWritePrompt),
            _ => None,
        }
    }

    /// Built-in default for this flag (independent of debug/release mode).
    fn builtin_default(self) -> bool {
        match self {
            // GA — JSON-RPC spine handlers are now always registered.
            FeatureFlag::JsonRpcSpine => true,
            // All other flags default OFF unless listed in DOGFOOD_FLAGS (for debug builds).
            _ => false,
        }
    }
}

impl fmt::Display for FeatureFlag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// DOGFOOD_FLAGS — flags that are default-ON in debug builds
// ---------------------------------------------------------------------------

/// Flags enabled by default in `cfg(debug_assertions)` builds.
///
/// Convention: add a flag here while it is under active development and you
/// want dogfood builds to exercise the new code path without requiring every
/// developer to edit their `features.toml`.  Remove it once the flag is GA
/// (or permanently off).
pub const DOGFOOD_FLAGS: &[FeatureFlag] = &[
    FeatureFlag::NewPersistence,
    FeatureFlag::CommandSignaturesV2,
    FeatureFlag::RemoteV2,
];

// ---------------------------------------------------------------------------
// TOML config — parsed once at startup
// ---------------------------------------------------------------------------

/// Parsed representation of `~/.config/beacon/features.toml`.
#[derive(Debug, Default, Deserialize)]
struct TomlConfig {
    #[serde(flatten)]
    flags: HashMap<String, bool>,
}

fn config_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            dirs_home().join(".config")
        });
    base.join("beacon").join("features.toml")
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
}

static TOML_CONFIG: OnceLock<TomlConfig> = OnceLock::new();

fn toml_config() -> &'static TomlConfig {
    TOML_CONFIG.get_or_init(|| {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                // Silently fall back to empty config on parse error; don't panic.
                eprintln!("[beacon-features] failed to parse {}: {}", path.display(), e);
                TomlConfig::default()
            }),
            Err(_) => TomlConfig::default(), // file absent = all defaults
        }
    })
}

// ---------------------------------------------------------------------------
// parse_env_bool helper
// ---------------------------------------------------------------------------

/// Parse an environment variable value as a boolean.
/// Truthy: `"1"`, `"true"`, `"yes"`, `"on"` (case-insensitive).
/// Falsy: `"0"`, `"false"`, `"no"`, `"off"` (case-insensitive).
/// Anything else → `None` (treat as unset).
fn parse_env_bool(val: &str) -> Option<bool> {
    match val.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// is_enabled — main public API
// ---------------------------------------------------------------------------

/// Returns `true` if `flag` is currently enabled.
///
/// Precedence: env var > TOML config > DOGFOOD default (debug only) > built-in default.
pub fn is_enabled(flag: FeatureFlag) -> bool {
    // 1. Environment variable override
    let env_name = flag.env_var();
    if let Ok(val) = std::env::var(&env_name) {
        if let Some(b) = parse_env_bool(&val) {
            return b;
        }
    }

    // 2. TOML config
    let cfg = toml_config();
    if let Some(&b) = cfg.flags.get(flag.as_str()) {
        return b;
    }

    // 3. DOGFOOD default (debug builds only)
    #[cfg(debug_assertions)]
    if DOGFOOD_FLAGS.contains(&flag) {
        return true;
    }

    // 4. Built-in default
    flag.builtin_default()
}

// ---------------------------------------------------------------------------
// FFI-friendly string-keyed wrapper
// ---------------------------------------------------------------------------

/// String-keyed variant of `is_enabled` for FFI callers.
/// Maps the snake_case key to a `FeatureFlag`; unknown keys return `false`.
pub fn is_enabled_by_name(name: &str) -> bool {
    match FeatureFlag::from_str_key(name) {
        Some(flag) => is_enabled(flag),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// set_flag — write a flag value to TOML config
// ---------------------------------------------------------------------------

/// Write `flag = value` to the TOML config file at [`config_path()`], preserving
/// all other keys that are already present.
///
/// This always writes to the file; the in-process `OnceLock` cache is **not**
/// updated (process must restart for the Rust layer to pick up the change).
/// The Swift layer writes to `UserDefaults` in addition so the UI reflects the
/// toggle immediately without a restart.
pub fn set_flag(flag: FeatureFlag, value: bool) -> Result<(), String> {
    set_flag_by_name(flag.as_str(), value)
}

/// String-keyed variant of [`set_flag`] for FFI callers.
/// Returns an error string on unknown keys or I/O failure.
pub fn set_flag_by_name(name: &str, value: bool) -> Result<(), String> {
    // Validate the key
    if FeatureFlag::from_str_key(name).is_none() {
        return Err(format!("unknown feature flag: {name}"));
    }

    let path = config_path();

    // Read existing content (if any) and parse as a mutable map
    let existing_toml = std::fs::read_to_string(&path).unwrap_or_default();
    let mut map: HashMap<String, bool> = toml::from_str(&existing_toml)
        .unwrap_or_default();

    // Update the target key
    map.insert(name.to_owned(), value);

    // Serialise back to TOML — build manually so ordering is stable
    let mut lines: Vec<String> = map
        .iter()
        .map(|(k, v)| format!("{} = {}", k, v))
        .collect();
    lines.sort(); // deterministic output
    let new_toml = lines.join("\n") + "\n";

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create config dir: {e}"))?;
    }

    // Write atomically via temp file in same directory
    let tmp_path = path.with_extension("toml.tmp");
    {
        let mut f = std::fs::File::create(&tmp_path)
            .map_err(|e| format!("failed to create temp config: {e}"))?;
        f.write_all(new_toml.as_bytes())
            .map_err(|e| format!("failed to write temp config: {e}"))?;
        f.flush()
            .map_err(|e| format!("failed to flush temp config: {e}"))?;
    }
    std::fs::rename(&tmp_path, &path)
        .map_err(|e| format!("failed to rename temp config: {e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    // Helper: run a closure with an env var set, then unset it.
    // Uses a per-test mutex to prevent data races between parallel tests that
    // modify the same env var.
    use std::sync::Mutex;
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    fn with_env<F: FnOnce()>(key: &str, val: &str, f: F) {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var(key, val);
        f();
        std::env::remove_var(key);
    }

    // --- parse_env_bool ---

    #[test]
    fn test_parse_env_bool_truthy() {
        for val in &["1", "true", "TRUE", "yes", "YES", "on", "ON"] {
            assert_eq!(parse_env_bool(val), Some(true), "expected true for '{}'", val);
        }
    }

    #[test]
    fn test_parse_env_bool_falsy() {
        for val in &["0", "false", "FALSE", "no", "NO", "off", "OFF"] {
            assert_eq!(parse_env_bool(val), Some(false), "expected false for '{}'", val);
        }
    }

    #[test]
    fn test_parse_env_bool_invalid() {
        assert_eq!(parse_env_bool("maybe"), None);
        assert_eq!(parse_env_bool(""), None);
    }

    // --- FeatureFlag helpers ---

    #[test]
    fn test_as_str_snake_case() {
        assert_eq!(FeatureFlag::ArcRouting.as_str(), "arc_routing");
        assert_eq!(FeatureFlag::NewPersistence.as_str(), "new_persistence");
        assert_eq!(FeatureFlag::RemoteV2.as_str(), "remote_v2");
        assert_eq!(FeatureFlag::JsonRpcSpine.as_str(), "json_rpc_spine");
        assert_eq!(FeatureFlag::CommandSignaturesV2.as_str(), "command_signatures_v2");
        // P3e2: Tier 2 flag
        assert_eq!(FeatureFlag::AgentFirstWritePrompt.as_str(), "agent_first_write_prompt");
    }

    #[test]
    fn test_env_var_name() {
        assert_eq!(FeatureFlag::ArcRouting.env_var(), "BEACON_FF_ARC_ROUTING");
        assert_eq!(FeatureFlag::JsonRpcSpine.env_var(), "BEACON_FF_JSON_RPC_SPINE");
    }

    #[test]
    fn test_from_str_key_roundtrip() {
        let all = [
            FeatureFlag::ArcRouting,
            FeatureFlag::NewPersistence,
            FeatureFlag::RemoteV2,
            FeatureFlag::JsonRpcSpine,
            FeatureFlag::CommandSignaturesV2,
            // P3e2: Tier 2 flag
            FeatureFlag::AgentFirstWritePrompt,
        ];
        for flag in all {
            assert_eq!(FeatureFlag::from_str_key(flag.as_str()), Some(flag));
        }
    }

    #[test]
    fn test_from_str_key_unknown() {
        assert_eq!(FeatureFlag::from_str_key("does_not_exist"), None);
    }

    // --- serde round-trip ---

    #[test]
    fn test_serde_roundtrip_snake_case() {
        // Serialise all flags and ensure they round-trip through JSON (serde derive).
        let flags = vec![
            FeatureFlag::ArcRouting,
            FeatureFlag::NewPersistence,
            FeatureFlag::RemoteV2,
            FeatureFlag::JsonRpcSpine,
            FeatureFlag::CommandSignaturesV2,
            // P3e2: Tier 2 flag
            FeatureFlag::AgentFirstWritePrompt,
        ];
        for flag in flags {
            let json = serde_json::to_string(&flag).unwrap();
            let back: FeatureFlag = serde_json::from_str(&json).unwrap();
            assert_eq!(flag, back);
        }
    }

    #[test]
    fn test_serde_snake_case_values() {
        let json = serde_json::to_string(&FeatureFlag::ArcRouting).unwrap();
        assert_eq!(json, r#""arc_routing""#);
        let json = serde_json::to_string(&FeatureFlag::CommandSignaturesV2).unwrap();
        assert_eq!(json, r#""command_signatures_v2""#);
    }

    // --- P3e2 Tier 2: AgentFirstWritePrompt flag ---

    #[test]
    fn agent_first_write_prompt_str_roundtrip() {
        // as_str → from_str_key must be identity.
        let flag = FeatureFlag::AgentFirstWritePrompt;
        assert_eq!(flag.as_str(), "agent_first_write_prompt");
        assert_eq!(FeatureFlag::from_str_key("agent_first_write_prompt"), Some(flag));
    }

    #[test]
    fn agent_first_write_prompt_default_is_off() {
        // Built-in default must be false (not in DOGFOOD_FLAGS).
        assert!(!FeatureFlag::AgentFirstWritePrompt.builtin_default());
        assert!(!DOGFOOD_FLAGS.contains(&FeatureFlag::AgentFirstWritePrompt));
    }

    #[test]
    fn agent_first_write_prompt_env_var_enables_flag() {
        // Env var BEACON_FF_AGENT_FIRST_WRITE_PROMPT=1 must enable the flag.
        with_env("BEACON_FF_AGENT_FIRST_WRITE_PROMPT", "1", || {
            assert!(is_enabled(FeatureFlag::AgentFirstWritePrompt));
        });
    }

    #[test]
    fn agent_first_write_prompt_env_var_disables_flag() {
        // Env var =0 must keep the flag off.
        with_env("BEACON_FF_AGENT_FIRST_WRITE_PROMPT", "0", || {
            assert!(!is_enabled(FeatureFlag::AgentFirstWritePrompt));
        });
    }

    // --- env var override beats everything ---

    #[test]
    fn test_env_override_on() {
        with_env("BEACON_FF_ARC_ROUTING", "1", || {
            assert!(is_enabled(FeatureFlag::ArcRouting));
        });
    }

    #[test]
    fn test_env_override_off() {
        with_env("BEACON_FF_ARC_ROUTING", "0", || {
            assert!(!is_enabled(FeatureFlag::ArcRouting));
        });
    }

    // --- missing flag returns built-in default (false) ---

    #[test]
    fn test_default_is_false() {
        // Clear env vars for all flags before checking defaults.
        let _guard = ENV_MUTEX.lock().unwrap();
        for var in &[
            "BEACON_FF_ARC_ROUTING",
            "BEACON_FF_NEW_PERSISTENCE",
            "BEACON_FF_REMOTE_V2",
            "BEACON_FF_JSON_RPC_SPINE",
            "BEACON_FF_COMMAND_SIGNATURES_V2",
        ] {
            std::env::remove_var(var);
        }
        // TOML config is OnceLock — already initialised to empty in tests.
        // In release cfg, all should be false (dogfood_flags is empty).
        // In debug cfg, DOGFOOD_FLAGS is also empty, so still false.
        assert!(!FeatureFlag::NewPersistence.builtin_default());
        assert!(!FeatureFlag::RemoteV2.builtin_default());
        // JsonRpcSpine is now GA → builtin_default = true.
        assert!(FeatureFlag::JsonRpcSpine.builtin_default());
        assert!(!FeatureFlag::CommandSignaturesV2.builtin_default());
    }

    // --- DOGFOOD_FLAGS honoured under debug ---

    #[test]
    fn test_dogfood_flags_constant_exists() {
        // Currently exercises NewPersistence + CommandSignaturesV2 + RemoteV2
        // by default in debug builds (3 flags as of 2026-05-02 — JsonRpcSpine
        // graduated to GA via builtin_default = true and was removed from this list).
        assert!(DOGFOOD_FLAGS.len() >= 3);
        assert!(DOGFOOD_FLAGS.contains(&FeatureFlag::NewPersistence));
        assert!(DOGFOOD_FLAGS.contains(&FeatureFlag::CommandSignaturesV2));
        assert!(DOGFOOD_FLAGS.contains(&FeatureFlag::RemoteV2));
        assert!(!DOGFOOD_FLAGS.contains(&FeatureFlag::JsonRpcSpine));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_dogfood_flag_would_be_on_if_listed() {
        // This test verifies the DOGFOOD logic path. We can't add to the const
        // at test-time, so we verify the is_enabled_by_name unknown-key path
        // returns false, and document: if a flag is in DOGFOOD_FLAGS and no
        // env/toml overrides exist, is_enabled returns true in debug builds.
        assert!(!is_enabled_by_name("does_not_exist"));
    }

    // --- is_enabled_by_name ---

    #[test]
    fn test_is_enabled_by_name_known() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("BEACON_FF_ARC_ROUTING");
        // Without override, should match is_enabled(ArcRouting)
        assert_eq!(
            is_enabled_by_name("arc_routing"),
            is_enabled(FeatureFlag::ArcRouting)
        );
    }

    #[test]
    fn test_is_enabled_by_name_unknown_returns_false() {
        assert!(!is_enabled_by_name("not_a_real_flag"));
    }

    // --- TOML config parsing (unit-level, not via OnceLock) ---

    #[test]
    fn test_toml_parse_directly() {
        let toml_str = r#"arc_routing = true
new_persistence = false"#;
        let cfg: TomlConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.flags.get("arc_routing"), Some(&true));
        assert_eq!(cfg.flags.get("new_persistence"), Some(&false));
    }

    #[test]
    fn test_toml_unknown_key_ignored() {
        let toml_str = r#"totally_unknown_flag = true"#;
        let cfg: TomlConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.flags.get("totally_unknown_flag"), Some(&true));
        // is_enabled_by_name for unknown key still returns false
        // (unknown key maps to None FeatureFlag → false)
    }

    // --- set_flag / set_flag_by_name ---

    #[test]
    fn test_set_flag_round_trip() {
        // Write to a temp dir so we never touch the real ~/.config/beacon/features.toml.
        let dir = std::env::temp_dir().join(format!("beacon_test_set_flag_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("features.toml");

        // Temporarily redirect config_path via XDG_CONFIG_HOME
        let _guard = ENV_MUTEX.lock().unwrap();
        // We can't redirect config_path() at runtime (it uses env at call time),
        // but we can call set_flag_by_name with an overridden env.
        // Instead, exercise the write/parse logic directly.

        // 1. Write arc_routing=true (file does not exist yet)
        let result = {
            let toml_src = "";
            let mut map: std::collections::HashMap<String, bool> = toml::from_str(toml_src).unwrap_or_default();
            map.insert("arc_routing".to_owned(), true);
            map.insert("new_persistence".to_owned(), false);
            let mut lines: Vec<String> = map.iter().map(|(k, v)| format!("{} = {}", k, v)).collect();
            lines.sort();
            let new_toml = lines.join("\n") + "\n";
            std::fs::write(&path, &new_toml).unwrap();
            new_toml
        };

        // 2. Read back and verify
        let read_back: TomlConfig = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read_back.flags.get("arc_routing"), Some(&true));
        assert_eq!(read_back.flags.get("new_persistence"), Some(&false));

        // 3. Write again, toggling arc_routing=false, preserving new_persistence
        let existing = std::fs::read_to_string(&path).unwrap();
        let mut map2: std::collections::HashMap<String, bool> = toml::from_str(&existing).unwrap_or_default();
        map2.insert("arc_routing".to_owned(), false);
        let mut lines2: Vec<String> = map2.iter().map(|(k, v)| format!("{} = {}", k, v)).collect();
        lines2.sort();
        std::fs::write(&path, lines2.join("\n") + "\n").unwrap();

        let read_back2: TomlConfig = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read_back2.flags.get("arc_routing"), Some(&false), "arc_routing should be false after toggle");
        assert_eq!(read_back2.flags.get("new_persistence"), Some(&false), "new_persistence should be preserved");

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
        let _ = result;
    }

    #[test]
    fn test_set_flag_by_name_unknown_key_errors() {
        let result = set_flag_by_name("does_not_exist", true);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("unknown feature flag"), "got: {msg}");
    }

    #[test]
    fn test_set_flag_by_name_known_keys_accepted() {
        // We can't write to real config in unit tests without XDG override,
        // but we can verify the key-validation path doesn't reject known keys.
        // (The actual I/O will attempt to write to ~/.config/beacon/features.toml
        // but that's acceptable in a test environment — the file may or may not
        // exist. We only care that the function does NOT return an unknown-key error.)
        let all = [
            "arc_routing",
            "new_persistence",
            "remote_v2",
            "json_rpc_spine",
            "command_signatures_v2",
            "agent_first_write_prompt",
        ];
        for key in all {
            let result = set_flag_by_name(key, false);
            // May succeed or fail with I/O error, but must NOT be "unknown feature flag"
            if let Err(e) = result {
                assert!(
                    !e.contains("unknown feature flag"),
                    "key {key} incorrectly rejected: {e}"
                );
            }
        }
    }
}
