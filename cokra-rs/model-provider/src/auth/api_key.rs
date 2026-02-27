// API Key Authentication

use crate::auth::AuthInfo;

/// API Key authentication handler
pub struct ApiKeyAuth {
    provider_id: String,
    env_var: String,
    key: Option<String>,
}

impl ApiKeyAuth {
    /// Create new API key auth
    pub fn new(provider_id: &str, env_var: &str) -> Self {
        Self {
            provider_id: provider_id.to_string(),
            env_var: env_var.to_string(),
            key: None,
        }
    }

    /// Check if authenticated
    pub fn is_authenticated(&self) -> bool {
        self.key.is_some() || std::env::var(&self.env_var).is_ok()
    }

    /// Get API key
    pub fn get_key(&self) -> Option<String> {
        if let Some(ref key) = self.key {
            Some(key.clone())
        } else {
            std::env::var(&self.env_var).ok()
        }
    }

    /// Set API key
    pub fn set_key(&mut self, key: String) {
        self.key = Some(key);
    }

    /// Clear API key
    pub fn clear(&mut self) {
        self.key = None;
    }
}
