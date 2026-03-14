//! Model client
//!
//! Unified client for interacting with multiple model providers

use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use tokio::sync::RwLock;

use cokra_protocol::ResponseEvent;

use super::auth::AuthManager;
use super::error::ModelError;
use super::error::Result;
use super::provider_catalog::find_provider_catalog_entry;
use super::providers::register_provider_by_registration;
use super::providers::registration_token_for_stored;
use super::registry::ProviderRegistryRef;
use super::transform::ProviderRuntimeKind;
use super::transform::ProviderRuntimeTransform;
use super::transform::RuntimeRequestDefaults;
use super::types::ChatRequest;
use super::types::ChatResponse;
use super::types::Chunk;

/// Model client
///
/// Provides a unified interface for interacting with different LLM providers.
pub struct ModelClient {
  registry: ProviderRegistryRef,
  default_provider: RwLock<Option<String>>,
  config: RwLock<ClientConfig>,
}

#[derive(Debug, Clone)]
pub struct ModelRuntimeInfo {
  pub provider_id: String,
  pub runtime_kind: ProviderRuntimeKind,
  pub connect_source: Option<String>,
}

impl ModelClient {
  /// Create a new model client
  pub async fn new(registry: ProviderRegistryRef) -> Result<Self> {
    Ok(Self {
      registry,
      default_provider: RwLock::new(None),
      config: RwLock::new(ClientConfig::default()),
    })
  }

  /// Create with default provider
  pub async fn with_default_provider(
    registry: ProviderRegistryRef,
    provider_id: &str,
  ) -> Result<Self> {
    let client = Self::new(registry).await?;
    client.set_default_provider(provider_id).await?;
    Ok(client)
  }

  /// Set the default provider
  pub async fn set_default_provider(&self, provider_id: &str) -> Result<()> {
    if !self.registry.has_provider(provider_id).await {
      return Err(ModelError::ProviderNotFound(provider_id.to_string()));
    }
    *self.default_provider.write().await = Some(provider_id.to_string());
    Ok(())
  }

  /// Get the default provider ID
  pub async fn get_default_provider(&self) -> Option<String> {
    self.default_provider.read().await.clone()
  }

  pub async fn runtime_info_for_model(&self, model: &str) -> Result<ModelRuntimeInfo> {
    let provider_id = self.resolve_provider_id(model).await?;
    let provider = self
      .registry
      .get(&provider_id)
      .await
      .ok_or_else(|| ModelError::ProviderNotFound(provider_id.clone()))?;
    let connect_source = provider
      .config()
      .headers
      .get("x-cokra-connect-source")
      .cloned();
    let transform = ProviderRuntimeTransform::from_config(provider.config());

    Ok(ModelRuntimeInfo {
      provider_id,
      runtime_kind: transform.runtime_kind(),
      connect_source,
    })
  }

  /// Send a chat completion request
  pub async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
    self
      .refresh_connect_provider_if_needed(&request.model)
      .await?;

    // Determine which provider to use
    let provider = self.select_provider(&request.model).await?;

    // Add default values if not set
    let mut request = self.enrich_request(request, &provider).await?;
    request.model = get_model_name(&request.model).to_string();

    // Call the provider
    provider.chat_completion(request).await
  }

  /// Send a streaming chat completion request
  pub async fn chat_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>> {
    self
      .refresh_connect_provider_if_needed(&request.model)
      .await?;
    let provider = self.select_provider(&request.model).await?;
    let mut request = self.enrich_request(request, &provider).await?;
    request.model = get_model_name(&request.model).to_string();
    provider.chat_completion_stream(request).await
  }

  /// Send a Responses-API compatible SSE request.
  pub async fn responses_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<ResponseEvent>> + Send>>> {
    self
      .refresh_connect_provider_if_needed(&request.model)
      .await?;
    let provider = self.select_provider(&request.model).await?;
    let mut request = self.enrich_request(request, &provider).await?;
    request.model = get_model_name(&request.model).to_string();
    provider.responses_stream(request).await
  }

  /// Select the appropriate provider for a model
  async fn select_provider(&self, model: &str) -> Result<Arc<dyn super::ModelProvider>> {
    let provider_id = self.resolve_provider_id(model).await?;
    self
      .registry
      .get(&provider_id)
      .await
      .ok_or(ModelError::ProviderNotFound(provider_id))
  }

  async fn resolve_provider_id(&self, model: &str) -> Result<String> {
    if let Some((provider_id, _)) = model.split_once('/') {
      return Ok(provider_id.to_string());
    }

    if let Some(provider_id) = self.get_default_provider().await {
      return Ok(provider_id);
    }

    self
      .registry
      .get_default()
      .await
      .map(|provider| provider.provider_id().to_string())
  }

  async fn refresh_connect_provider_if_needed(&self, model: &str) -> Result<()> {
    let provider_id = self.resolve_provider_id(model).await?;
    let Some(runtime_config) = self.registry.get_config(&provider_id).await else {
      return Ok(());
    };
    let Some(source_entry_id) = runtime_config
      .headers
      .get("x-cokra-connect-source")
      .cloned()
    else {
      return Ok(());
    };
    let Some(entry) = find_provider_catalog_entry(&source_entry_id) else {
      return Ok(());
    };

    let auth = AuthManager::new().map_err(|err| ModelError::AuthError(err.to_string()))?;
    let stored = auth
      .load_for_runtime_registration(entry.id)
      .await
      .map_err(|err| ModelError::AuthError(err.to_string()))?
      .ok_or_else(|| {
        ModelError::AuthError(format!(
          "stored OAuth credentials not found for provider {}",
          entry.id
        ))
      })?;
    let Some(token) = registration_token_for_stored(entry.runtime_registration, &stored) else {
      return Ok(());
    };

    if runtime_config.api_key.as_deref() == Some(token.as_str()) {
      return Ok(());
    }

    // Tradeoff: ModelClient only has the live registry config at request time,
    // so re-registration reuses that config as the base instead of the original
    // app config object. This preserves runtime overrides like custom base_url.
    register_provider_by_registration(
      self.registry.as_ref(),
      &cokra_config::Config::default(),
      entry.runtime_registration,
      token,
      Some(entry.id),
      Some(&stored),
      Some(&runtime_config),
    )
    .await
  }

  /// Enrich request with default values
  async fn enrich_request(
    &self,
    request: ChatRequest,
    provider: &Arc<dyn super::ModelProvider>,
  ) -> Result<ChatRequest> {
    let config = self.config.read().await;
    let transform = ProviderRuntimeTransform::from_config(provider.config());
    Ok(transform.apply_client_defaults(
      request,
      RuntimeRequestDefaults {
        temperature: config.default_temperature,
        max_tokens: config.default_max_tokens,
      },
    ))
  }

  /// Set client configuration
  pub async fn set_config(&self, config: ClientConfig) {
    *self.config.write().await = config;
  }

  /// Get the provider registry
  pub fn registry(&self) -> &ProviderRegistryRef {
    &self.registry
  }
}

impl Clone for ModelClient {
  fn clone(&self) -> Self {
    Self {
      registry: Arc::clone(&self.registry),
      default_provider: RwLock::new(self.default_provider.blocking_read().clone()),
      config: RwLock::new(self.config.blocking_read().clone()),
    }
  }
}

/// Client configuration
#[derive(Debug, Clone)]
pub struct ClientConfig {
  /// Default temperature for requests
  pub default_temperature: Option<f32>,

  /// Default max tokens for requests
  pub default_max_tokens: Option<u32>,

  /// Request timeout in seconds
  pub timeout: Option<u64>,

  /// Maximum number of retries
  pub max_retries: Option<u32>,
}

impl Default for ClientConfig {
  fn default() -> Self {
    Self {
      default_temperature: Some(0.7),
      default_max_tokens: Some(4096),
      timeout: Some(120),
      max_retries: Some(3),
    }
  }
}

/// Helper to parse model ID
///
/// Returns (provider_id, model_name)
pub fn parse_model_id(model_id: &str) -> (Option<&str>, &str) {
  if let Some((provider_id, model_name)) = model_id.split_once('/') {
    return (Some(provider_id), model_name);
  }
  (None, model_id)
}

/// Get provider ID from model ID
pub fn get_provider_id(model_id: &str) -> Option<&str> {
  parse_model_id(model_id).0
}

/// Get just the model name without provider prefix
pub fn get_model_name(model_id: &str) -> &str {
  parse_model_id(model_id).1
}
