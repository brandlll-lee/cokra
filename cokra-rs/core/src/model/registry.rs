//! Provider Registry
//!
//! Manages registration and discovery of model providers

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::ModelProvider;
use super::ProviderConfig;
use super::auth::AuthManager;
use super::error::ModelError;
use super::error::Result;
use super::models_dev::ModelsDevClient;
use super::plugin_registry::PluginRegistry;
use super::provider::ProviderInfo;

/// Provider Registry
///
/// A registry for managing multiple LLM providers.
/// Providers can be registered, retrieved, and listed.
pub struct ProviderRegistry {
  providers: RwLock<HashMap<String, Arc<dyn ModelProvider>>>,
  default_provider: RwLock<Option<String>>,
  configs: RwLock<HashMap<String, ProviderConfig>>,
  /// 1:1 opencode: models.dev client for fetching the complete provider+model database
  models_dev: ModelsDevClient,
}

impl Default for ProviderRegistry {
  fn default() -> Self {
    Self::new()
  }
}

impl ProviderRegistry {
  /// Create a new registry
  pub fn new() -> Self {
    Self {
      providers: RwLock::new(HashMap::new()),
      default_provider: RwLock::new(None),
      configs: RwLock::new(HashMap::new()),
      models_dev: ModelsDevClient::new(),
    }
  }

  /// Register a provider
  ///
  /// # Arguments
  /// * `provider` - The provider to register (must implement ModelProvider)
  ///
  /// # Example
  /// ```ignore
  /// let registry = ProviderRegistry::new();
  /// registry.register(OpenAIProvider::new("key".to_string(), None)).await;
  /// ```
  pub async fn register<P: ModelProvider + 'static>(&self, provider: P) {
    let provider_id = provider.provider_id().to_string();
    self
      .providers
      .write()
      .await
      .insert(provider_id.clone(), Arc::new(provider));
  }

  /// Register a provider with configuration
  pub async fn register_with_config<P: ModelProvider + 'static>(
    &self,
    provider: P,
    config: ProviderConfig,
  ) {
    let provider_id = provider.provider_id().to_string();
    self
      .configs
      .write()
      .await
      .insert(provider_id.clone(), config);
    self
      .providers
      .write()
      .await
      .insert(provider_id, Arc::new(provider));
  }

  /// Get a provider by ID
  pub async fn get(&self, provider_id: &str) -> Option<Arc<dyn ModelProvider>> {
    self.providers.read().await.get(provider_id).cloned()
  }

  /// Get the default provider
  pub async fn get_default(&self) -> Result<Arc<dyn ModelProvider>> {
    let default_id = self.default_provider.read().await.clone();

    if let Some(id) = default_id {
      self
        .get(&id)
        .await
        .ok_or_else(|| ModelError::ProviderNotFound(id))
    } else {
      // Try to return the first available provider
      let providers = self.providers.read().await;
      if let Some((_id, provider)) = providers.iter().next() {
        Ok(provider.clone())
      } else {
        Err(ModelError::NoDefaultProvider)
      }
    }
  }

  /// Set the default provider
  pub async fn set_default(&self, provider_id: &str) -> Result<()> {
    // Verify the provider exists
    if !self.providers.read().await.contains_key(provider_id) {
      return Err(ModelError::ProviderNotFound(provider_id.to_string()));
    }

    *self.default_provider.write().await = Some(provider_id.to_string());
    Ok(())
  }

  /// List all registered providers
  pub async fn list_providers(&self) -> Vec<ProviderInfo> {
    let providers = self.providers.read().await;
    let configs = self.configs.read().await;
    providers
      .iter()
      .map(|(id, provider)| {
        let config = configs.get(id);
        let authenticated = config
          .as_ref()
          .and_then(|c| c.api_key.as_ref())
          .map(|k| !k.is_empty())
          .unwrap_or(false);

        ProviderInfo::new(provider.provider_id(), provider.provider_name())
          .env_vars(
            provider
              .required_env_vars()
              .into_iter()
              .map(|s| s.to_string())
              .collect(),
          )
          .authenticated(authenticated)
          .models(
            provider
              .default_models()
              .into_iter()
              .map(|s| s.to_string())
              .collect(),
          )
      })
      .collect()
  }

  /// List all providers with their **live** model lists.
  ///
  /// 1:1 opencode `Provider.list()` pattern:
  /// 1. Fetch the models.dev database (all known providers + models)
  /// 2. For each registered provider, try live `list_models()` API first
  /// 3. Fall back to models.dev data, then static `default_models()`
  /// 4. Also include models.dev providers that have env vars set but aren't
  ///    registered (e.g. mistral, xai, groq, deepinfra, etc.)
  pub async fn list_models_live(&self) -> Vec<ProviderInfo> {
    // 1:1 opencode: fetch models.dev database
    let models_dev_db = self.models_dev.get().await.unwrap_or_default();

    // Snapshot registered providers under the lock, then release before I/O.
    let snapshot: Vec<(String, Arc<dyn ModelProvider>, bool)> = {
      let providers = self.providers.read().await;
      let configs = self.configs.read().await;
      providers
        .iter()
        .map(|(id, provider)| {
          let authenticated = configs
            .get(id)
            .and_then(|c| c.api_key.as_ref())
            .map(|k| !k.is_empty())
            .unwrap_or(false);
          (id.clone(), Arc::clone(provider), authenticated)
        })
        .collect()
    };

    let mut results = Vec::new();
    let mut seen_provider_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Phase 1: registered providers — try live API, fall back to models.dev, then static
    for (id, provider, authenticated) in &snapshot {
      seen_provider_ids.insert(id.clone());

      let (models, is_live): (Vec<String>, bool) = match provider.list_models().await {
        Ok(response) if !response.data.is_empty() => {
          (response.data.into_iter().map(|m| m.id).collect(), true)
        }
        _ => {
          // Fall back to models.dev data for this provider
          if let Some(mdev) = models_dev_db.get(id.as_str()) {
            let mut models: Vec<String> = mdev.models.keys().cloned().collect();
            models.sort();
            if !models.is_empty() {
              (models, false)
            } else {
              (
                provider
                  .default_models()
                  .into_iter()
                  .map(|s| s.to_string())
                  .collect(),
                false,
              )
            }
          } else {
            (
              provider
                .default_models()
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
              false,
            )
          }
        }
      };

      results.push(
        ProviderInfo::new(provider.provider_id(), provider.provider_name())
          .env_vars(
            provider
              .required_env_vars()
              .into_iter()
              .map(|s| s.to_string())
              .collect(),
          )
          .authenticated(*authenticated)
          .models(models)
          .live(is_live),
      );
    }

    // Phase 2: 1:1 opencode — include models.dev providers that have env vars
    // set but aren't registered (e.g. mistral, xai, groq, deepinfra, cerebras,
    // cohere, togetherai, perplexity, etc.)
    for (provider_id, mdev_provider) in &models_dev_db {
      if seen_provider_ids.contains(provider_id) {
        continue;
      }

      // 1:1 opencode: only include if at least one env var is set
      let has_auth = mdev_provider
        .env
        .iter()
        .any(|var| std::env::var(var).is_ok());
      if !has_auth {
        continue;
      }

      let mut models: Vec<String> = mdev_provider.models.keys().cloned().collect();
      models.sort();

      // Filter out deprecated models (1:1 opencode)
      models.retain(|model_id| {
        mdev_provider
          .models
          .get(model_id)
          .map(|m| m.status.as_deref() != Some("deprecated"))
          .unwrap_or(true)
      });

      if models.is_empty() {
        continue;
      }

      results.push(
        ProviderInfo::new(provider_id.clone(), mdev_provider.name.clone())
          .env_vars(mdev_provider.env.clone())
          .authenticated(true)
          .models(models)
          .live(false),
      );
    }

    // Sort results by provider name for consistent ordering
    results.sort_by(|a, b| a.name.cmp(&b.name));
    results
  }

  pub async fn list_models_catalog(&self) -> Vec<ProviderInfo> {
    let models_dev_db = self.models_dev.get().await.unwrap_or_default();

    let registered_ids: std::collections::HashSet<String> = {
      let providers = self.providers.read().await;
      providers.keys().cloned().collect()
    };

    let mut results = Vec::new();
    for (provider_id, mdev_provider) in &models_dev_db {
      let mut models: Vec<String> = mdev_provider.models.keys().cloned().collect();
      models.sort();
      models.retain(|model_id| {
        mdev_provider
          .models
          .get(model_id)
          .map(|m| m.status.as_deref() != Some("deprecated"))
          .unwrap_or(true)
      });
      if models.is_empty() {
        continue;
      }

      let has_env = mdev_provider
        .env
        .iter()
        .any(|var| std::env::var(var).is_ok());
      let authenticated = registered_ids.contains(provider_id) || has_env;

      results.push(
        ProviderInfo::new(provider_id.clone(), mdev_provider.name.clone())
          .env_vars(mdev_provider.env.clone())
          .authenticated(authenticated)
          .models(models)
          .visible(authenticated)
          .live(false),
      );
    }

    results.sort_by(|a, b| a.name.cmp(&b.name));
    results
  }

  pub async fn list_connect_catalog(&self) -> Vec<ProviderInfo> {
    let auth = AuthManager::new().ok();
    let configs = self.configs.read().await.clone();
    let mut providers = PluginRegistry::entries()
      .into_iter()
      .map(|item| {
        ProviderInfo::new(item.id, item.name)
          .connect_method(item.connect_method)
          .connectable(true)
          .env_vars(item.env_vars)
          .models(item.default_models)
      })
      .collect::<Vec<_>>();

    for provider in &mut providers {
      let env_connected = provider.env_vars.iter().any(|env| {
        std::env::var(env)
          .ok()
          .filter(|value| !value.is_empty())
          .is_some()
      });
      let stored_connected = if let Some(auth) = &auth {
        auth.load(&provider.id).await.ok().flatten().is_some()
      } else {
        false
      };
      let runtime_connected = PluginRegistry::find(&provider.id)
        .and_then(|entry| {
          entry
            .runtime_provider_id
            .map(|runtime_provider_id| (entry, runtime_provider_id))
        })
        .and_then(|(entry, runtime_provider_id)| {
          configs
            .get(runtime_provider_id)
            .map(|config| (entry, config))
        })
        .is_some_and(|(_entry, config)| {
          config
            .headers
            .get("x-cokra-connect-source")
            .is_some_and(|source| source == &provider.id)
        });
      provider.authenticated = env_connected || stored_connected || runtime_connected;
    }

    providers.sort_by(|a, b| a.name.cmp(&b.name));
    providers
  }

  pub async fn list_connected_models_catalog(&self) -> Vec<ProviderInfo> {
    let connected = self
      .list_connect_catalog()
      .await
      .into_iter()
      .filter(|provider| provider.authenticated)
      .collect::<Vec<_>>();

    connected
      .into_iter()
      .filter_map(|provider| {
        let entry = PluginRegistry::find(&provider.id)?;
        let model_provider_id = entry.primary_model_provider_id()?;
        let mut info = ProviderInfo::new(model_provider_id, provider.name)
          .models(provider.models)
          .authenticated(true)
          .visible(true)
          .live(false);
        info.options = serde_json::json!({
          "runtime_ready": entry.supports_model_runtime(),
        });
        Some(info)
      })
      .collect()
  }

  pub async fn is_connect_catalog_provider_connected(&self, provider_id: &str) -> bool {
    self
      .list_connect_catalog()
      .await
      .into_iter()
      .find(|provider| provider.id == provider_id)
      .is_some_and(|provider| provider.authenticated)
  }

  /// Trigger a background refresh of the models.dev database.
  pub async fn refresh_models_dev(&self) {
    let _ = self.models_dev.refresh().await;
  }

  /// Check if a provider exists
  pub async fn has_provider(&self, provider_id: &str) -> bool {
    self.providers.read().await.contains_key(provider_id)
  }

  /// Remove a provider
  pub async fn remove(&self, provider_id: &str) -> bool {
    let removed = self.providers.write().await.remove(provider_id).is_some();
    if removed {
      self.configs.write().await.remove(provider_id);

      // Clear default if it was this provider
      let mut default = self.default_provider.write().await;
      if default.as_deref() == Some(provider_id) {
        *default = None;
      }
    }
    removed
  }

  /// Get count of registered providers
  pub async fn len(&self) -> usize {
    self.providers.read().await.len()
  }

  /// Check if registry is empty
  pub async fn is_empty(&self) -> bool {
    self.providers.read().await.is_empty()
  }

  /// Get provider config
  pub async fn get_config(&self, provider_id: &str) -> Option<ProviderConfig> {
    self.configs.read().await.get(provider_id).cloned()
  }

  /// Update provider config
  pub async fn update_config(&self, provider_id: &str, config: ProviderConfig) -> Result<()> {
    if !self.providers.read().await.contains_key(provider_id) {
      return Err(ModelError::ProviderNotFound(provider_id.to_string()));
    }

    self
      .configs
      .write()
      .await
      .insert(provider_id.to_string(), config);
    Ok(())
  }
}

/// Thread-safe reference to the registry
pub type ProviderRegistryRef = Arc<ProviderRegistry>;

/// Create a new provider registry
pub fn new_registry() -> ProviderRegistryRef {
  Arc::new(ProviderRegistry::new())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
  use super::*;
  use async_trait::async_trait;
  use futures::Stream;
  use std::pin::Pin;

  #[derive(Debug)]
  struct TestProvider {
    id: &'static str,
    name: &'static str,
  }

  #[async_trait]
  impl ModelProvider for TestProvider {
    fn provider_id(&self) -> &'static str {
      self.id
    }

    fn provider_name(&self) -> &'static str {
      self.name
    }

    async fn chat_completion(
      &self,
      _request: crate::model::types::ChatRequest,
    ) -> super::Result<crate::model::types::ChatResponse> {
      todo!()
    }

    async fn chat_completion_stream(
      &self,
      _request: crate::model::types::ChatRequest,
    ) -> super::Result<Pin<Box<dyn Stream<Item = super::Result<crate::model::types::Chunk>> + Send>>>
    {
      todo!()
    }

    async fn list_models(&self) -> super::Result<crate::model::types::ListModelsResponse> {
      todo!()
    }

    async fn validate_auth(&self) -> super::Result<()> {
      Ok(())
    }

    fn client(&self) -> &reqwest::Client {
      todo!()
    }

    fn config(&self) -> &crate::model::types::ProviderConfig {
      todo!()
    }
  }

  #[tokio::test]
  async fn test_register_and_get() {
    let registry = ProviderRegistry::new();

    registry
      .register(TestProvider {
        id: "test",
        name: "Test Provider",
      })
      .await;

    let provider = registry.get("test").await;
    assert!(provider.is_some());
    assert_eq!(provider.unwrap().provider_id(), "test");
  }

  #[tokio::test]
  async fn test_list_providers() {
    let registry = ProviderRegistry::new();

    registry
      .register(TestProvider {
        id: "test1",
        name: "Test Provider 1",
      })
      .await;

    registry
      .register(TestProvider {
        id: "test2",
        name: "Test Provider 2",
      })
      .await;

    let providers = registry.list_providers().await;
    assert_eq!(providers.len(), 2);
  }

  #[tokio::test]
  async fn test_default_provider() {
    let registry = ProviderRegistry::new();

    registry
      .register(TestProvider {
        id: "test",
        name: "Test Provider",
      })
      .await;

    registry.set_default("test").await.unwrap();

    let default = registry.get_default().await.unwrap();
    assert_eq!(default.provider_id(), "test");
  }

  #[tokio::test]
  async fn test_connect_catalog_does_not_mark_oauth_provider_connected_from_unrelated_env() {
    let home = tempfile::tempdir().expect("tempdir");

    unsafe {
      std::env::set_var("HOME", home.path());
      std::env::set_var("OPENAI_API_KEY", "test-openai-key-123");
    }

    let registry = ProviderRegistry::new();
    let providers = registry.list_connect_catalog().await;
    let github_copilot = providers
      .iter()
      .find(|provider| provider.id == "github-copilot")
      .expect("github provider");
    let antigravity = providers
      .iter()
      .find(|provider| provider.id == "google-antigravity")
      .expect("antigravity provider");

    assert!(!github_copilot.authenticated);
    assert!(!antigravity.authenticated);

    unsafe {
      std::env::remove_var("OPENAI_API_KEY");
      std::env::remove_var("HOME");
    }
  }

  #[tokio::test]
  async fn test_connect_catalog_marks_runtime_registered_oauth_source_connected() {
    let home = tempfile::tempdir().expect("tempdir");
    unsafe {
      std::env::set_var("HOME", home.path());
      std::env::remove_var("OPENAI_API_KEY");
      std::env::remove_var("OPENROUTER_API_KEY");
      std::env::remove_var("ANTHROPIC_API_KEY");
      std::env::remove_var("GOOGLE_API_KEY");
    }

    let registry = ProviderRegistry::new();
    registry
      .register_with_config(
        TestProvider {
          id: "openai",
          name: "OpenAI",
        },
        ProviderConfig {
          provider_id: "openai".to_string(),
          headers: std::iter::once((
            "x-cokra-connect-source".to_string(),
            "openai-codex".to_string(),
          ))
          .collect(),
          ..Default::default()
        },
      )
      .await;

    let providers = registry.list_connect_catalog().await;
    let openai_codex = providers
      .iter()
      .find(|provider| provider.id == "openai-codex")
      .expect("openai codex provider");
    let openai = providers
      .iter()
      .find(|provider| provider.id == "openai")
      .expect("openai provider");

    assert!(openai_codex.authenticated);
    assert!(!openai.authenticated);

    unsafe {
      std::env::remove_var("HOME");
    }
  }
}
