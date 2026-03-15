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
use super::model_catalog;
use super::models_dev::ModelsDevClient;
use super::provider::ProviderInfo;
use super::provider_catalog::find_connect_provider_by_runtime_id;
use super::provider_catalog::find_provider_catalog_entry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCatalogEntry {
  pub provider_id: String,
  pub provider_name: String,
  pub model_id: String,
  pub model_name: String,
  pub context_window: Option<u64>,
  pub reasoning: bool,
}

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
  /// Shared auth manager used for connect-catalog credential discovery.
  auth: Option<Arc<AuthManager>>,
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
      // Tradeoff: resolve default storage once at startup to avoid repeated disk I/O
      // and env-dependent path resolution on every catalog render.
      auth: AuthManager::new().ok().map(Arc::new),
    }
  }

  pub fn new_with_auth(auth: Option<Arc<AuthManager>>) -> Self {
    Self {
      providers: RwLock::new(HashMap::new()),
      default_provider: RwLock::new(None),
      configs: RwLock::new(HashMap::new()),
      models_dev: ModelsDevClient::new(),
      auth,
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
    model_catalog::build_connect_catalog(self.auth.as_ref()).await
  }

  pub async fn list_connected_models_catalog(&self) -> Vec<ProviderInfo> {
    // Tradeoff: use cached models.dev data to keep the TUI responsive; a background
    // refresh will warm the cache if it's missing.
    let models_dev_db = self
      .models_dev
      .get_cached_or_refresh()
      .await
      .unwrap_or_default();
    let connected = self
      .list_connect_catalog()
      .await
      .into_iter()
      .filter(|provider| provider.authenticated)
      .collect::<Vec<_>>();

    model_catalog::build_connected_models_catalog(&models_dev_db, connected)
  }

  pub async fn lookup_model_catalog(
    &self,
    provider_id: &str,
    model_id: &str,
  ) -> Option<ModelCatalogEntry> {
    let runtime_config = self.get_config(provider_id).await;
    let connect_source = runtime_config
      .as_ref()
      .and_then(|config| config.headers.get("x-cokra-connect-source").cloned());
    let connect_provider = connect_source
      .as_deref()
      .and_then(find_provider_catalog_entry)
      .or_else(|| find_connect_provider_by_runtime_id(provider_id));
    let display_provider_id = connect_source.unwrap_or_else(|| {
      connect_provider
        .map(|provider| provider.id.to_string())
        .unwrap_or_else(|| provider_id.to_string())
    });
    let display_provider_name = connect_provider.map(|provider| provider.name.to_string());

    let models_dev_db = self
      .models_dev
      .get_cached_or_refresh()
      .await
      .unwrap_or_default();

    if let Some(provider) = models_dev_db.get(provider_id)
      && let Some(model) = provider.models.get(model_id)
    {
      return Some(ModelCatalogEntry {
        provider_id: display_provider_id.clone(),
        provider_name: display_provider_name.unwrap_or_else(|| provider.name.clone()),
        model_id: model_id.to_string(),
        model_name: if model.name.is_empty() {
          model_id.to_string()
        } else {
          model.name.clone()
        },
        context_window: model
          .limit
          .as_ref()
          .map(|limit| limit.context)
          .filter(|limit| *limit > 0),
        reasoning: model.reasoning,
      });
    }

    self
      .get(provider_id)
      .await
      .map(|provider| ModelCatalogEntry {
        provider_id: display_provider_id,
        provider_name: display_provider_name
          .unwrap_or_else(|| provider.provider_name().to_string()),
        model_id: model_id.to_string(),
        model_name: model_id.to_string(),
        context_window: None,
        reasoning: false,
      })
  }

  pub async fn post_connect_probe(
    &self,
    connect_provider_id: &str,
  ) -> model_catalog::PostConnectProbeResult {
    let models_dev_db = self
      .models_dev
      .get_cached_or_refresh()
      .await
      .unwrap_or_default();
    model_catalog::post_connect_probe(self, &models_dev_db, connect_provider_id).await
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
  use crate::model::models_dev;
  use async_trait::async_trait;
  use futures::Stream;
  use pretty_assertions::assert_eq;
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
  async fn test_connect_catalog_header_tag_does_not_affect_connected_status() {
    // Phase 4: runtime_connected via x-cokra-connect-source header is removed.
    // Only env vars and stored credentials determine connect catalog status.
    let home = tempfile::tempdir().expect("tempdir");
    unsafe {
      std::env::set_var("HOME", home.path());
      std::env::remove_var("OPENAI_API_KEY");
      std::env::remove_var("OPENROUTER_API_KEY");
      std::env::remove_var("ANTHROPIC_API_KEY");
      std::env::remove_var("GOOGLE_API_KEY");
    }

    let registry = ProviderRegistry::new_with_auth(Some(Arc::new(AuthManager::memory())));
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

    // Neither provider has stored credentials or env vars, so both are not connected
    // regardless of any header tags on registered runtime providers.
    assert!(!openai_codex.authenticated);
    assert!(!openai.authenticated);

    unsafe {
      std::env::remove_var("HOME");
    }
  }

  #[tokio::test]
  async fn test_connect_catalog_stored_credentials_mark_provider_connected() {
    use crate::model::auth::AuthManager;
    use crate::model::auth::Credentials;
    use crate::model::auth::StoredCredentials;

    unsafe {
      std::env::remove_var("OPENAI_API_KEY");
      std::env::remove_var("ANTHROPIC_API_KEY");
    }

    let auth = Arc::new(AuthManager::memory());
    auth
      .save_stored(StoredCredentials::new(
        "openai",
        Credentials::ApiKey {
          key: "sk-stored-key-123".to_string(),
        },
      ))
      .await
      .expect("save stored");

    let registry = ProviderRegistry::new_with_auth(Some(auth));
    let providers = registry.list_connect_catalog().await;
    let openai = providers
      .iter()
      .find(|provider| provider.id == "openai")
      .expect("openai provider");
    let anthropic = providers
      .iter()
      .find(|provider| provider.id == "anthropic")
      .expect("anthropic provider");

    assert!(
      openai.authenticated,
      "stored credentials should mark openai as connected"
    );
    assert!(!anthropic.authenticated, "no credentials for anthropic");
  }

  #[tokio::test]
  async fn lookup_model_catalog_prefers_models_dev_limits() {
    let registry = ProviderRegistry::new();
    registry
      .register(TestProvider {
        id: "openai",
        name: "OpenAI",
      })
      .await;

    let mut models = HashMap::new();
    models.insert(
      "gpt-5.3-codex".to_string(),
      models_dev::ModelsDevModel {
        id: "gpt-5.3-codex".to_string(),
        name: "GPT-5.3 Codex".to_string(),
        family: None,
        release_date: "2025-01-01".to_string(),
        attachment: true,
        reasoning: true,
        temperature: false,
        tool_call: true,
        cost: None,
        limit: Some(models_dev::ModelsDevLimit {
          context: 272_000,
          input: Some(272_000),
          output: 128_000,
        }),
        modalities: None,
        status: None,
        options: None,
        headers: None,
        provider: None,
      },
    );

    registry
      .models_dev
      .replace_cached_database_for_tests(HashMap::from([(
        "openai".to_string(),
        models_dev::ModelsDevProvider {
          id: "openai".to_string(),
          name: "OpenAI".to_string(),
          api: None,
          env: Vec::new(),
          npm: None,
          models,
        },
      )]))
      .await;

    let entry = registry
      .lookup_model_catalog("openai", "gpt-5.3-codex")
      .await
      .expect("catalog entry");

    assert_eq!(
      entry,
      ModelCatalogEntry {
        provider_id: "openai".to_string(),
        provider_name: "OpenAI".to_string(),
        model_id: "gpt-5.3-codex".to_string(),
        model_name: "GPT-5.3 Codex".to_string(),
        context_window: Some(272_000),
        reasoning: true,
      }
    );
  }

  #[tokio::test]
  async fn lookup_model_catalog_prefers_connect_source_for_display_identity() {
    let registry = ProviderRegistry::new();
    registry
      .register_with_config(
        TestProvider {
          id: "openai",
          name: "OpenAI",
        },
        ProviderConfig {
          provider_id: "openai".to_string(),
          headers: HashMap::from([(
            "x-cokra-connect-source".to_string(),
            "openai-codex".to_string(),
          )]),
          ..Default::default()
        },
      )
      .await;

    let mut models = HashMap::new();
    models.insert(
      "gpt-5.3-codex".to_string(),
      models_dev::ModelsDevModel {
        id: "gpt-5.3-codex".to_string(),
        name: "GPT-5.3 Codex".to_string(),
        family: None,
        release_date: "2025-01-01".to_string(),
        attachment: true,
        reasoning: true,
        temperature: false,
        tool_call: true,
        cost: None,
        limit: Some(models_dev::ModelsDevLimit {
          context: 272_000,
          input: Some(272_000),
          output: 128_000,
        }),
        modalities: None,
        status: None,
        options: None,
        headers: None,
        provider: None,
      },
    );

    registry
      .models_dev
      .replace_cached_database_for_tests(HashMap::from([(
        "openai".to_string(),
        models_dev::ModelsDevProvider {
          id: "openai".to_string(),
          name: "OpenAI".to_string(),
          api: None,
          env: Vec::new(),
          npm: None,
          models,
        },
      )]))
      .await;

    let entry = registry
      .lookup_model_catalog("openai", "gpt-5.3-codex")
      .await
      .expect("catalog entry");

    assert_eq!(
      entry,
      ModelCatalogEntry {
        provider_id: "openai-codex".to_string(),
        provider_name: "ChatGPT Plus/Pro (Codex Subscription)".to_string(),
        model_id: "gpt-5.3-codex".to_string(),
        model_name: "GPT-5.3 Codex".to_string(),
        context_window: Some(272_000),
        reasoning: true,
      }
    );
  }
}
