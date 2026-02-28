//! OAuth device flow support.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

use super::storage::CredentialStorage;
use super::{AuthError, Credentials, Result, StoredCredentials};

/// OAuth provider configuration.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
  pub provider_id: String,
  pub client_id: String,
  pub client_secret: Option<String>,
  pub auth_url: String,
  pub token_url: String,
  pub scopes: Vec<String>,
  pub redirect_uri: String,
}

/// Device authorization response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
  pub device_code: String,
  pub user_code: String,
  pub verification_uri: String,
  #[serde(default)]
  pub verification_uri_complete: Option<String>,
  pub expires_in: u64,
  #[serde(default = "default_interval")]
  pub interval: u64,
}

/// OAuth token payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthToken {
  pub access_token: String,
  #[serde(default)]
  pub refresh_token: Option<String>,
  pub expires_in: u64,
  pub token_type: String,
  #[serde(default)]
  pub scope: Option<String>,
}

/// OAuth polling error payload.
#[derive(Debug, Clone, Deserialize)]
struct OAuthErrorResponse {
  #[serde(default)]
  error: String,
  #[serde(default)]
  error_description: Option<String>,
}

/// OAuth manager for device flow.
pub struct OAuthManager {
  storage: Arc<dyn CredentialStorage>,
  client: reqwest::Client,
}

impl OAuthManager {
  pub fn new(storage: Arc<dyn CredentialStorage>) -> Self {
    let client = reqwest::Client::builder()
      .timeout(Duration::from_secs(30))
      .build()
      .unwrap_or_else(|_| reqwest::Client::new());
    Self { storage, client }
  }

  pub fn with_client(storage: Arc<dyn CredentialStorage>, client: reqwest::Client) -> Self {
    Self { storage, client }
  }

  /// Starts OAuth device flow.
  pub async fn start_device_flow(&self, config: &OAuthConfig) -> Result<DeviceCodeResponse> {
    let scope = config.scopes.join(" ");
    let mut form: Vec<(String, String)> = vec![
      ("client_id".to_string(), config.client_id.clone()),
      ("scope".to_string(), scope),
    ];

    if let Some(secret) = &config.client_secret {
      form.push(("client_secret".to_string(), secret.clone()));
    }

    let response = self
      .client
      .post(&config.auth_url)
      .header("Accept", "application/json")
      .form(&form)
      .send()
      .await
      .map_err(|e| AuthError::OAuthError(format!("failed to start device flow: {e}")))?;

    if !response.status().is_success() {
      let status = response.status();
      let text = response.text().await.unwrap_or_default();
      return Err(AuthError::OAuthError(format!(
        "device flow request failed (HTTP {}): {}",
        status, text
      )));
    }

    response
      .json::<DeviceCodeResponse>()
      .await
      .map_err(|e| AuthError::OAuthError(format!("failed to parse device flow response: {e}")))
  }

  /// Polls token endpoint until token is available.
  pub async fn poll_for_token(
    &self,
    config: &OAuthConfig,
    device_code: &DeviceCodeResponse,
  ) -> Result<OAuthToken> {
    let start = std::time::Instant::now();
    let mut interval = device_code.interval.max(1);

    loop {
      if start.elapsed().as_secs() > device_code.expires_in {
        return Err(AuthError::Timeout);
      }

      let mut form: Vec<(String, String)> = vec![
        ("client_id".to_string(), config.client_id.clone()),
        ("device_code".to_string(), device_code.device_code.clone()),
        (
          "grant_type".to_string(),
          "urn:ietf:params:oauth:grant-type:device_code".to_string(),
        ),
      ];
      if let Some(secret) = &config.client_secret {
        form.push(("client_secret".to_string(), secret.clone()));
      }

      let response = self
        .client
        .post(&config.token_url)
        .header("Accept", "application/json")
        .form(&form)
        .send()
        .await
        .map_err(|e| AuthError::OAuthError(format!("failed polling token endpoint: {e}")))?;

      if response.status().is_success() {
        let token = response
          .json::<OAuthToken>()
          .await
          .map_err(|e| AuthError::OAuthError(format!("failed to parse token response: {e}")))?;

        let expires_at = chrono::Utc::now().timestamp() as u64 + token.expires_in;
        let credentials = Credentials::OAuth {
          access_token: token.access_token.clone(),
          refresh_token: token.refresh_token.clone().unwrap_or_default(),
          expires_at,
          account_id: None,
          enterprise_url: None,
        };
        self
          .storage
          .save(StoredCredentials::new(
            config.provider_id.clone(),
            credentials,
          ))
          .await?;
        return Ok(token);
      }

      let error_payload =
        response
          .json::<OAuthErrorResponse>()
          .await
          .unwrap_or(OAuthErrorResponse {
            error: "unknown_error".to_string(),
            error_description: None,
          });

      match error_payload.error.as_str() {
        "authorization_pending" => {}
        "slow_down" => {
          interval += 5;
        }
        "expired_token" => return Err(AuthError::Timeout),
        other => {
          let description = error_payload.error_description.unwrap_or_default();
          return Err(AuthError::OAuthError(format!("{other}: {description}")));
        }
      }

      tokio::time::sleep(Duration::from_secs(interval)).await;
    }
  }

  /// Refreshes an OAuth token using refresh token.
  pub async fn refresh_token(
    &self,
    config: &OAuthConfig,
    refresh_token: &str,
  ) -> Result<OAuthToken> {
    let mut form: Vec<(String, String)> = vec![
      ("client_id".to_string(), config.client_id.clone()),
      ("refresh_token".to_string(), refresh_token.to_string()),
      ("grant_type".to_string(), "refresh_token".to_string()),
    ];
    if let Some(secret) = &config.client_secret {
      form.push(("client_secret".to_string(), secret.clone()));
    }

    let response = self
      .client
      .post(&config.token_url)
      .header("Accept", "application/json")
      .form(&form)
      .send()
      .await
      .map_err(|e| AuthError::OAuthError(format!("failed to refresh token: {e}")))?;

    if !response.status().is_success() {
      let status = response.status();
      let text = response.text().await.unwrap_or_default();
      return Err(AuthError::OAuthError(format!(
        "refresh token request failed (HTTP {}): {}",
        status, text
      )));
    }

    let token = response
      .json::<OAuthToken>()
      .await
      .map_err(|e| AuthError::OAuthError(format!("failed to parse refreshed token: {e}")))?;

    let expires_at = chrono::Utc::now().timestamp() as u64 + token.expires_in;
    let credentials = Credentials::OAuth {
      access_token: token.access_token.clone(),
      refresh_token: token
        .refresh_token
        .clone()
        .unwrap_or_else(|| refresh_token.to_string()),
      expires_at,
      account_id: None,
      enterprise_url: None,
    };
    self
      .storage
      .save(StoredCredentials::new(
        config.provider_id.clone(),
        credentials,
      ))
      .await?;

    Ok(token)
  }
}

fn default_interval() -> u64 {
  5
}
