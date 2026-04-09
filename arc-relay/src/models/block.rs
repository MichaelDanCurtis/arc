use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DbBlock {
    pub id: String,
    pub session_id: String,
    pub tab_id: String,
    pub command: String,
    pub output: Option<Vec<u8>>,
    pub exit_code: Option<i32>,
    pub cwd: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub shadow_git_ref: Option<String>,
    pub pinned: bool,
    pub host_info: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// Persist a beacon_model::Block to Postgres with zstd-compressed output.
#[allow(dead_code)]
pub async fn persist_block(pool: &PgPool, block: &beacon_model::Block) -> Result<(), sqlx::Error> {
    let id = block.header.id.to_string();
    let session_id = block.header.session_id.to_string();
    let tab_id = block.header.tab_id.to_string();
    let command = &block.header.command;
    let cwd = block.header.cwd.to_str().map(|s| s.to_string());
    let started_at: DateTime<Utc> = block.header.started_at.into();
    let host_info = serde_json::to_value(&block.header.host_info).ok();

    let compressed_output: Option<Vec<u8>> = if !block.output.is_empty() {
        Some(zstd::encode_all(block.output.as_slice(), 3).map_err(sqlx::Error::Io)?)
    } else {
        None
    };

    let (exit_code, ended_at, shadow_git_ref) = match &block.footer {
        Some(f) => {
            let ended: DateTime<Utc> = f.ended_at.into();
            let git_ref = f.shadow_git_ref.as_ref().map(|r| r.0.clone());
            (f.exit_code, Some(ended), git_ref)
        }
        None => (None, None, None),
    };

    sqlx::query(
        "INSERT INTO blocks \
         (id, session_id, tab_id, command, output, exit_code, cwd, \
          started_at, ended_at, shadow_git_ref, pinned, host_info) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, FALSE, $11) \
         ON CONFLICT (id) DO UPDATE SET \
           output = EXCLUDED.output, \
           exit_code = EXCLUDED.exit_code, \
           ended_at = EXCLUDED.ended_at, \
           shadow_git_ref = EXCLUDED.shadow_git_ref",
    )
    .bind(&id)
    .bind(&session_id)
    .bind(&tab_id)
    .bind(command)
    .bind(compressed_output)
    .bind(exit_code)
    .bind(cwd)
    .bind(started_at)
    .bind(ended_at)
    .bind(shadow_git_ref)
    .bind(host_info)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get blocks for a session, optionally after a cursor block ID.
pub async fn get_blocks_for_session(
    pool: &PgPool,
    session_id: &str,
    since: Option<&str>,
    limit: i64,
) -> Result<Vec<DbBlock>, sqlx::Error> {
    match since {
        Some(cursor_id) => {
            sqlx::query_as::<_, DbBlock>(
                "SELECT * FROM blocks \
                 WHERE session_id = $1 \
                   AND started_at > (SELECT started_at FROM blocks WHERE id = $2) \
                 ORDER BY started_at ASC \
                 LIMIT $3",
            )
            .bind(session_id)
            .bind(cursor_id)
            .bind(limit)
            .fetch_all(pool)
            .await
        }
        None => {
            sqlx::query_as::<_, DbBlock>(
                "SELECT * FROM blocks \
                 WHERE session_id = $1 \
                 ORDER BY started_at ASC \
                 LIMIT $2",
            )
            .bind(session_id)
            .bind(limit)
            .fetch_all(pool)
            .await
        }
    }
}

/// Count blocks in a session.
#[allow(dead_code)]
pub async fn get_block_count(pool: &PgPool, session_id: &str) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM blocks WHERE session_id = $1")
        .bind(session_id)
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Non-DB: zstd compression/decompression round-trip.
    #[test]
    fn zstd_round_trip() {
        let original = b"hello beacon output\n".repeat(100);
        let compressed = zstd::encode_all(original.as_slice(), 3).expect("compress");
        assert!(compressed.len() < original.len(), "zstd should compress");
        let decompressed = zstd::decode_all(compressed.as_slice()).expect("decompress");
        assert_eq!(decompressed, original);
    }

    /// Non-DB: empty output compresses to None (not stored).
    #[test]
    fn empty_output_skips_compression() {
        // Simulate the logic in persist_block for empty output
        let output: Vec<u8> = vec![];
        let result: Option<Vec<u8>> = if !output.is_empty() {
            Some(zstd::encode_all(output.as_slice(), 3).expect("compress"))
        } else {
            None
        };
        assert!(result.is_none());
    }

    /// Non-DB: compressed output can be decoded back after round-trip through bytes.
    #[test]
    fn zstd_round_trip_binary_output() {
        let original: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
        let compressed = zstd::encode_all(original.as_slice(), 3).expect("compress");
        let decompressed = zstd::decode_all(compressed.as_slice()).expect("decompress");
        assert_eq!(decompressed, original);
    }

    fn get_pool_url() -> Option<String> {
        std::env::var("DATABASE_URL").ok()
    }

    #[tokio::test]
    #[ignore]
    async fn test_persist_and_get_blocks() {
        use beacon_model::{Block, BlockFooter, BlockHeader, BlockId, HostInfo, SessionId, TabId};
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::time::SystemTime;
        use uuid::Uuid;

        let url = match get_pool_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        // Need a user + session for FK constraints.
        let owner_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x')")
            .bind(owner_id)
            .bind(format!("block_owner_{}@test.invalid", owner_id))
            .execute(&pool)
            .await
            .expect("user");

        let session_id_str = format!("blk_sess_{}", Uuid::new_v4().simple());
        sqlx::query(
            "INSERT INTO sessions (id, host_id, owner_id, state, started_at, last_seen_at) \
             VALUES ($1, $2, $3, 'active', NOW(), NOW())",
        )
        .bind(&session_id_str)
        .bind(Uuid::new_v4())
        .bind(owner_id)
        .execute(&pool)
        .await
        .expect("session");

        let sid: SessionId = session_id_str.parse().unwrap_or_else(|_| SessionId::new());
        let header = BlockHeader {
            id: BlockId::new(),
            session_id: sid,
            tab_id: TabId::new(),
            command: "echo hello".to_string(),
            cwd: PathBuf::from("/tmp"),
            started_at: SystemTime::now(),
            env_snapshot: HashMap::new(),
            host_info: HostInfo::current(),
        };
        let block = Block {
            header,
            output: b"hello\n".to_vec(),
            footer: Some(BlockFooter {
                id: BlockId::new(),
                exit_code: Some(0),
                ended_at: SystemTime::now(),
                shadow_git_ref: None,
            }),
        };

        persist_block(&pool, &block).await.expect("persist_block");

        let count = get_block_count(&pool, &session_id_str)
            .await
            .expect("count");
        assert_eq!(count, 1);

        let blocks = get_blocks_for_session(&pool, &session_id_str, None, 100)
            .await
            .expect("get_blocks");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].command, "echo hello");
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_blocks_since_cursor() {
        use beacon_model::{Block, BlockHeader, BlockId, HostInfo, SessionId, TabId};
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::time::{Duration, SystemTime};
        use uuid::Uuid;

        let url = match get_pool_url() {
            Some(u) => u,
            None => return,
        };
        let pool = PgPool::connect(&url).await.expect("connect");

        let owner_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'x')")
            .bind(owner_id)
            .bind(format!("cursor_owner_{}@test.invalid", owner_id))
            .execute(&pool)
            .await
            .expect("user");

        let session_id_str = format!("cur_sess_{}", Uuid::new_v4().simple());
        sqlx::query(
            "INSERT INTO sessions (id, host_id, owner_id, state, started_at, last_seen_at) \
             VALUES ($1, $2, $3, 'active', NOW(), NOW())",
        )
        .bind(&session_id_str)
        .bind(Uuid::new_v4())
        .bind(owner_id)
        .execute(&pool)
        .await
        .expect("session");

        let base_time = SystemTime::now();
        let mut first_block_id = String::new();
        let sid: SessionId = SessionId::new();

        for i in 0..3usize {
            let started = base_time + Duration::from_secs(i as u64);
            let block_id = BlockId::new();
            if i == 0 {
                first_block_id = block_id.to_string();
            }
            let block = Block {
                header: BlockHeader {
                    id: block_id,
                    session_id: sid,
                    tab_id: TabId::new(),
                    command: format!("cmd_{}", i),
                    cwd: PathBuf::from("/tmp"),
                    started_at: started,
                    env_snapshot: HashMap::new(),
                    host_info: HostInfo::current(),
                },
                output: vec![],
                footer: None,
            };
            // Override session_id in the DB insert directly since we need the actual string
            let _ = sqlx::query(
                "INSERT INTO blocks (id, session_id, tab_id, command, started_at) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(block.header.id.to_string())
            .bind(&session_id_str)
            .bind(block.header.tab_id.to_string())
            .bind(&block.header.command)
            .bind(DateTime::<Utc>::from(started))
            .execute(&pool)
            .await
            .expect("insert block");
        }

        let all = get_blocks_for_session(&pool, &session_id_str, None, 100)
            .await
            .expect("all");
        assert_eq!(all.len(), 3);

        let after_first =
            get_blocks_for_session(&pool, &session_id_str, Some(&first_block_id), 100)
                .await
                .expect("since");
        assert_eq!(after_first.len(), 2);
    }
}
