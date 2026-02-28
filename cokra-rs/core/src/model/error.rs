//! Model layer error types

use thiserror::Error;

/// Model provider errors
#[derive(Error, Debug)]
pub enum ModelError {
  /// Authentication failed
  #[error("Authentication failed: {0}")]
  AuthError(String),

  /// Invalid request
  #[error("Invalid request: {0}")]
  InvalidRequest(String),

  /// Invalid response from provider
  #[error("Invalid response: {0}")]
  InvalidResponse(String),

  /// Provider API error
  #[error("Provider API error: {0}")]
  ApiError(String),

  /// Rate limited
  #[error("Rate limited: {0}")]
  RateLimited(String),

  /// Network error
  #[error("Network error: {0}")]
  NetworkError(#[from] reqwest::Error),

  /// JSON parse error
  #[error("JSON parse error: {0}")]
  JsonError(#[from] serde_json::Error),

  /// Provider not found
  #[error("Provider not found: {0}")]
  ProviderNotFound(String),

  /// No default provider configured
  #[error("No default provider configured")]
  NoDefaultProvider,

  /// Tool not found
  #[error("Tool not found: {0}")]
  ToolNotFound(String),

  /// Tool execution error
  #[error("Tool execution error: {0}")]
  ToolError(String),

  /// Streaming error
  #[error("Streaming error: {0}")]
  StreamError(String),

  /// Timeout
  #[error("Request timeout: {0}")]
  Timeout(String),

  /// Context limit exceeded
  #[error("Context limit exceeded: {0}")]
  ContextLimitExceeded(String),

  /// Invalid credentials
  #[error("Invalid credentials: {0}")]
  InvalidCredentials(String),

  /// OAuth error
  #[error("OAuth error: {0}")]
  OAuthError(String),
}

/// Alias for Result<T, ModelError>
pub type Result<T> = std::result::Result<T, ModelError>;
