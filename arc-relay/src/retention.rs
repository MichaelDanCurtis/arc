use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time;
use tracing::info;

const RETENTION_DAYS: u64 = 90;
const PURGE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60); // daily

/// Starts the retention purge background task.
/// Runs daily and removes blocks older than RETENTION_DAYS.
pub fn spawn_retention_task() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(PURGE_INTERVAL);
        loop {
            interval.tick().await;
            purge_expired_blocks().await;
        }
    })
}

async fn purge_expired_blocks() {
    let cutoff_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
        - (RETENTION_DAYS * 24 * 60 * 60 * 1000);

    // TODO: When DB is wired, execute:
    // DELETE FROM blocks WHERE created_at_ms < cutoff_ms
    info!(
        cutoff_ms,
        "retention purge: would delete blocks older than {} days", RETENTION_DAYS
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the retention constants are sensible.
    #[test]
    fn retention_constants_are_reasonable() {
        assert_eq!(RETENTION_DAYS, 90);
        assert_eq!(PURGE_INTERVAL, Duration::from_secs(24 * 60 * 60));
    }

    /// Verify that spawn_retention_task returns a JoinHandle (compiles + runs).
    #[tokio::test]
    async fn spawn_retention_task_returns_handle() {
        let handle = spawn_retention_task();
        // The task is running; just confirm we got a handle and can abort it cleanly.
        handle.abort();
        // AbortError is expected when we abort — that's fine.
        let _ = handle.await;
    }

    /// Verify purge_expired_blocks doesn't panic (no DB yet, just logging).
    #[tokio::test]
    async fn purge_expired_blocks_does_not_panic() {
        purge_expired_blocks().await;
    }
}
