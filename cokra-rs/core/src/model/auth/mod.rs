//! Authentication module for model providers
//!
//! Handles different authentication methods including API keys, OAuth, and bearer tokens.

pub mod manager;
pub mod oauth;
pub mod resolver;
pub mod storage;

pub use manager::AuthManager;
pub use oauth::{DeviceCodeResponse, OAuthConfig, OAuthManager, OAuthToken};
pub use resolver::{AuthResolver, EnvAuthResolver};
pub use storage::{CredentialStorage, FileCredentialStorage};

use serde::{Deserialize, Serialize};
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

/// Credentials for authenticating with a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Credentials {
  /// API Key authentication
  #[serde(rename = "api_key")]
  ApiKey { key: String },

  /// OAuth authentication
  #[serde(rename = "oauth")]
  OAuth {
    /// Access token
    access_token: String,

    /// Refresh token
    refresh_token: String,

    /// Expiration timestamp (Unix seconds)
    expires_at: u64,

    /// Optional account ID
    #[serde(default)]
    account_id: Option<String>,

    /// Optional enterprise URL
    #[serde(default)]
    enterprise_url: Option<String>,
  },

  /// Bearer token authentication
  #[serde(rename = "bearer")]
  Bearer { token: String },

  /// Device code for OAuth flow
  #[serde(rename = "device_code")]
  DeviceCode {
    /// Device code
    device_code: String,

    /// User code
    user_code: String,

    /// Verification URL
    verification_url: String,

    /// Expiration time
    expires_in: u64,

    /// Polling interval
    interval: u64,
  },
}

impl Credentials {
  /// Get the actual credential value for HTTP requests
  pub fn get_value(&self) -> String {
    match self {
      Credentials::ApiKey { key } => key.clone(),
      Credentials::OAuth { access_token, .. } => access_token.clone(),
      Credentials::Bearer { token } => token.clone(),
      Credentials::DeviceCode { device_code, .. } => device_code.clone(),
    }
  }

  /// Check if credentials are expired (for OAuth)
  pub fn is_expired(&self) -> bool {
    match self {
      Credentials::OAuth { expires_at, .. } => *expires_at < chrono::Utc::now().timestamp() as u64,
      _ => false,
    }
  }

  /// Get the Authorization header value
  pub fn get_auth_header(&self) -> String {
    match self {
      Credentials::ApiKey { key } => format!("Bearer {}", key),
      Credentials::OAuth { access_token, .. } => format!("Bearer {}", access_token),
      Credentials::Bearer { token } => format!("Bearer {}", token),
      Credentials::DeviceCode { device_code, .. } => format!("Bearer {}", device_code),
    }
  }
}

/// Stored credentials with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCredentials {
  /// Provider ID
  pub provider_id: String,

  /// Actual credentials
  pub credentials: Credentials,

  /// When these credentials were stored
  pub stored_at: u64,

  /// Display name for the account
  #[serde(default)]
  pub account_name: Option<String>,

  /// Optional metadata
  #[serde(default)]
  pub metadata: serde_json::Value,
}

impl StoredCredentials {
  /// Create new stored credentials
  pub fn new(provider_id: impl Into<String>, credentials: Credentials) -> Self {
    Self {
      provider_id: provider_id.into(),
      credentials,
      stored_at: chrono::Utc::now().timestamp() as u64,
      account_name: None,
      metadata: serde_json::json!({}),
    }
  }

  /// Set account name
  pub fn with_account_name(mut self, name: impl Into<String>) -> Self {
    self.account_name = Some(name.into());
    self
  }

  /// Set metadata
  pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
    self.metadata = metadata;
    self
  }
}

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
