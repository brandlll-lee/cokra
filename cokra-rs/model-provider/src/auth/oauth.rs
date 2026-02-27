// OAuth Authentication

use serde::{Deserialize, Serialize};

/// OAuth authentication handler
pub struct OAuthAuth {
    provider_id: String,
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_at: Option<i64>,
}

impl OAuthAuth {
    /// Create new OAuth auth
    pub fn new(provider_id: &str) -> Self {
        Self {
            provider_id: provider_id.to_string(),
            access_token: None,
            refresh_token: None,
            expires_at: None,
        }
    }

    /// Check if authenticated
    pub fn is_authenticated(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            let now = chrono::Utc::now().timestamp();
            now < expires_at
        } else {
            self.access_token.is_some()
        }
    }

    /// Get access token
    pub fn access_token(&self) -> Option<&str> {
        self.access_token.as_deref()
    }

    /// Set tokens
    pub fn set_tokens(
        &mut self,
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<i64>,
    ) {
        self.access_token = Some(access_token);
        self.refresh_token = refresh_token;
        if let Some(expires_in) = expires_in {
            self.expires_at = Some(chrono::Utc::now().timestamp() + expires_in);
        }
    }

    /// Clear tokens
    pub fn clear(&mut self) {
        self.access_token = None;
        self.refresh_token = None;
        self.expires_at = None;
    }
}

/// OAuth result
#[derive(Debug, Serialize, Deserialize)]
pub enum OAuthResult {
    Success {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<i64>,
    },
    Error {
        message: String,
    },
}
