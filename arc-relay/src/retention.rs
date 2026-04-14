use sqlx::PgPool;
use std::time::Duration;
use tokio::time;
use tracing::{error, info};

const RETENTION_DAYS: i64 = 90;
const PURGE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60); // daily

/// Starts the retention purge background task.
/// Runs daily and removes unpinned blocks older than RETENTION_DAYS.
pub fn spawn_retention_task(pool: PgPool) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(PURGE_INTERVAL);
        // tick() fires immediately on first call; skip to run after first PURGE_INTERVAL
        interval.tick().await;
        loop {
            interval.tick().await;
            match purge_expired_blocks(&pool).await {
                Ok(deleted) => {
                    info!(
                        deleted,
                        "retention purge: removed {} unpinned blocks older than {} days",
                        deleted,
                        RETENTION_DAYS
                    );
                }
                Err(e) => {
                    error!(error = %e, "retention purge failed");
                }
            }
        }
    })
}

/// Delete unpinned blocks older than RETENTION_DAYS. Returns the number of rows deleted.
pub async fn purge_expired_blocks(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM blocks \
         WHERE NOT pinned \
           AND created_at < NOW() - ($1 || ' days')::interval",
    )
    .bind(RETENTION_DAYS.to_string())
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
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
}
