//! Model Provider trait and implementations
//!
//! This module defines the [ModelProvider] trait that all LLM providers must implement.

use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

use super::error::{ModelError, Result};
use super::types::{
  ChatRequest, ChatResponse, Chunk, ListModelsResponse, ModelInfo, ProviderConfig,
};

/// Model Provider trait
///
/// All LLM providers must implement this trait to be used with Cokra.
/// It provides a unified interface for chat completions, streaming, and authentication.
#[async_trait]
pub trait ModelProvider: Send + Sync {
  /// Returns the unique identifier for this provider
  fn provider_id(&self) -> &'static str;

  /// Returns the display name for this provider
  fn provider_name(&self) -> &'static str;

  /// Returns the list of environment variables required by this provider
  fn required_env_vars(&self) -> Vec<&'static str> {
    Vec::new()
  }

  /// Returns the default models for this provider
  fn default_models(&self) -> Vec<&'static str> {
    Vec::new()
  }

  /// Creates a chat completion
  async fn chat_completion(&self, request: ChatRequest) -> Result<ChatResponse>;

  /// Creates a streaming chat completion
  async fn chat_completion_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>>;

  /// Lists available models for this provider
  async fn list_models(&self) -> Result<ListModelsResponse>;

  /// Validates that authentication is working
  async fn validate_auth(&self) -> Result<()>;

  /// Returns the HTTP client for this provider
  fn client(&self) -> &Client;

  /// Returns the configuration for this provider
  fn config(&self) -> &ProviderConfig;
}

/// Information about a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
  /// Unique identifier
  pub id: String,

  /// Display name
  pub name: String,

  /// Required environment variables
  pub env_vars: Vec<String>,

  /// Whether the provider is authenticated
  pub authenticated: bool,

  /// Available models
  pub models: Vec<String>,

  /// Provider-specific options
  #[serde(default)]
  pub options: serde_json::Value,
}

impl ProviderInfo {
  /// Create a new ProviderInfo
  pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
    Self {
      id: id.into(),
      name: name.into(),
      env_vars: Vec::new(),
      authenticated: false,
      models: Vec::new(),
      options: serde_json::json!({}),
    }
  }

  /// Set authenticated status
  pub fn authenticated(mut self, authenticated: bool) -> Self {
    self.authenticated = authenticated;
    self
  }

  /// Set environment variables
  pub fn env_vars(mut self, env_vars: Vec<String>) -> Self {
    self.env_vars = env_vars;
    self
  }

  /// Set available models
  pub fn models(mut self, models: Vec<String>) -> Self {
    self.models = models;
    self
  }
}

/// Builder for creating providers
pub struct ProviderBuilder<P: ModelProvider> {
  provider: P,
}

impl<P: ModelProvider + Default> ProviderBuilder<P> {
  /// Create a new builder with default configuration
  pub fn new() -> Self {
    Self {
      provider: P::default(),
    }
  }
}

impl<P: ModelProvider> ProviderBuilder<P> {
  /// Set the API key
  pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
    // This would need to be implemented by each provider
    self
  }

  /// Set the base URL
  pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
    self
  }

  /// Build the provider
  pub fn build(self) -> P {
    self.provider
  }
}

/// Stream wrapper for async streams
pub struct ModelStream {
  // Internal stream type
}

impl ModelStream {
  /// Create a new model stream
  pub fn new() -> Self {
    Self {}
  }
}

/// Trait for converting responses to chunks
pub trait ToChunk {
  /// Convert a response to a chunk
  fn to_chunk(&self) -> Chunk;
}

// =============================================================================
// Helper functions for provider implementations
// =============================================================================

/// Standard error handling for HTTP responses
pub async fn handle_response(response: reqwest::Response) -> Result<String> {
  if response.status().is_success() {
    Ok(response.text().await?)
  } else {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(ModelError::ApiError(format!("HTTP {}: {}", status, body)))
  }
}

/// Parse JSON from response
pub async fn parse_response<T: serde::de::DeserializeOwned>(
  response: reqwest::Response,
) -> Result<T> {
  let body = handle_response(response).await?;
  Ok(serde_json::from_str(&body)?)
}

/// Build headers for API requests
pub fn build_headers(
  api_key: &str,
  extra: &std::collections::HashMap<String, String>,
) -> reqwest::header::HeaderMap {
  use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};

  let mut headers = HeaderMap::new();

  // Authorization header
  if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", api_key)) {
    headers.insert(AUTHORIZATION, value);
  }

  // Extra headers
  for (key, value) in extra {
    if let (Ok(name), Ok(val)) = (
      HeaderName::from_bytes(key.as_bytes()),
      HeaderValue::from_str(value),
    ) {
      headers.insert(name, val);
    }
  }

  headers
}
