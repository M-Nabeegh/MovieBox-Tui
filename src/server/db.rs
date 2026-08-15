use std::str::FromStr;
use std::time::Duration;

use sqlx::{
    SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub async fn connect(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(30))
        .synchronous(SqliteSynchronous::Normal);
    let pool = SqlitePoolOptions::new()
        // MovieBox is a single-instance server; serialize SQLite work to avoid
        // competing writers while the download worker records progress.
        .max_connections(1)
        .connect_with(options)
        .await?;
    migrate(&pool).await.map_err(sqlx::Error::from)?;
    Ok(pool)
}

pub async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}
