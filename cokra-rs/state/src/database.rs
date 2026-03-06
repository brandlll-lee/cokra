use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::Row;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;

#[derive(Clone)]
pub struct StateDb {
  pool: SqlitePool,
}

impl StateDb {
  pub async fn new(path: impl AsRef<Path>) -> Result<Self> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
      tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("failed to create state db parent: {}", parent.display()))?;
    }
    let options = SqliteConnectOptions::new()
      .filename(path)
      .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
      .max_connections(1)
      .connect_with(options)
      .await
      .with_context(|| format!("failed to open state db: {}", path.display()))?;
    let db = Self { pool };
    db.init().await?;
    Ok(db)
  }

  pub fn default_path_for(base_dir: impl AsRef<Path>) -> PathBuf {
    base_dir.as_ref().join(".cokra").join("state.db")
  }

  pub async fn load_json<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
    let row = sqlx::query("SELECT payload FROM team_state WHERE scope_key = ?")
      .bind(key)
      .fetch_optional(&self.pool)
      .await
      .with_context(|| format!("failed to load state for key {key}"))?;
    let Some(row) = row else {
      return Ok(None);
    };
    let payload: String = row.try_get("payload")?;
    Ok(Some(serde_json::from_str(&payload).with_context(|| {
      format!("failed to decode stored team state for key {key}")
    })?))
  }

  pub async fn save_json<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
    let payload = serde_json::to_string(value)
      .with_context(|| format!("failed to encode team state for key {key}"))?;
    let updated_at = chrono::Utc::now().timestamp();
    sqlx::query(
      "INSERT INTO team_state (scope_key, payload, updated_at) VALUES (?, ?, ?) \
       ON CONFLICT(scope_key) DO UPDATE SET payload = excluded.payload, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(payload)
    .bind(updated_at)
    .execute(&self.pool)
    .await
    .with_context(|| format!("failed to save state for key {key}"))?;
    Ok(())
  }

  pub async fn delete(&self, key: &str) -> Result<()> {
    sqlx::query("DELETE FROM team_state WHERE scope_key = ?")
      .bind(key)
      .execute(&self.pool)
      .await
      .with_context(|| format!("failed to delete state for key {key}"))?;
    Ok(())
  }

  async fn init(&self) -> Result<()> {
    sqlx::query(
      "CREATE TABLE IF NOT EXISTS team_state (
         scope_key TEXT PRIMARY KEY,
         payload TEXT NOT NULL,
         updated_at INTEGER NOT NULL
       )",
    )
    .execute(&self.pool)
    .await
    .context("failed to initialize team_state table")?;
    Ok(())
  }
}
