//! Authentication manager
//!
//! Centralized authentication management for all model providers

use super::AuthError;
use super::AuthRequest;
use super::AuthType;
use super::Credentials;
use super::Result;
use super::StoredCredentials;
use super::oauth::DeviceCodeResponse;
use super::oauth::OAuthConfig;
use super::oauth::OAuthManager;
use super::resolver::AuthResolver;
use super::resolver::EnvAuthResolver;
use super::storage::CredentialStorage;
use super::storage::FileCredentialStorage;
use super::storage::MemoryCredentialStorage;

/// Authentication manager
///
/// Handles authentication for all model providers, supporting:
/// - Environment variable resolution
/// - Persistent credential storage
/// - OAuth flows
/// - API key management
pub struct AuthManager {
  storage: std::sync::Arc<dyn CredentialStorage>,
  resolvers: Vec<Box<dyn AuthResolver>>,
}

impl AuthManager {
  /// Create a new auth manager with default storage
  pub fn new() -> Result<Self> {
    let storage = std::sync::Arc::new(FileCredentialStorage::default_storage()?);
    Self::with_storage(storage)
  }

  /// Create with custom storage
  pub fn with_storage(storage: std::sync::Arc<dyn CredentialStorage>) -> Result<Self> {
    let resolvers = vec![Box::new(EnvAuthResolver::new()) as Box<dyn AuthResolver>];

    Ok(Self { storage, resolvers })
  }

  /// Create a memory-only auth manager (for testing)
  pub fn memory() -> Self {
    let storage = std::sync::Arc::new(MemoryCredentialStorage::new());
    Self {
      storage,
      resolvers: vec![],
    }
  }

  /// Add a resolver to the chain
  pub fn add_resolver(mut self, resolver: Box<dyn AuthResolver>) -> Self {
    self.resolvers.push(resolver);
    self
  }

  /// Resolve credentials for a provider
  ///
  /// Tries all resolvers in order:
  /// 1. Environment variables
  /// 2. Config file
  /// 3. Storage
  pub fn resolve_credentials(&self, provider_id: &str) -> Option<Credentials> {
    for resolver in &self.resolvers {
      if let Some(creds) = resolver.resolve(provider_id) {
        return Some(creds);
      }
    }
    None
  }

  /// Resolve credentials or load from storage
  pub async fn load(&self, provider_id: &str) -> Result<Option<StoredCredentials>> {
    // First try to resolve from env/config
    if let Some(creds) = self.resolve_credentials(provider_id) {
      return Ok(Some(StoredCredentials::new(provider_id, creds)));
    }

    // Then try storage
    self.storage.load(provider_id).await
  }

  pub async fn load_for_runtime_registration(
    &self,
    provider_id: &str,
  ) -> Result<Option<StoredCredentials>> {
    let Some(stored) = self.load(provider_id).await? else {
      return Ok(None);
    };

    let should_refresh = match &stored.credentials {
      Credentials::OAuth {
        refresh_token,
        expires_at,
        ..
      } => !refresh_token.is_empty() && *expires_at <= chrono::Utc::now().timestamp() as u64,
      _ => false,
    };

    if !should_refresh {
      return Ok(Some(stored));
    }

    // Tradeoff: runtime registration only refreshes persisted OAuth credentials.
    // Env/config credentials bypass storage and should remain side-effect free.
    self.refresh_oauth(provider_id).await?;
    self.storage.load(provider_id).await
  }

  /// Save credentials to storage
  pub async fn save(&self, provider_id: &str, credentials: Credentials) -> Result<()> {
    let stored = StoredCredentials::new(provider_id, credentials);
    self.storage.save(stored).await
  }

  /// Save a fully populated stored credential record.
  pub async fn save_stored(&self, stored: StoredCredentials) -> Result<()> {
    self.storage.save(stored).await
  }

  /// Delete credentials from storage
  pub async fn remove(&self, provider_id: &str) -> Result<()> {
    self.storage.delete(provider_id).await
  }

  /// Validate credentials
  ///
  /// This is a basic validation - actual validation depends on the provider
  pub fn validate(&self, credentials: &Credentials) -> Result<()> {
    match credentials {
      Credentials::ApiKey { key } => {
        if key.is_empty() {
          return Err(AuthError::InvalidCredentials(
            "API key is empty".to_string(),
          ));
        }
        if key.len() < 10 {
          return Err(AuthError::InvalidCredentials(
            "API key is too short".to_string(),
          ));
        }
        Ok(())
      }
      Credentials::OAuth {
        access_token,
        expires_at,
        ..
      } => {
        if access_token.is_empty() {
          return Err(AuthError::InvalidCredentials(
            "Access token is empty".to_string(),
          ));
        }
        if *expires_at < chrono::Utc::now().timestamp() as u64 {
          return Err(AuthError::TokenExpired("oauth".to_string()));
        }
        Ok(())
      }
      Credentials::Bearer { token } => {
        if token.is_empty() {
          return Err(AuthError::InvalidCredentials(
            "Bearer token is empty".to_string(),
          ));
        }
        Ok(())
      }
      Credentials::DeviceCode { .. } => {
        // Device codes are valid by definition (they're meant to be exchanged)
        Ok(())
      }
    }
  }

  /// Check if credentials exist for a provider
  pub async fn has_credentials(&self, provider_id: &str) -> bool {
    if self.resolve_credentials(provider_id).is_some() {
      return true;
    }

    self
      .storage
      .load(provider_id)
      .await
      .ok()
      .flatten()
      .is_some()
  }

  /// List all providers with stored credentials
  pub async fn list_providers(&self) -> Result<Vec<String>> {
    self.storage.list().await
  }

  /// Get credentials from environment variables
  pub fn from_env(provider_id: &str, required_vars: &[&str]) -> Result<Credentials> {
    let _ = required_vars;
    let resolver = EnvAuthResolver::new();
    resolver
      .resolve(provider_id)
      .ok_or_else(|| AuthError::NotFound(provider_id.to_string()))
  }

  /// Get storage reference
  pub fn storage(&self) -> &std::sync::Arc<dyn CredentialStorage> {
    &self.storage
  }

  /// Refresh OAuth credentials if expired
  pub async fn refresh_oauth(&self, provider_id: &str) -> Result<Credentials> {
    let provider = provider_id.to_string();
    let stored = self
      .storage
      .load(provider_id)
      .await?
      .ok_or_else(|| AuthError::NotFound(provider.clone()))?;

    // GitHub Copilot OAuth: refresh token is the GitHub access token; access token is the
    // Copilot token obtained via `copilot_internal/v2/token` (pi-mono parity).
    if provider_id == "github-copilot" || provider_id == "github-copilot-enterprise" {
      match &stored.credentials {
        Credentials::OAuth {
          refresh_token,
          enterprise_url,
          ..
        } => {
          if refresh_token.is_empty() {
            return Err(AuthError::TokenExpired(provider));
          }

          let (access_token, expires_at) =
            crate::model::oauth_connect::refresh_github_copilot_token(
              refresh_token,
              enterprise_url.as_deref(),
            )
            .await?;

          let base_url = crate::model::oauth_connect::get_github_copilot_base_url(
            Some(&access_token),
            enterprise_url.as_deref(),
          );

          let mut updated = stored.clone();
          updated.credentials = Credentials::OAuth {
            access_token,
            refresh_token: refresh_token.clone(),
            expires_at,
            account_id: None,
            enterprise_url: enterprise_url.clone(),
          };
          if updated.metadata.is_object() {
            updated.metadata["base_url"] = serde_json::Value::String(base_url);
          } else {
            updated.metadata = serde_json::json!({ "base_url": base_url });
          }

          self.storage.save(updated.clone()).await?;
          return Ok(updated.credentials);
        }
        _ => {
          return Err(AuthError::OAuthError(
            "GitHub Copilot OAuth refresh requires OAuth credentials".to_string(),
          ));
        }
      }
    }

    match &stored.credentials {
      Credentials::OAuth {
        refresh_token,
        expires_at,
        ..
      } => {
        if *expires_at <= chrono::Utc::now().timestamp() as u64 {
          if refresh_token.is_empty() {
            return Err(AuthError::TokenExpired(provider));
          }
          let config = if provider_id == "google-gemini-cli" || provider_id == "google-antigravity"
          {
            crate::model::oauth_connect::oauth_refresh_config_for_provider_with_stored(
              provider_id,
              Some(&stored),
            )?
            .ok_or_else(|| {
              AuthError::OAuthError(format!(
                "OAuth refresh is not configured for provider {}",
                provider_id
              ))
            })?
          } else {
            Self::oauth_config_for_provider(provider_id, None, None)?
          };
          let oauth = OAuthManager::new(self.storage.clone());
          oauth.refresh_token(&config, refresh_token).await?;
          if let Some(updated) = self.storage.load(provider_id).await? {
            return Ok(updated.credentials);
          }
        }
        Ok(stored.credentials)
      }
      _ => Ok(stored.credentials),
    }
  }

  /// Begin OAuth flow
  pub async fn begin_oauth(&self, request: AuthRequest) -> Result<StoredCredentials> {
    let config = Self::oauth_config_for_request(&request)?;
    let oauth = OAuthManager::new(self.storage.clone());

    if request.auth_type != AuthType::OAuth && request.auth_type != AuthType::OAuthDevice {
      return Err(AuthError::OAuthError(format!(
        "unsupported auth type for oauth flow: {:?}",
        request.auth_type
      )));
    }

    let device = oauth.start_device_flow(&config).await?;
    let stored = StoredCredentials::new(
      request.provider_id.clone(),
      Credentials::DeviceCode {
        device_code: device.device_code.clone(),
        user_code: device.user_code.clone(),
        verification_url: device.verification_uri.clone(),
        expires_in: device.expires_in,
        interval: device.interval,
      },
    );
    self.storage.save(stored.clone()).await?;
    Ok(stored)
  }

  /// Complete OAuth flow with callback
  pub async fn complete_oauth(&self, provider_id: &str, code: &str) -> Result<StoredCredentials> {
    let config = Self::oauth_config_for_provider(provider_id, None, None)?;
    let oauth = OAuthManager::new(self.storage.clone());

    let stored = self.storage.load(provider_id).await?;
    let device_code = match stored.as_ref().map(|item| &item.credentials) {
      Some(Credentials::DeviceCode {
        device_code,
        user_code,
        verification_url,
        expires_in,
        interval,
      }) => DeviceCodeResponse {
        device_code: device_code.clone(),
        user_code: user_code.clone(),
        verification_uri: verification_url.clone(),
        verification_uri_complete: None,
        expires_in: *expires_in,
        interval: *interval,
      },
      _ => DeviceCodeResponse {
        device_code: code.to_string(),
        user_code: String::new(),
        verification_uri: String::new(),
        verification_uri_complete: None,
        expires_in: 900,
        interval: 5,
      },
    };

    oauth.poll_for_token(&config, &device_code).await?;
    self
      .storage
      .load(provider_id)
      .await?
      .ok_or_else(|| AuthError::NotFound(provider_id.to_string()))
  }

  fn oauth_config_for_request(request: &AuthRequest) -> Result<OAuthConfig> {
    Self::oauth_config_for_provider(
      &request.provider_id,
      request.client_id.clone(),
      request.scopes.clone(),
    )
  }

  fn oauth_config_for_provider(
    provider_id: &str,
    client_id: Option<String>,
    scopes: Option<Vec<String>>,
  ) -> Result<OAuthConfig> {
    match provider_id {
      "github" | "github-copilot" | "github-copilot-enterprise" => {
        let fallback_client_id = std::env::var("GITHUB_OAUTH_CLIENT_ID")
          .ok()
          .or_else(|| std::env::var("GITHUB_CLIENT_ID").ok());
        let client_id = client_id.or(fallback_client_id).ok_or_else(|| {
          AuthError::OAuthError(
            "missing GitHub OAuth client id; set GITHUB_OAUTH_CLIENT_ID".to_string(),
          )
        })?;

        Ok(OAuthConfig {
          provider_id: provider_id.to_string(),
          client_id,
          client_secret: std::env::var("GITHUB_OAUTH_CLIENT_SECRET").ok(),
          auth_url: "https://github.com/login/device/code".to_string(),
          token_url: "https://github.com/login/oauth/access_token".to_string(),
          scopes: scopes.unwrap_or_else(|| {
            vec![
              "read:user".to_string(),
              "user:email".to_string(),
              "copilot".to_string(),
            ]
          }),
          redirect_uri: "urn:ietf:wg:oauth:2.0:oob".to_string(),
        })
      }
      _ => crate::model::oauth_connect::oauth_refresh_config_for_provider(provider_id)?.ok_or_else(
        || {
          AuthError::OAuthError(format!(
            "OAuth device flow is not configured for provider {}",
            provider_id
          ))
        },
      ),
    }
  }
}

impl Default for AuthManager {
  fn default() -> Self {
    Self::new().unwrap_or_else(|_| Self::memory())
  }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_memory_auth_manager() {
    let manager = AuthManager::memory();

    tokio::runtime::Runtime::new().unwrap().block_on(async {
      // Save credentials
      manager
        .save(
          "test",
          Credentials::ApiKey {
            key: "test-key".to_string(),
          },
        )
        .await
        .unwrap();

      // Load credentials
      let loaded = manager.load("test").await.unwrap().unwrap();
      assert_eq!(loaded.credentials.get_value(), "test-key");

      // Remove credentials
      manager.remove("test").await.unwrap();
      assert!(manager.load("test").await.unwrap().is_none());
    });
  }

  #[test]
  fn test_validate_api_key() {
    let manager = AuthManager::memory();

    let valid = Credentials::ApiKey {
      key: "sk-valid-key-12345".to_string(),
    };
    assert!(manager.validate(&valid).is_ok());

    let invalid = Credentials::ApiKey { key: String::new() };
    assert!(manager.validate(&invalid).is_err());
  }

  #[test]
  fn test_env_resolution() {
    let manager = AuthManager::memory().add_resolver(Box::new(EnvAuthResolver::new()));
    let provider_id = "manager_test_provider";
    let env_var = "MANAGER_TEST_PROVIDER_API_KEY";

    unsafe {
      std::env::set_var(env_var, "test-from-env");
    }

    let creds = manager.resolve_credentials(provider_id);
    assert!(creds.is_some());
    assert_eq!(creds.unwrap().get_value(), "test-from-env");

    unsafe {
      std::env::remove_var(env_var);
    }
  }

  #[test]
  fn test_load_for_runtime_registration_keeps_google_metadata() {
    let storage = std::sync::Arc::new(MemoryCredentialStorage::new());
    let manager = AuthManager::with_storage(storage).unwrap();

    tokio::runtime::Runtime::new().unwrap().block_on(async {
      let mut stored = StoredCredentials::new(
        "google-antigravity",
        Credentials::OAuth {
          access_token: "oauth-access".to_string(),
          refresh_token: "oauth-refresh".to_string(),
          expires_at: chrono::Utc::now().timestamp() as u64 + 60,
          account_id: None,
          enterprise_url: None,
        },
      );
      stored.metadata = serde_json::json!({
        "project_id": "proj-123",
      });

      manager.save_stored(stored).await.unwrap();

      let loaded = manager
        .load_for_runtime_registration("google-antigravity")
        .await
        .unwrap()
        .expect("stored credentials");

      assert_eq!(loaded.metadata["project_id"], serde_json::json!("proj-123"));
      assert_eq!(loaded.credentials.get_value(), "oauth-access");
    });
  }
}
