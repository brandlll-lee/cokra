//! Model client
//!
//! Unified client for interacting with multiple model providers

use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use tokio::sync::RwLock;

use cokra_protocol::ResponseEvent;

use super::error::{ModelError, Result};
use super::registry::ProviderRegistryRef;
use super::types::{ChatRequest, ChatResponse, Chunk};

/// Model client
///
/// Provides a unified interface for interacting with different LLM providers.
pub struct ModelClient {
  registry: ProviderRegistryRef,
  default_provider: RwLock<Option<String>>,
  config: RwLock<ClientConfig>,
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

  /// Send a chat completion request
  pub async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
    // Determine which provider to use
    let provider = self.select_provider(&request.model).await?;

    // Add default values if not set
    let mut request = self.enrich_request(request).await?;
    request.model = get_model_name(&request.model).to_string();

    // Call the provider
    provider.chat_completion(request).await
  }

  /// Send a streaming chat completion request
  pub async fn chat_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>> {
    let provider = self.select_provider(&request.model).await?;
    let mut request = self.enrich_request(request).await?;
    request.model = get_model_name(&request.model).to_string();
    provider.chat_completion_stream(request).await
  }

  /// Send a Responses-API compatible SSE request.
  pub async fn responses_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<ResponseEvent>> + Send>>> {
    let provider = self.select_provider(&request.model).await?;
    let mut request = self.enrich_request(request).await?;
    request.model = get_model_name(&request.model).to_string();
    provider.responses_stream(request).await
  }

  /// Select the appropriate provider for a model
  async fn select_provider(&self, model: &str) -> Result<Arc<dyn super::ModelProvider>> {
    if let Some((provider_id, _)) = model.split_once('/') {
      return self
        .registry
        .get(provider_id)
        .await
        .ok_or_else(|| ModelError::ProviderNotFound(provider_id.to_string()));
    }

    if let Some(provider_id) = self.get_default_provider().await {
      return self
        .registry
        .get(&provider_id)
        .await
        .ok_or(ModelError::ProviderNotFound(provider_id));
    }

    self.registry.get_default().await
  }

  /// Enrich request with default values
  async fn enrich_request(&self, mut request: ChatRequest) -> Result<ChatRequest> {
    let config = self.config.read().await;

    // Set default temperature
    if request.temperature.is_none() {
      request.temperature = config.default_temperature;
    }

    // Set default max tokens
    if request.max_tokens.is_none() {
      request.max_tokens = config.default_max_tokens;
    }

    Ok(request)
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
