use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// Create a connection pool for the given Postgres URL.
pub async fn init_pool(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(20)
        .connect(database_url)
        .await
}

/// Run embedded migrations from the `./migrations` directory.
///
/// Migration files are embedded into the binary at compile time by
/// `sqlx::migrate!`. The macro only needs `DATABASE_URL` at *runtime*,
/// not at compile time (offline mode is not required here).
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}
