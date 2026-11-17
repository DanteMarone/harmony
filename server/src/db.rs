//! SQLite connection pool and schema migrations.

use std::path::Path;

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

pub async fn open(data_dir: &Path) -> anyhow::Result<SqlitePool> {
    std::fs::create_dir_all(data_dir)?;
    let options = SqliteConnectOptions::new()
        .filename(data_dir.join("harmony.db"))
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}
