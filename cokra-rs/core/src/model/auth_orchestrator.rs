use thiserror::Error;

use super::ProviderRegistry;
use super::auth::AuthError;
use super::auth::AuthManager;
use super::auth::Credentials;
use super::auth::OAuthConfig;
use super::auth::StoredCredentials;
use super::oauth_connect::OAuthConnectStart;
use super::oauth_connect::PendingOAuthConnect;
use super::plugin_registry::ProviderPluginKind;
use super::provider_catalog::RuntimeRegistrationKind;
use super::provider_catalog::find_connect_provider;
use super::provider_catalog::find_connect_provider_by_runtime_id;
use super::provider_catalog::find_provider_catalog_entry;
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
  /// Find the connect-catalog entry by either a connect id (e.g. "github-copilot")
  /// or a runtime registry id (e.g. "github"). Returns the first matching visible
  /// catalog entry.
  pub fn find_connect_entry(
    provider_id: &str,
  ) -> Option<super::provider_catalog::ProviderCatalogEntry> {
    find_connect_provider(provider_id)
      .or_else(|| find_connect_provider_by_runtime_id(provider_id))
      .or_else(|| find_provider_catalog_entry(provider_id))
  }

  pub async fn connect_api_key(
    registry: &ProviderRegistry,
    config: &cokra_config::Config,
    provider_id: &str,
    api_key: String,
  ) -> Result<ProviderConnectionResult> {
    let entry = find_provider(provider_id)?;
    let stored = StoredCredentials::new(
      provider_id,
      Credentials::ApiKey {
        key: api_key.clone(),
      },
    );
    Self::persist_and_register(registry, config, entry.id, stored, Some(api_key)).await
  }

  pub async fn start_oauth(provider_id: &str) -> Result<OAuthConnectStart> {
    let entry = find_provider(provider_id)?;
    let plugin = entry
      .plugin_kind
      .ok_or_else(|| ProviderAuthError::ProviderNotFound(provider_id.to_string()))?;
    Ok(start_oauth_connect(entry.id, entry.name, plugin).await?)
  }

  pub async fn complete_oauth(
    pending: &PendingOAuthConnect,
    input: Option<&str>,
  ) -> Result<StoredCredentials> {
    Ok(complete_oauth_connect(pending, input).await?)
  }

  pub async fn persist_and_register(
    registry: &ProviderRegistry,
    config: &cokra_config::Config,
    provider_id: &str,
    stored: StoredCredentials,
    explicit_token: Option<String>,
  ) -> Result<ProviderConnectionResult> {
    let entry = find_provider(provider_id)?;

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
  pub async fn ensure_runtime_registered(
    registry: &ProviderRegistry,
    config: &cokra_config::Config,
    connect_provider_id: &str,
  ) -> Result<bool> {
    let Some(entry) = Self::find_connect_entry(connect_provider_id) else {
      return Ok(false);
    };
    if entry.runtime_registration == RuntimeRegistrationKind::None {
      return Ok(false);
    }

    let runtime_id = entry.runtime_provider_id.unwrap_or(entry.id);
    if registry.has_provider(runtime_id).await {
      return Ok(true);
    }

    let auth = AuthManager::new();
    let stored = match &auth {
      Ok(auth) => match auth.load_for_runtime_registration(entry.id).await {
        Ok(stored) => stored,
        Err(AuthError::TokenExpired(_)) => return Ok(false),
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

pub async fn start_oauth_connect(
  provider_id: &str,
  provider_name: &str,
  kind: ProviderPluginKind,
) -> std::result::Result<OAuthConnectStart, AuthError> {
  match kind {
    ProviderPluginKind::GitHubCopilot => {
      super::oauth_connect::start_github_copilot_connect_with_domain(
        provider_id,
        provider_name,
        None,
      )
      .await
    }
    ProviderPluginKind::AnthropicOAuth => {
      super::oauth_connect::start_anthropic_connect(provider_id, provider_name)
    }
    ProviderPluginKind::OpenAICodex => {
      super::oauth_connect::start_openai_codex_connect(provider_id, provider_name)
    }
    ProviderPluginKind::GoogleGeminiCli => {
      let (client_id, _secret) =
        super::oauth_connect::google_oauth_client(ProviderPluginKind::GoogleGeminiCli, None)?;
      super::oauth_connect::start_google_connect(
        provider_id,
        provider_name,
        kind,
        client_id,
        super::oauth_connect::GOOGLE_GEMINI_AUTH_URL,
        super::oauth_connect::GOOGLE_GEMINI_REDIRECT_URI,
        super::oauth_connect::GOOGLE_GEMINI_SCOPES,
      )
    }
    ProviderPluginKind::GoogleAntigravity => {
      let (client_id, _secret) =
        super::oauth_connect::google_oauth_client(ProviderPluginKind::GoogleAntigravity, None)?;
      super::oauth_connect::start_google_connect(
        provider_id,
        provider_name,
        kind,
        client_id,
        super::oauth_connect::GOOGLE_ANTIGRAVITY_AUTH_URL,
        super::oauth_connect::GOOGLE_ANTIGRAVITY_REDIRECT_URI,
        super::oauth_connect::GOOGLE_ANTIGRAVITY_SCOPES,
      )
    }
  }
}

pub async fn complete_oauth_connect(
  pending: &PendingOAuthConnect,
  input: Option<&str>,
) -> std::result::Result<StoredCredentials, AuthError> {
  match pending.kind {
    ProviderPluginKind::GitHubCopilot => {
      super::oauth_connect::complete_github_copilot_connect(pending).await
    }
    ProviderPluginKind::AnthropicOAuth => {
      super::oauth_connect::complete_anthropic_connect(pending, input.unwrap_or_default()).await
    }
    ProviderPluginKind::OpenAICodex => {
      super::oauth_connect::complete_openai_codex_connect(pending, input.unwrap_or_default()).await
    }
    ProviderPluginKind::GoogleGeminiCli => {
      let (client_id, client_secret) =
        super::oauth_connect::google_oauth_client(ProviderPluginKind::GoogleGeminiCli, None)?;
      super::oauth_connect::complete_google_connect(
        pending,
        input.unwrap_or_default(),
        client_id,
        client_secret,
        super::oauth_connect::GOOGLE_GEMINI_TOKEN_URL,
        super::oauth_connect::GOOGLE_GEMINI_REDIRECT_URI,
        false,
      )
      .await
    }
    ProviderPluginKind::GoogleAntigravity => {
      let (client_id, client_secret) =
        super::oauth_connect::google_oauth_client(ProviderPluginKind::GoogleAntigravity, None)?;
      super::oauth_connect::complete_google_connect(
        pending,
        input.unwrap_or_default(),
        client_id,
        client_secret,
        super::oauth_connect::GOOGLE_ANTIGRAVITY_TOKEN_URL,
        super::oauth_connect::GOOGLE_ANTIGRAVITY_REDIRECT_URI,
        true,
      )
      .await
    }
  }
}

pub fn uses_local_callback(kind: ProviderPluginKind) -> bool {
  super::oauth_connect::uses_local_callback(kind)
}

pub fn oauth_refresh_config_for_provider(
  provider_id: &str,
) -> std::result::Result<Option<OAuthConfig>, AuthError> {
  oauth_refresh_config_for_provider_with_stored(provider_id, None)
}

pub fn oauth_refresh_config_for_provider_with_stored(
  provider_id: &str,
  stored: Option<&StoredCredentials>,
) -> std::result::Result<Option<OAuthConfig>, AuthError> {
  super::oauth_connect::oauth_refresh_config_for_provider_with_stored(provider_id, stored)
}

fn find_provider(provider_id: &str) -> Result<super::provider_catalog::ProviderCatalogEntry> {
  find_provider_catalog_entry(provider_id)
    .ok_or_else(|| ProviderAuthError::ProviderNotFound(provider_id.to_string()))
}

#[cfg(test)]
mod tests {
  use super::ProviderAuth;
  use pretty_assertions::assert_eq;

  #[test]
  fn find_connect_entry_by_connect_id() {
    let entry = ProviderAuth::find_connect_entry("github-copilot");
    assert!(
      entry.is_some(),
      "github-copilot should be found by connect id"
    );
    assert_eq!(entry.expect("entry").id, "github-copilot");
  }

  #[test]
  fn find_connect_entry_by_runtime_id_reverse_lookup() {
    let entry = ProviderAuth::find_connect_entry("github");
    assert!(
      entry.is_some(),
      "github runtime id should resolve to github-copilot"
    );
    assert_eq!(entry.expect("entry").id, "github-copilot");
  }

  #[test]
  fn find_connect_entry_openai_codex_by_connect_id() {
    let entry = ProviderAuth::find_connect_entry("openai-codex");
    assert!(entry.is_some());
    assert_eq!(entry.expect("entry").id, "openai-codex");
  }

  #[test]
  fn find_connect_entry_anthropic_oauth_by_connect_id() {
    let entry = ProviderAuth::find_connect_entry("anthropic-oauth");
    assert!(entry.is_some());
    assert_eq!(entry.expect("entry").id, "anthropic-oauth");
  }

  #[test]
  fn find_connect_entry_unknown_id_returns_none() {
    assert!(ProviderAuth::find_connect_entry("totally-unknown-provider-xyz").is_none());
  }

  #[test]
  fn find_connect_entry_openrouter_by_connect_id() {
    let entry = ProviderAuth::find_connect_entry("openrouter");
    assert!(entry.is_some());
    assert_eq!(entry.expect("entry").id, "openrouter");
  }

  #[test]
  fn find_connect_entry_antigravity_by_runtime_id() {
    let entry = ProviderAuth::find_connect_entry("google-antigravity");
    assert!(entry.is_some());
    assert_eq!(entry.expect("entry").id, "google-antigravity");
  }
}
