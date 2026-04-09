use std::env;

/// Runtime configuration for arc-relay, loaded from environment variables.
#[derive(Debug, Clone)]
pub struct ArcConfig {
    /// ARC_BIND_ADDR — socket address to listen on (default: "0.0.0.0:9900")
    pub bind_addr: String,
    /// ARC_DATABASE_URL — Postgres connection URL (required)
    pub database_url: String,
    /// ARC_JWT_SECRET — HMAC secret for signing JWTs (required)
    pub jwt_secret: String,
    /// ARC_ACCESS_TOKEN_TTL_SECS — access token lifetime in seconds (default: 900)
    pub access_token_ttl_secs: u64,
    /// ARC_REFRESH_TOKEN_TTL_SECS — refresh token lifetime in seconds (default: 2592000 = 30 days)
    pub refresh_token_ttl_secs: u64,
    /// ARC_MAX_SESSIONS_PER_USER — hard cap on live sessions per user (default: 50)
    #[allow(dead_code)]
    pub max_sessions_per_user: u32,
    /// ARC_MAX_BLOCKS_PER_SESSION — hard cap on blocks per session (default: 5000)
    #[allow(dead_code)]
    pub max_blocks_per_session: u32,
}

impl ArcConfig {
    /// Load configuration from environment variables.
    ///
    /// Returns `Err` with a human-readable message if any required variable is absent
    /// or if a value cannot be parsed.
    pub fn from_env() -> Result<Self, String> {
        let database_url = env::var("ARC_DATABASE_URL").map_err(|_| {
            "ARC_DATABASE_URL is required but was not set. \
             Example: postgres://user:pass@localhost:5432/arc"
                .to_string()
        })?;

        let jwt_secret = env::var("ARC_JWT_SECRET").map_err(|_| {
            "ARC_JWT_SECRET is required but was not set. \
             Generate a strong random secret (e.g. `openssl rand -hex 32`)."
                .to_string()
        })?;

        if jwt_secret.len() < 32 {
            return Err(
                "ARC_JWT_SECRET must be at least 32 characters long for security.".to_string(),
            );
        }

        let bind_addr = env::var("ARC_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:9900".to_string());

        let access_token_ttl_secs = parse_env_u64("ARC_ACCESS_TOKEN_TTL_SECS", 900)?;
        let refresh_token_ttl_secs = parse_env_u64("ARC_REFRESH_TOKEN_TTL_SECS", 2_592_000)?;
        let max_sessions_per_user = parse_env_u32("ARC_MAX_SESSIONS_PER_USER", 50)?;
        let max_blocks_per_session = parse_env_u32("ARC_MAX_BLOCKS_PER_SESSION", 5000)?;

        Ok(Self {
            bind_addr,
            database_url,
            jwt_secret,
            access_token_ttl_secs,
            refresh_token_ttl_secs,
            max_sessions_per_user,
            max_blocks_per_session,
        })
    }
}

/// Convenience default used in tests and local dev.
///
/// **Do not use this in production** — `database_url` and `jwt_secret` are
/// placeholder values that will fail at runtime.
impl Default for ArcConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:9900".to_string(),
            database_url: "postgres://arc:arc@localhost:5432/arc".to_string(),
            jwt_secret: "dev-only-secret-do-not-use-in-production-32x".to_string(),
            access_token_ttl_secs: 900,
            refresh_token_ttl_secs: 2_592_000,
            max_sessions_per_user: 50,
            max_blocks_per_session: 5000,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_env_u64(name: &str, default: u64) -> Result<u64, String> {
    match env::var(name) {
        Err(_) => Ok(default),
        Ok(val) => val
            .parse::<u64>()
            .map_err(|_| format!("{name} must be a non-negative integer, got: {val:?}")),
    }
}

fn parse_env_u32(name: &str, default: u32) -> Result<u32, String> {
    match env::var(name) {
        Err(_) => Ok(default),
        Ok(val) => val
            .parse::<u32>()
            .map_err(|_| format!("{name} must be a non-negative integer, got: {val:?}")),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Global lock that serializes all tests that touch environment variables.
    ///
    /// `std::env::set_var` / `remove_var` are not thread-safe; without this
    /// guard concurrent tests race and see each other's env mutations.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Run `f` while holding `ENV_MUTEX`, with `vars` set in the environment.
    /// Cleans up the vars and releases the lock even if `f` panics.
    fn with_env<F: FnOnce() -> R, R>(vars: &[(&str, &str)], f: F) -> R {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        for (k, v) in vars {
            // SAFETY: we hold ENV_MUTEX so no other thread is touching env vars.
            unsafe { env::set_var(k, v) };
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        for (k, _) in vars {
            unsafe { env::remove_var(k) };
        }
        // _guard drops here, releasing the mutex.
        match result {
            Ok(r) => r,
            Err(e) => std::panic::resume_unwind(e),
        }
    }

    #[test]
    fn from_env_all_vars_set_succeeds() {
        with_env(
            &[
                ("ARC_DATABASE_URL", "postgres://u:p@localhost/arc"),
                (
                    "ARC_JWT_SECRET",
                    "a-very-long-secret-that-is-definitely-32-chars",
                ),
                ("ARC_BIND_ADDR", "127.0.0.1:8080"),
                ("ARC_ACCESS_TOKEN_TTL_SECS", "1800"),
                ("ARC_REFRESH_TOKEN_TTL_SECS", "86400"),
                ("ARC_MAX_SESSIONS_PER_USER", "10"),
                ("ARC_MAX_BLOCKS_PER_SESSION", "100"),
            ],
            || {
                let cfg = ArcConfig::from_env().expect("should succeed with all vars set");
                assert_eq!(cfg.bind_addr, "127.0.0.1:8080");
                assert_eq!(cfg.database_url, "postgres://u:p@localhost/arc");
                assert_eq!(
                    cfg.jwt_secret,
                    "a-very-long-secret-that-is-definitely-32-chars"
                );
                assert_eq!(cfg.access_token_ttl_secs, 1800);
                assert_eq!(cfg.refresh_token_ttl_secs, 86400);
                assert_eq!(cfg.max_sessions_per_user, 10);
                assert_eq!(cfg.max_blocks_per_session, 100);
            },
        );
    }

    #[test]
    fn from_env_missing_database_url_returns_error() {
        // Hold the env mutex before touching env vars.
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // Make sure ARC_DATABASE_URL is absent (it might be set in outer env).
        unsafe {
            env::remove_var("ARC_DATABASE_URL");
            env::remove_var("ARC_JWT_SECRET");
        }

        let err = ArcConfig::from_env().expect_err("should fail without DATABASE_URL");
        assert!(
            err.contains("ARC_DATABASE_URL"),
            "error should mention ARC_DATABASE_URL, got: {err}"
        );
    }

    #[test]
    fn from_env_missing_jwt_secret_returns_error() {
        with_env(
            &[("ARC_DATABASE_URL", "postgres://u:p@localhost/arc")],
            || {
                // with_env already holds the mutex; safe to call remove_var here.
                unsafe { env::remove_var("ARC_JWT_SECRET") };
                let err = ArcConfig::from_env().expect_err("should fail without JWT_SECRET");
                assert!(
                    err.contains("ARC_JWT_SECRET"),
                    "error should mention ARC_JWT_SECRET, got: {err}"
                );
            },
        );
    }

    #[test]
    fn from_env_short_jwt_secret_returns_error() {
        with_env(
            &[
                ("ARC_DATABASE_URL", "postgres://u:p@localhost/arc"),
                ("ARC_JWT_SECRET", "too-short"),
            ],
            || {
                let err = ArcConfig::from_env().expect_err("should fail with short JWT secret");
                assert!(
                    err.contains("32 characters"),
                    "error should mention minimum length, got: {err}"
                );
            },
        );
    }

    #[test]
    fn from_env_defaults_applied_when_optional_vars_absent() {
        with_env(
            &[
                ("ARC_DATABASE_URL", "postgres://u:p@localhost/arc"),
                (
                    "ARC_JWT_SECRET",
                    "a-very-long-secret-that-is-definitely-32-chars",
                ),
            ],
            || {
                // Remove optional vars so defaults kick in.
                // with_env already holds ENV_MUTEX, so these are safe.
                unsafe {
                    env::remove_var("ARC_BIND_ADDR");
                    env::remove_var("ARC_ACCESS_TOKEN_TTL_SECS");
                    env::remove_var("ARC_REFRESH_TOKEN_TTL_SECS");
                    env::remove_var("ARC_MAX_SESSIONS_PER_USER");
                    env::remove_var("ARC_MAX_BLOCKS_PER_SESSION");
                }

                let cfg = ArcConfig::from_env().expect("should succeed with defaults");
                assert_eq!(cfg.bind_addr, "0.0.0.0:9900");
                assert_eq!(cfg.access_token_ttl_secs, 900);
                assert_eq!(cfg.refresh_token_ttl_secs, 2_592_000);
                assert_eq!(cfg.max_sessions_per_user, 50);
                assert_eq!(cfg.max_blocks_per_session, 5000);
            },
        );
    }

    #[test]
    fn default_config_values_are_correct() {
        let cfg = ArcConfig::default();
        assert_eq!(cfg.bind_addr, "0.0.0.0:9900");
        assert_eq!(cfg.access_token_ttl_secs, 900);
        assert_eq!(cfg.refresh_token_ttl_secs, 2_592_000);
        assert_eq!(cfg.max_sessions_per_user, 50);
        assert_eq!(cfg.max_blocks_per_session, 5000);
    }
}
