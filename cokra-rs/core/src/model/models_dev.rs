//! models.dev integration — 1:1 opencode ModelsDev pattern
//!
//! Fetches the complete provider+model database from `https://models.dev/api.json`,
//! caches it locally, and provides a unified data source for all providers.
//! This replaces the hardcoded `default_models()` lists in each provider.

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::error::ModelError;
use super::error::Result;

/// 1:1 opencode ModelsDev.Model schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsDevModel {
  pub id: String,
  pub name: String,
  #[serde(default)]
  pub family: Option<String>,
  #[serde(default)]
  pub release_date: String,
  #[serde(default)]
  pub attachment: bool,
  #[serde(default)]
  pub reasoning: bool,
  #[serde(default)]
  pub temperature: bool,
  #[serde(default)]
  pub tool_call: bool,
  #[serde(default)]
  pub cost: Option<ModelsDevCost>,
  #[serde(default)]
  pub limit: Option<ModelsDevLimit>,
  #[serde(default)]
  pub modalities: Option<ModelsDevModalities>,
  #[serde(default)]
  pub status: Option<String>,
  #[serde(default)]
  pub options: Option<serde_json::Value>,
  #[serde(default)]
  pub headers: Option<HashMap<String, String>>,
  #[serde(default)]
  pub provider: Option<ModelsDevModelProvider>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsDevCost {
  #[serde(default)]
  pub input: f64,
  #[serde(default)]
  pub output: f64,
  #[serde(default)]
  pub cache_read: Option<f64>,
  #[serde(default)]
  pub cache_write: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsDevLimit {
  #[serde(default)]
  pub context: u64,
  #[serde(default)]
  pub input: Option<u64>,
  #[serde(default)]
  pub output: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsDevModalities {
  #[serde(default)]
  pub input: Vec<String>,
  #[serde(default)]
  pub output: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsDevModelProvider {
  #[serde(default)]
  pub npm: Option<String>,
}

/// 1:1 opencode ModelsDev.Provider schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsDevProvider {
  pub id: String,
  pub name: String,
  #[serde(default)]
  pub api: Option<String>,
  #[serde(default)]
  pub env: Vec<String>,
  #[serde(default)]
  pub npm: Option<String>,
  #[serde(default)]
  pub models: HashMap<String, ModelsDevModel>,
}

/// The full database fetched from models.dev
pub type ModelsDevDatabase = HashMap<String, ModelsDevProvider>;

const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const CACHE_FILENAME: &str = "cokra-models.json";

/// 1:1 opencode: fetch + cache + refresh pattern for models.dev data
pub struct ModelsDevClient {
  cache_path: PathBuf,
  data: Arc<RwLock<Option<ModelsDevDatabase>>>,
  client: reqwest::Client,
}

impl Default for ModelsDevClient {
  fn default() -> Self {
    Self::new()
  }
}

impl ModelsDevClient {
  pub fn new() -> Self {
    let cache_dir = dirs::cache_dir()
      .unwrap_or_else(|| PathBuf::from("."))
      .join("cokra");
    let cache_path = cache_dir.join(CACHE_FILENAME);

    Self {
      cache_path,
      data: Arc::new(RwLock::new(None)),
      client: reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new()),
    }
  }

  /// 1:1 opencode ModelsDev.Data: try cache first, then fetch from API
  pub async fn get(&self) -> Result<ModelsDevDatabase> {
    // Return cached data if available
    {
      let data = self.data.read().await;
      if let Some(db) = data.as_ref() {
        return Ok(db.clone());
      }
    }

    // Try loading from disk cache
    if let Ok(contents) = tokio::fs::read_to_string(&self.cache_path).await
      && let Ok(db) = serde_json::from_str::<ModelsDevDatabase>(&contents)
      && !db.is_empty()
    {
      let mut data = self.data.write().await;
      *data = Some(db.clone());
      return Ok(db);
    }

    // Fetch from API
    match self.fetch_from_api().await {
      Ok(db) => {
        let mut data = self.data.write().await;
        *data = Some(db.clone());
        // Save to cache in background (best-effort)
        let cache_path = self.cache_path.clone();
        let db_for_save = db.clone();
        tokio::spawn(async move {
          if let Some(parent) = cache_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
          }
          if let Ok(json) = serde_json::to_string(&db_for_save) {
            let _ = tokio::fs::write(&cache_path, json).await;
          }
        });
        Ok(db)
      }
      Err(e) => {
        tracing::warn!("Failed to fetch models.dev: {e}");
        // Return empty database as fallback
        Ok(HashMap::new())
      }
    }
  }

  /// 1:1 opencode ModelsDev.refresh: fetch latest data from API and update cache
  pub async fn refresh(&self) -> Result<()> {
    match self.fetch_from_api().await {
      Ok(db) => {
        let mut data = self.data.write().await;
        *data = Some(db.clone());
        // Save to cache
        if let Some(parent) = self.cache_path.parent() {
          let _ = tokio::fs::create_dir_all(parent).await;
        }
        if let Ok(json) = serde_json::to_string(&db) {
          let _ = tokio::fs::write(&self.cache_path, json).await;
        }
        Ok(())
      }
      Err(e) => {
        tracing::warn!("Failed to refresh models.dev: {e}");
        Err(e)
      }
    }
  }

  async fn fetch_from_api(&self) -> Result<ModelsDevDatabase> {
    let response = self
      .client
      .get(MODELS_DEV_URL)
      .header("User-Agent", "cokra-cli")
      .send()
      .await
      .map_err(ModelError::NetworkError)?;

    if !response.status().is_success() {
      return Err(ModelError::ApiError(format!(
        "models.dev returned HTTP {}",
        response.status()
      )));
    }

    let text = response.text().await.map_err(ModelError::NetworkError)?;

    let db: ModelsDevDatabase = serde_json::from_str(&text)
      .map_err(|e| ModelError::ApiError(format!("JSON parse error: {e}")))?;

    Ok(db)
  }

  /// Get the list of model IDs for a specific provider from the models.dev database.
  /// Returns empty vec if provider not found or database not loaded.
  pub async fn get_provider_models(&self, provider_id: &str) -> Vec<String> {
    match self.get().await {
      Ok(db) => {
        if let Some(provider) = db.get(provider_id) {
          let mut models: Vec<String> = provider.models.keys().cloned().collect();
          models.sort();
          models
        } else {
          Vec::new()
        }
      }
      Err(_) => Vec::new(),
    }
  }

  /// Get all providers with their models from the models.dev database.
  /// 1:1 opencode Provider.list(): returns only providers that have
  /// matching env vars set or are explicitly configured.
  pub async fn list_available_providers(&self) -> Vec<ModelsDevProviderSummary> {
    let db = match self.get().await {
      Ok(db) => db,
      Err(_) => return Vec::new(),
    };

    let mut results = Vec::new();
    for (id, provider) in &db {
      // 1:1 opencode: check if any env var for this provider is set
      let has_auth = provider.env.iter().any(|var| std::env::var(var).is_ok());

      if !has_auth {
        continue;
      }

      let models: Vec<String> = provider.models.keys().cloned().collect();
      results.push(ModelsDevProviderSummary {
        id: id.clone(),
        name: provider.name.clone(),
        api: provider.api.clone(),
        env: provider.env.clone(),
        models,
      });
    }

    // Sort by provider name for consistent ordering
    results.sort_by(|a, b| a.name.cmp(&b.name));
    results
  }
}

/// Summary of a provider from models.dev with its model list
#[derive(Debug, Clone)]
pub struct ModelsDevProviderSummary {
  pub id: String,
  pub name: String,
  pub api: Option<String>,
  pub env: Vec<String>,
  pub models: Vec<String>,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn deserialize_provider_entry() {
    let json = r#"{
      "id": "openai",
      "name": "OpenAI",
      "api": "https://api.openai.com/v1",
      "env": ["OPENAI_API_KEY"],
      "npm": "@ai-sdk/openai",
      "models": {
        "gpt-4o": {
          "id": "gpt-4o",
          "name": "GPT-4o",
          "release_date": "2024-05-13",
          "tool_call": true,
          "temperature": true,
          "reasoning": false,
          "attachment": true,
          "limit": { "context": 128000, "output": 16384 }
        }
      }
    }"#;

    let provider: ModelsDevProvider = serde_json::from_str(json).unwrap();
    assert_eq!(provider.id, "openai");
    assert_eq!(provider.name, "OpenAI");
    assert_eq!(provider.models.len(), 1);
    assert!(provider.models.contains_key("gpt-4o"));
  }

  #[test]
  fn cache_path_is_valid() {
    let client = ModelsDevClient::new();
    assert!(client.cache_path.ends_with(CACHE_FILENAME));
  }
}
