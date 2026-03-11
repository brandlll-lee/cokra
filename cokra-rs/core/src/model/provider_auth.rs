use thiserror::Error;

use super::ProviderRegistry;
use super::auth::AuthError;
use super::auth::AuthManager;
use super::auth::Credentials;
use super::auth::StoredCredentials;
use super::auth::auth_provider_descriptors;
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

#[cfg(test)]
mod tests {
  use super::ProviderAuth;

  #[test]
  fn find_connect_entry_by_connect_id() {
    let entry = ProviderAuth::find_connect_entry("github-copilot");
    assert!(entry.is_some(), "github-copilot should be found by connect id");
    let entry = entry.unwrap();
    assert_eq!(entry.id, "github-copilot");
  }

  #[test]
  fn find_connect_entry_by_runtime_id_reverse_lookup() {
    // "github" is the runtime provider id, not a connect catalog id.
    // Should reverse-lookup to "github-copilot".
    let entry = ProviderAuth::find_connect_entry("github");
    assert!(entry.is_some(), "github runtime id should resolve to github-copilot");
    let entry = entry.unwrap();
    assert_eq!(entry.id, "github-copilot");
  }

  #[test]
  fn find_connect_entry_openai_codex_by_connect_id() {
    let entry = ProviderAuth::find_connect_entry("openai-codex");
    assert!(entry.is_some());
    assert_eq!(entry.unwrap().id, "openai-codex");
  }

  #[test]
  fn find_connect_entry_anthropic_oauth_by_connect_id() {
    let entry = ProviderAuth::find_connect_entry("anthropic-oauth");
    assert!(entry.is_some());
    assert_eq!(entry.unwrap().id, "anthropic-oauth");
  }

  #[test]
  fn find_connect_entry_unknown_id_returns_none() {
    let entry = ProviderAuth::find_connect_entry("totally-unknown-provider-xyz");
    assert!(entry.is_none());
  }

  #[test]
  fn find_connect_entry_openrouter_by_connect_id() {
    // openrouter: connect id and runtime id are the same, confirm it works.
    let entry = ProviderAuth::find_connect_entry("openrouter");
    assert!(entry.is_some());
    assert_eq!(entry.unwrap().id, "openrouter");
  }

  #[test]
  fn find_connect_entry_antigravity_by_runtime_id() {
    // google-antigravity: connect id == runtime id for this provider.
    let entry = ProviderAuth::find_connect_entry("google-antigravity");
    assert!(entry.is_some());
    assert_eq!(entry.unwrap().id, "google-antigravity");
  }
}

impl ProviderAuth {
  /// Find the connect-catalog entry by either a connect id (e.g. "github-copilot")
  /// or a runtime registry id (e.g. "github"). Returns the first matching visible
  /// catalog entry.
  pub fn find_connect_entry(
    provider_id: &str,
  ) -> Option<super::connect_catalog::ConnectProviderCatalogEntry> {
    // Direct connect-catalog lookup first.
    if let Some(entry) = PluginRegistry::find(provider_id) {
      return Some(entry);
    }
    // Reverse lookup: find catalog entry whose runtime_provider_id matches.
    auth_provider_descriptors()
      .iter()
      .find(|d| d.runtime_provider_id == Some(provider_id) && d.visible_in_connect_catalog)
      .and_then(|d| PluginRegistry::find(d.id))
  }

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
  ///
  /// `connect_provider_id` is the connect-catalog provider id (e.g. "github-copilot"),
  /// which may differ from the runtime registry id (e.g. "github").
  pub async fn ensure_runtime_registered(
    registry: &ProviderRegistry,
    config: &cokra_config::Config,
    connect_provider_id: &str,
  ) -> Result<bool> {
    // Supports both connect catalog id ("github-copilot") and runtime id ("github").
    let entry = Self::find_connect_entry(connect_provider_id);
    let Some(entry) = entry else {
      return Ok(false);
    };
    if entry.runtime_registration == super::connect_catalog::RuntimeRegistrationKind::None {
      return Ok(false);
    }

    // The runtime provider id is what gets registered in ProviderRegistry (e.g. "github").
    // Fall back to the connect id when no explicit runtime_provider_id is set.
    let runtime_id = entry.runtime_provider_id.unwrap_or(entry.id);
    if registry.has_provider(runtime_id).await {
      return Ok(true);
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

    Ok(registry.has_provider(runtime_id).await)
  }
}
