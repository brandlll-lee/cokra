use thiserror::Error;

use super::ProviderRegistry;
use super::auth::AuthError;
use super::auth::AuthManager;
use super::auth::Credentials;
use super::auth::StoredCredentials;
use super::oauth_connect::OAuthConnectStart;
use super::oauth_connect::PendingOAuthConnect;
use super::plugin_registry::PluginRegistry;
use super::providers::register_provider_by_registration;
use super::providers::registration_token_for_stored;

#[derive(Debug, Error)]
pub enum ProviderAuthError {
  #[error(transparent)]
  Auth(#[from] AuthError),
  #[error(transparent)]
  Model(#[from] super::error::ModelError),
  #[error("Provider not found: {0}")]
  ProviderNotFound(String),
}

pub type Result<T> = std::result::Result<T, ProviderAuthError>;

#[derive(Debug, Clone)]
pub struct ProviderConnectionResult {
  pub provider_id: String,
  pub provider_name: String,
  pub stored: StoredCredentials,
  pub save_error: Option<String>,
  pub runtime_registered: bool,
}

pub struct ProviderAuth;

impl ProviderAuth {
  pub async fn connect_api_key(
    registry: &ProviderRegistry,
    config: &cokra_config::Config,
    provider_id: &str,
    api_key: String,
  ) -> Result<ProviderConnectionResult> {
    let entry = PluginRegistry::find(provider_id)
      .ok_or_else(|| ProviderAuthError::ProviderNotFound(provider_id.to_string()))?;
    let stored = StoredCredentials::new(
      provider_id,
      Credentials::ApiKey {
        key: api_key.clone(),
      },
    );
    Self::persist_and_register(registry, config, entry.id, stored, Some(api_key)).await
  }

  pub async fn start_oauth(provider_id: &str) -> Result<OAuthConnectStart> {
    let entry = PluginRegistry::find(provider_id)
      .ok_or_else(|| ProviderAuthError::ProviderNotFound(provider_id.to_string()))?;
    let kind = entry
      .oauth_provider
      .ok_or_else(|| ProviderAuthError::ProviderNotFound(provider_id.to_string()))?;
    Ok(super::oauth_connect::start_oauth_connect(entry.id, entry.name, kind).await?)
  }

  pub async fn complete_oauth(
    pending: &PendingOAuthConnect,
    input: Option<&str>,
  ) -> Result<StoredCredentials> {
    Ok(super::oauth_connect::complete_oauth_connect(pending, input).await?)
  }

  pub async fn persist_and_register(
    registry: &ProviderRegistry,
    config: &cokra_config::Config,
    provider_id: &str,
    stored: StoredCredentials,
    explicit_token: Option<String>,
  ) -> Result<ProviderConnectionResult> {
    let entry = PluginRegistry::find(provider_id)
      .ok_or_else(|| ProviderAuthError::ProviderNotFound(provider_id.to_string()))?;

    let auth = AuthManager::new();
    let save_error = match &auth {
      Ok(auth) => auth
        .save_stored(stored.clone())
        .await
        .err()
        .map(|err| err.to_string()),
      Err(err) => Some(err.to_string()),
    };

    let runtime_stored = match (&explicit_token, &auth) {
      (None, Ok(auth)) => auth.load_for_runtime_registration(entry.id).await?,
      _ => Some(stored.clone()),
    };

    let runtime_registered = if let Some(token) = explicit_token.or_else(|| {
      runtime_stored
        .as_ref()
        .and_then(|stored| registration_token_for_stored(entry.runtime_registration, stored))
    }) {
      register_provider_by_registration(
        registry,
        config,
        entry.runtime_registration,
        token,
        Some(entry.id),
        runtime_stored.as_ref(),
        None,
      )
      .await?;
      true
    } else {
      false
    };

    Ok(ProviderConnectionResult {
      provider_id: entry.id.to_string(),
      provider_name: entry.name.to_string(),
      stored,
      save_error,
      runtime_registered,
    })
  }

  /// Ensure a connected (catalog) provider is registered in the runtime registry.
  ///
  /// The Connect catalog tracks authentication separately from the runtime provider
  /// registry. For OAuth providers in particular, we want "connected" models to be
  /// selectable immediately (pi-mono parity), and only wire the runtime provider
  /// lazily when needed.
  pub async fn ensure_runtime_registered(
    registry: &ProviderRegistry,
    config: &cokra_config::Config,
    runtime_provider_id: &str,
  ) -> Result<bool> {
    if registry.has_provider(runtime_provider_id).await {
      return Ok(true);
    }

    let entry = PluginRegistry::entries()
      .into_iter()
      .find(|entry| entry.runtime_provider_id == Some(runtime_provider_id))
      .or_else(|| PluginRegistry::find(runtime_provider_id));
    let Some(entry) = entry else {
      return Ok(false);
    };
    if entry.runtime_registration == super::connect_catalog::RuntimeRegistrationKind::None {
      return Ok(false);
    }

    let auth = AuthManager::new();
    let stored = match &auth {
      Ok(auth) => match auth.load_for_runtime_registration(entry.id).await {
        Ok(stored) => stored,
        Err(AuthError::TokenExpired(_)) => {
          // Tradeoff: runtime wiring is best-effort; if OAuth cannot be refreshed,
          // let the caller fall back to the provider's connect flow.
          return Ok(false);
        }
        Err(err) => return Err(err.into()),
      },
      Err(_) => None,
    };
    let Some(stored) = stored else {
      return Ok(false);
    };
    let Some(token) = registration_token_for_stored(entry.runtime_registration, &stored) else {
      return Ok(false);
    };

    register_provider_by_registration(
      registry,
      config,
      entry.runtime_registration,
      token,
      Some(entry.id),
      Some(&stored),
      None,
    )
    .await?;

    Ok(registry.has_provider(runtime_provider_id).await)
  }
}
