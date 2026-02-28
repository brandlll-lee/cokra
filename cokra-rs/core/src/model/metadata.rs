//! Model metadata manager.
//!
//! Pulls model metadata from models.dev and stores a local cache.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;

use super::error::{ModelError, Result};

/// Interleaved reasoning config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterleavedConfig {
  pub field: String,
}

/// Model capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
  pub attachment: bool,
  pub reasoning: bool,
  pub temperature: bool,
  pub tool_call: bool,
  #[serde(default)]
  pub interleaved: Option<InterleavedConfig>,
}

/// Model pricing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCost {
  pub input: f64,
  pub output: f64,
  #[serde(default)]
  pub cache_read: Option<f64>,
  #[serde(default)]
  pub cache_write: Option<f64>,
}

/// Token/context limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelLimit {
  pub context: usize,
  #[serde(default)]
  pub input: Option<usize>,
  pub output: usize,
}

/// IO modalities.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Modalities {
  #[serde(default)]
  pub input: Vec<String>,
  #[serde(default)]
  pub output: Vec<String>,
}

/// Model lifecycle status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelStatus {
  Alpha,
  Beta,
  Deprecated,
}

/// Unified metadata record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetadata {
  pub id: String,
  pub name: String,
  #[serde(default)]
  pub family: Option<String>,
  pub release_date: String,
  pub capabilities: ModelCapabilities,
  #[serde(default)]
  pub cost: Option<ModelCost>,
  pub limit: ModelLimit,
  #[serde(default)]
  pub modalities: Modalities,
  #[serde(default)]
  pub status: Option<ModelStatus>,
}

/// Raw models.dev provider section.
#[derive(Debug, Clone, Deserialize)]
struct ProviderData {
  #[serde(default)]
  models: HashMap<String, ModelsDevModel>,
}

/// Raw models.dev model entry.
#[derive(Debug, Clone, Deserialize)]
struct ModelsDevModel {
  #[serde(default)]
  name: String,
  #[serde(default)]
  family: Option<String>,
  release_date: String,
  #[serde(default)]
  attachment: bool,
  #[serde(default)]
  reasoning: bool,
  #[serde(default)]
  temperature: bool,
  #[serde(default)]
  tool_call: bool,
  #[serde(default)]
  interleaved: Option<serde_json::Value>,
  #[serde(default)]
  cost: Option<ModelCost>,
  limit: ModelLimit,
  #[serde(default)]
  modalities: Option<Modalities>,
  #[serde(default)]
  status: Option<ModelStatus>,
}

/// Local metadata cache format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetadataCache {
  models: HashMap<String, ModelMetadata>,
  saved_at_unix: u64,
}

/// Model metadata manager.
pub struct ModelMetadataManager {
  cache: Arc<RwLock<HashMap<String, ModelMetadata>>>,
  cache_path: PathBuf,
  refresh_interval: Duration,
  last_refresh: Arc<RwLock<Option<SystemTime>>>,
}

impl ModelMetadataManager {
  /// Creates a manager at `<cache_dir>/models.json`.
  pub fn new(cache_dir: &Path) -> Self {
    Self {
      cache: Arc::new(RwLock::new(HashMap::new())),
      cache_path: cache_dir.join("models.json"),
      refresh_interval: Duration::from_secs(3600),
      last_refresh: Arc::new(RwLock::new(None)),
    }
  }

  /// Overrides the refresh interval.
  pub fn with_refresh_interval(mut self, interval: Duration) -> Self {
    self.refresh_interval = interval;
    self
  }

  /// Loads cache from disk if present.
  pub async fn load_cache(&self) -> Result<()> {
    if !self.cache_path.exists() {
      return Ok(());
    }

    let data = tokio::fs::read_to_string(&self.cache_path)
      .await
      .map_err(|e| ModelError::ApiError(format!("failed to read metadata cache: {e}")))?;
    let parsed = serde_json::from_str::<MetadataCache>(&data)
      .map_err(|e| ModelError::InvalidResponse(format!("failed to parse metadata cache: {e}")))?;
    *self.cache.write().await = parsed.models;
    *self.last_refresh.write().await = Some(SystemTime::now());
    Ok(())
  }

  /// Saves cache to disk.
  pub async fn save_cache(&self) -> Result<()> {
    if let Some(parent) = self.cache_path.parent() {
      tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| ModelError::ApiError(format!("failed to create metadata cache dir: {e}")))?;
    }
    let payload = MetadataCache {
      models: self.cache.read().await.clone(),
      saved_at_unix: chrono::Utc::now().timestamp() as u64,
    };
    let text = serde_json::to_string_pretty(&payload).map_err(|e| {
      ModelError::InvalidResponse(format!("failed to serialize metadata cache: {e}"))
    })?;
    tokio::fs::write(&self.cache_path, text)
      .await
      .map_err(|e| ModelError::ApiError(format!("failed to write metadata cache: {e}")))?;
    Ok(())
  }

  /// Refreshes cache from models.dev.
  pub async fn refresh(&self) -> Result<()> {
    let client = reqwest::Client::builder()
      .timeout(Duration::from_secs(10))
      .build()
      .map_err(ModelError::NetworkError)?;
    let response = client
      .get("https://models.dev/api.json")
      .send()
      .await
      .map_err(ModelError::NetworkError)?;
    if !response.status().is_success() {
      return Err(ModelError::ApiError(format!(
        "failed to fetch models.dev: HTTP {}",
        response.status()
      )));
    }
    let text = response.text().await.map_err(ModelError::NetworkError)?;
    self.apply_models_dev_payload(&text).await
  }

  /// Returns one metadata record.
  pub async fn get(&self, model_id: &str) -> Option<ModelMetadata> {
    self.cache.read().await.get(model_id).cloned()
  }

  /// Lists all cached records.
  pub async fn list(&self) -> Vec<ModelMetadata> {
    self.cache.read().await.values().cloned().collect()
  }

  /// Ensures metadata is loaded and refreshed when stale.
  pub async fn ensure_fresh(&self) -> Result<()> {
    if self.cache.read().await.is_empty() {
      self.load_cache().await?;
    }

    let should_refresh = match *self.last_refresh.read().await {
      Some(last) => last
        .elapsed()
        .map(|elapsed| elapsed > self.refresh_interval)
        .unwrap_or(true),
      None => true,
    };
    if should_refresh {
      self.refresh().await?;
    }
    Ok(())
  }

  async fn apply_models_dev_payload(&self, json_payload: &str) -> Result<()> {
    let parsed =
      serde_json::from_str::<HashMap<String, ProviderData>>(json_payload).map_err(|e| {
        ModelError::InvalidResponse(format!("failed to parse models.dev payload: {e}"))
      })?;

    let mut next = HashMap::new();
    for (provider_id, provider_data) in parsed {
      for (model_id, model) in provider_data.models {
        let full_id = format!("{provider_id}/{model_id}");
        next.insert(full_id.clone(), to_metadata(full_id, model));
      }
    }

    *self.cache.write().await = next;
    *self.last_refresh.write().await = Some(SystemTime::now());
    self.save_cache().await?;
    Ok(())
  }
}

fn to_metadata(id: String, model: ModelsDevModel) -> ModelMetadata {
  let interleaved = model.interleaved.and_then(|v| match v {
    serde_json::Value::Object(map) => {
      map
        .get("field")
        .and_then(serde_json::Value::as_str)
        .map(|field| InterleavedConfig {
          field: field.to_string(),
        })
    }
    _ => None,
  });

  ModelMetadata {
    id,
    name: model.name,
    family: model.family,
    release_date: model.release_date,
    capabilities: ModelCapabilities {
      attachment: model.attachment,
      reasoning: model.reasoning,
      temperature: model.temperature,
      tool_call: model.tool_call,
      interleaved,
    },
    cost: model.cost,
    limit: model.limit,
    modalities: model.modalities.unwrap_or_default(),
    status: model.status,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn test_apply_models_dev_payload() {
    let cache_dir = std::env::temp_dir().join(format!("cokra-metadata-{}", uuid::Uuid::new_v4()));
    let manager = ModelMetadataManager::new(&cache_dir);

    let payload = r#"{
      "openai": {
        "models": {
          "gpt-4o": {
            "name": "GPT-4o",
            "release_date": "2024-05-13",
            "attachment": true,
            "reasoning": true,
            "temperature": true,
            "tool_call": true,
            "limit": {
              "context": 128000,
              "output": 16384
            },
            "modalities": {
              "input": ["text", "image"],
              "output": ["text"]
            }
          }
        }
      }
    }"#;

    let applied = manager.apply_models_dev_payload(payload).await;
    assert!(applied.is_ok());

    let model = manager.get("openai/gpt-4o").await;
    assert!(model.is_some());
    let model = model.expect("metadata");
    assert_eq!(model.name, "GPT-4o");
    assert_eq!(model.limit.context, 128000);
    assert!(model.capabilities.tool_call);
  }
}
