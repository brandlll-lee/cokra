//! Authentication module for model providers.
//!
//! Handles different authentication methods including API keys, OAuth, and bearer tokens.

pub mod manager;
pub mod oauth;
pub mod resolver;

pub use manager::AuthManager;
pub use oauth::DeviceCodeResponse;
pub use oauth::OAuthConfig;
pub use oauth::OAuthManager;
pub use oauth::OAuthToken;
pub use resolver::AuthResolver;
pub use resolver::EnvAuthResolver;

pub use crate::model::auth_store::CredentialStorage;
pub use crate::model::auth_store::Credentials;
pub use crate::model::auth_store::FileCredentialStorage;
pub use crate::model::auth_store::MemoryCredentialStorage;
pub use crate::model::auth_store::StoredCredentials;
pub use crate::model::provider_catalog::OAuthClientEnv;
pub use crate::model::provider_catalog::RuntimeRegistrationKind;

use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

/// Authentication errors
#[derive(Error, Debug)]
pub enum AuthError {
  /// Credentials not found
  #[error("Credentials not found for provider: {0}")]
  NotFound(String),

  /// Invalid credentials
  #[error("Invalid credentials: {0}")]
  InvalidCredentials(String),

  /// OAuth error
  #[error("OAuth error: {0}")]
  OAuthError(String),

  /// Storage error
  #[error("Storage error: {0}")]
  StorageError(String),

  /// IO error
  #[error("IO error: {0}")]
  IoError(#[from] std::io::Error),

  /// JSON parse error
  #[error("JSON error: {0}")]
  JsonError(#[from] serde_json::Error),

  /// Token expired
  #[error("Token expired for provider: {0}")]
  TokenExpired(String),

  /// OAuth wait timeout
  #[error("OAuth operation timed out")]
  Timeout,
}

/// Authentication result
pub type Result<T> = std::result::Result<T, AuthError>;

/// Authentication request
#[derive(Debug, Clone)]
pub struct AuthRequest {
  /// Provider ID
  pub provider_id: String,

  /// Request type
  pub auth_type: AuthType,

  /// Optional client ID for OAuth
  pub client_id: Option<String>,

  /// Optional scopes
  pub scopes: Option<Vec<String>>,
}

impl AuthRequest {
  /// Create a new auth request
  pub fn new(provider_id: impl Into<String>, auth_type: AuthType) -> Self {
    Self {
      provider_id: provider_id.into(),
      auth_type,
      client_id: None,
      scopes: None,
    }
  }

  /// Set client ID
  pub fn with_client_id(mut self, client_id: impl Into<String>) -> Self {
    self.client_id = Some(client_id.into());
    self
  }

  /// Set scopes
  pub fn with_scopes(mut self, scopes: Vec<String>) -> Self {
    self.scopes = Some(scopes);
    self
  }
}

/// Type of authentication
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthType {
  /// API Key authentication
  ApiKey,

  /// OAuth with callback
  OAuth,

  /// OAuth device code flow
  OAuthDevice,

  /// Bearer token
  Bearer,
}

/// OAuth callback response
#[derive(Debug, Clone, Deserialize)]
pub struct OAuthCallback {
  /// Authorization code
  pub code: String,

  /// State parameter
  pub state: String,

  /// Error if any
  #[serde(default)]
  pub error: Option<String>,
}

/// Authentication provider info
#[derive(Debug, Clone, Serialize)]
pub struct AuthProviderInfo {
  /// Provider ID
  pub id: String,

  /// Display name
  pub name: String,

  /// Supported auth methods
  pub auth_methods: Vec<AuthMethod>,

  /// OAuth client ID (if applicable)
  pub oauth_client_id: Option<String>,

  /// OAuth scopes (if applicable)
  pub oauth_scopes: Option<Vec<String>>,
}

/// Authentication method
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthMethod {
  /// API Key method
  #[serde(rename = "api_key")]
  ApiKey {
    label: String,
    #[serde(default)]
    placeholder: String,
  },

  /// OAuth method
  #[serde(rename = "oauth")]
  OAuth {
    label: String,
    #[serde(default)]
    authorization_url: String,
  },
}
