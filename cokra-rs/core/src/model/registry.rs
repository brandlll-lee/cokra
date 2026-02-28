//! Provider Registry
//!
//! Manages registration and discovery of model providers

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::error::{ModelError, Result};
use super::provider::ProviderInfo;
use super::{ModelProvider, ProviderConfig};

/// Provider Registry
///
/// A registry for managing multiple LLM providers.
/// Providers can be registered, retrieved, and listed.
pub struct ProviderRegistry {
  providers: RwLock<HashMap<String, Arc<dyn ModelProvider>>>,
  default_provider: RwLock<Option<String>>,
  configs: RwLock<HashMap<String, ProviderConfig>>,
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
      if let Some((id, provider)) = providers.iter().next() {
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
    let default_id = self.default_provider.read().await.clone();

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
}
