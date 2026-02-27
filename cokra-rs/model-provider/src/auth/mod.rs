// Auth Module
pub mod api_key;
mod oauth;
mod storage;

pub use api_key::ApiKeyAuth;
pub use oauth::OAuthAuth;
pub use storage::AuthStorage;

use serde::{Deserialize, Serialize};

/// Authentication information
#[derive(Clone, Serialize, Deserialize)]
pub enum AuthInfo {
    /// API key authentication
    ApiKey { key: String },

    /// OAuth authentication
    OAuth {
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<i64>,
        account_id: Option<String>,
    },

    /// No authentication needed
    None,
}

/// Authentication method
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthMethod {
    OAuth,
    ApiKey,
}

/// Auth configuration for a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Method type
    pub method: AuthMethod,

    /// Label for UI
    pub label: String,

    /// Environment variable names
    pub env_vars: Vec<String>,
}
