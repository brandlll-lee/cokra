use super::plugin_registry::PluginRegistry;
use super::plugin_registry::ProviderPluginKind;
use super::provider::ProviderConnectMethod;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeRegistrationKind {
  None,
  OpenAI,
  OpenAICodex,
  Anthropic,
  GitHubCopilot,
  Google,
  OpenRouter,
  GoogleGeminiCli,
  GoogleAntigravity,
}

#[derive(Debug, Clone, Copy)]
pub struct OAuthClientEnv {
  pub client_id_env: &'static str,
  pub client_secret_env: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderCatalogEntry {
  pub id: &'static str,
  pub name: &'static str,
  pub connect_method: ProviderConnectMethod,
  pub env_vars: &'static [&'static str],
  pub default_models: &'static [&'static str],
  pub runtime_registration: RuntimeRegistrationKind,
  pub runtime_provider_id: Option<&'static str>,
  pub oauth_client_env: Option<OAuthClientEnv>,
  pub visible_in_connect_catalog: bool,
  pub plugin_kind: Option<ProviderPluginKind>,
}

impl ProviderCatalogEntry {
  pub fn env_vars(&self) -> Vec<String> {
    self
      .env_vars
      .iter()
      .map(|value| (*value).to_string())
      .collect()
  }

  pub fn default_models(&self) -> Vec<String> {
    self
      .default_models
      .iter()
      .map(|value| (*value).to_string())
      .collect()
  }

  pub fn primary_model_provider_id(&self) -> Option<String> {
    if let Some(runtime_provider_id) = self.runtime_provider_id {
      return Some(runtime_provider_id.to_string());
    }

    self
      .default_models
      .first()
      .and_then(|model| model.split('/').next())
      .map(ToString::to_string)
  }

  pub fn supports_model_runtime(&self) -> bool {
    self.runtime_registration != RuntimeRegistrationKind::None
  }
}

const EMPTY_ENV_VARS: &[&str] = &[];
const EMPTY_DEFAULT_MODELS: &[&str] = &[];
const OPENROUTER_DEFAULT_MODELS: &[&str] = &["anthropic/claude-haiku-4.5"];
const OPENAI_DEFAULT_MODELS: &[&str] = &["gpt-5"];
const ANTHROPIC_DEFAULT_MODELS: &[&str] = &["claude-sonnet-4-5"];
const GOOGLE_DEFAULT_MODELS: &[&str] = &["gemini-2.5-pro"];

const OPENAI_ENV_VARS: &[&str] = &["OPENAI_API_KEY"];
const ANTHROPIC_ENV_VARS: &[&str] = &["ANTHROPIC_API_KEY"];
const GOOGLE_ENV_VARS: &[&str] = &["GOOGLE_API_KEY", "GEMINI_API_KEY"];
const OPENROUTER_ENV_VARS: &[&str] = &["OPENROUTER_API_KEY"];
const GITHUB_ENV_VARS: &[&str] = &["GITHUB_COPILOT_TOKEN", "GITHUB_TOKEN"];

const BUILTIN_PROVIDER_CATALOG_ENTRIES: &[ProviderCatalogEntry] = &[
  ProviderCatalogEntry {
    id: "openrouter",
    name: "OpenRouter",
    connect_method: ProviderConnectMethod::ApiKey,
    env_vars: OPENROUTER_ENV_VARS,
    default_models: OPENROUTER_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::OpenRouter,
    runtime_provider_id: Some("openrouter"),
    oauth_client_env: None,
    visible_in_connect_catalog: true,
    plugin_kind: None,
  },
  ProviderCatalogEntry {
    id: "openai",
    name: "OpenAI",
    connect_method: ProviderConnectMethod::ApiKey,
    env_vars: OPENAI_ENV_VARS,
    default_models: OPENAI_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::OpenAI,
    runtime_provider_id: Some("openai"),
    oauth_client_env: None,
    visible_in_connect_catalog: true,
    plugin_kind: None,
  },
  ProviderCatalogEntry {
    id: "anthropic",
    name: "Anthropic",
    connect_method: ProviderConnectMethod::ApiKey,
    env_vars: ANTHROPIC_ENV_VARS,
    default_models: ANTHROPIC_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::Anthropic,
    runtime_provider_id: Some("anthropic"),
    oauth_client_env: None,
    visible_in_connect_catalog: true,
    plugin_kind: None,
  },
  ProviderCatalogEntry {
    id: "google",
    name: "Google",
    connect_method: ProviderConnectMethod::ApiKey,
    env_vars: GOOGLE_ENV_VARS,
    default_models: GOOGLE_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::Google,
    runtime_provider_id: Some("google"),
    oauth_client_env: None,
    visible_in_connect_catalog: true,
    plugin_kind: None,
  },
  ProviderCatalogEntry {
    id: "github",
    name: "GitHub",
    connect_method: ProviderConnectMethod::ApiKey,
    env_vars: GITHUB_ENV_VARS,
    default_models: EMPTY_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::None,
    runtime_provider_id: Some("github"),
    oauth_client_env: None,
    visible_in_connect_catalog: false,
    plugin_kind: None,
  },
];

pub fn provider_catalog_entries() -> Vec<ProviderCatalogEntry> {
  let mut entries = BUILTIN_PROVIDER_CATALOG_ENTRIES.to_vec();
  entries.extend(
    PluginRegistry::plugins()
      .iter()
      .map(|plugin| plugin.catalog),
  );
  entries
}

pub fn find_provider_catalog_entry(provider_id: &str) -> Option<ProviderCatalogEntry> {
  BUILTIN_PROVIDER_CATALOG_ENTRIES
    .iter()
    .copied()
    .find(|descriptor| descriptor.id == provider_id)
    .or_else(|| PluginRegistry::find(provider_id).map(|plugin| plugin.catalog))
}

pub fn find_provider_catalog_entry_by_plugin_kind(
  kind: ProviderPluginKind,
) -> Option<ProviderCatalogEntry> {
  PluginRegistry::find_by_kind(kind).map(|plugin| plugin.catalog)
}

pub fn connect_provider_catalog() -> Vec<ProviderCatalogEntry> {
  provider_catalog_entries()
    .into_iter()
    .filter(|descriptor| descriptor.visible_in_connect_catalog)
    .collect()
}

pub fn find_connect_provider(provider_id: &str) -> Option<ProviderCatalogEntry> {
  let descriptor = find_provider_catalog_entry(provider_id)?;
  descriptor.visible_in_connect_catalog.then_some(descriptor)
}

pub fn find_connect_provider_by_runtime_id(provider_id: &str) -> Option<ProviderCatalogEntry> {
  provider_catalog_entries().into_iter().find(|descriptor| {
    descriptor.runtime_provider_id == Some(provider_id) && descriptor.visible_in_connect_catalog
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use pretty_assertions::assert_eq;

  #[test]
  fn visible_connect_catalog_entries_are_connectable() {
    let visible = connect_provider_catalog();

    assert!(!visible.is_empty());
    assert!(visible.iter().all(|descriptor| {
      descriptor.connect_method == ProviderConnectMethod::ApiKey
        || descriptor.connect_method == ProviderConnectMethod::OAuth
    }));
  }

  #[test]
  fn plugin_catalog_entries_expose_google_oauth_client_env() {
    let descriptor = find_provider_catalog_entry("google-antigravity").expect("descriptor");
    assert!(descriptor.oauth_client_env.is_some());
    assert_eq!(
      descriptor.plugin_kind,
      Some(ProviderPluginKind::GoogleAntigravity)
    );
  }

  #[test]
  fn github_runtime_provider_uses_explicit_env_mapping() {
    let descriptor = find_provider_catalog_entry("github").expect("descriptor");

    assert_eq!(
      descriptor.env_vars(),
      vec!["GITHUB_COPILOT_TOKEN", "GITHUB_TOKEN"]
    );
    assert!(!descriptor.visible_in_connect_catalog);
  }
}
