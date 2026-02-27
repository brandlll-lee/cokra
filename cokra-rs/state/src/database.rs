// Cokra State Database
// SQLite database implementation

use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};
use anyhow::Result;

/// State database handle
pub struct StateDb {
    pool: SqlitePool,
}

impl StateDb {
    /// Create a new state database
    pub async fn new(db_path: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            .connect()
            .await?;

        // Run migrations
        sqlx::query(include_str!("schema.sql"))
            .execute(&pool)
            .await?;

        Ok(Self { pool })
    }

    /// Close the database
    pub async fn close(self) -> Result<()> {
        self.pool.close().await;
        Ok(())
    }
}
