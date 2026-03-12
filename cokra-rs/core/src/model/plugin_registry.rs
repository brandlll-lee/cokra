use super::provider::ProviderConnectMethod;
use super::provider_catalog::OAuthClientEnv;
use super::provider_catalog::ProviderCatalogEntry;
use super::provider_catalog::RuntimeRegistrationKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPluginKind {
  AnthropicOAuth,
  GitHubCopilot,
  GoogleGeminiCli,
  GoogleAntigravity,
  OpenAICodex,
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderPlugin {
  pub kind: ProviderPluginKind,
  pub catalog: ProviderCatalogEntry,
}

const ANTHROPIC_OAUTH_DEFAULT_MODELS: &[&str] = &["claude-sonnet-4-5"];
const GITHUB_COPILOT_DEFAULT_MODELS: &[&str] = &["gpt-4o"];
const GOOGLE_GEMINI_CLI_DEFAULT_MODELS: &[&str] = &[
  "gemini-2.5-pro",
  "gemini-2.5-flash",
  "gemini-3-flash-preview",
  "gemini-3-pro-preview",
];
const GOOGLE_ANTIGRAVITY_DEFAULT_MODELS: &[&str] = &[
  "gemini-3-pro-high",
  "gemini-3-flash",
  "claude-sonnet-4-5-thinking",
  "claude-opus-4-5-thinking",
  "gpt-oss-120b-medium",
];
const OPENAI_CODEX_DEFAULT_MODELS: &[&str] = &[
  "gpt-5.3-codex",
  "gpt-5.2-codex",
  "gpt-5.1-codex",
  "gpt-5.1-codex-mini",
];

const GOOGLE_GEMINI_CLIENT_ENV: OAuthClientEnv = OAuthClientEnv {
  client_id_env: "COKRA_GOOGLE_GEMINI_CLIENT_ID",
  client_secret_env: Some("COKRA_GOOGLE_GEMINI_CLIENT_SECRET"),
};

const GOOGLE_ANTIGRAVITY_CLIENT_ENV: OAuthClientEnv = OAuthClientEnv {
  client_id_env: "COKRA_GOOGLE_ANTIGRAVITY_CLIENT_ID",
  client_secret_env: Some("COKRA_GOOGLE_ANTIGRAVITY_CLIENT_SECRET"),
};

const PROVIDER_PLUGINS: &[ProviderPlugin] = &[
  ProviderPlugin {
    kind: ProviderPluginKind::AnthropicOAuth,
    catalog: ProviderCatalogEntry {
      id: "anthropic-oauth",
      name: "Anthropic (Claude Pro/Max)",
      connect_method: ProviderConnectMethod::OAuth,
      env_vars: &[],
      default_models: ANTHROPIC_OAUTH_DEFAULT_MODELS,
      runtime_registration: RuntimeRegistrationKind::Anthropic,
      runtime_provider_id: Some("anthropic"),
      oauth_client_env: None,
      visible_in_connect_catalog: true,
      plugin_kind: Some(ProviderPluginKind::AnthropicOAuth),
    },
  },
  ProviderPlugin {
    kind: ProviderPluginKind::GitHubCopilot,
    catalog: ProviderCatalogEntry {
      id: "github-copilot",
      name: "GitHub Copilot",
      connect_method: ProviderConnectMethod::OAuth,
      env_vars: &[],
      default_models: GITHUB_COPILOT_DEFAULT_MODELS,
      runtime_registration: RuntimeRegistrationKind::GitHubCopilot,
      runtime_provider_id: Some("github"),
      oauth_client_env: None,
      visible_in_connect_catalog: true,
      plugin_kind: Some(ProviderPluginKind::GitHubCopilot),
    },
  },
  ProviderPlugin {
    kind: ProviderPluginKind::GitHubCopilot,
    catalog: ProviderCatalogEntry {
      id: "github-copilot-enterprise",
      name: "GitHub Copilot Enterprise",
      connect_method: ProviderConnectMethod::OAuth,
      env_vars: &[],
      default_models: GITHUB_COPILOT_DEFAULT_MODELS,
      runtime_registration: RuntimeRegistrationKind::GitHubCopilot,
      runtime_provider_id: Some("github"),
      oauth_client_env: None,
      visible_in_connect_catalog: false,
      plugin_kind: Some(ProviderPluginKind::GitHubCopilot),
    },
  },
  ProviderPlugin {
    kind: ProviderPluginKind::GoogleGeminiCli,
    catalog: ProviderCatalogEntry {
      id: "google-gemini-cli",
      name: "Google Cloud Code Assist (Gemini CLI)",
      connect_method: ProviderConnectMethod::OAuth,
      env_vars: &[],
      default_models: GOOGLE_GEMINI_CLI_DEFAULT_MODELS,
      runtime_registration: RuntimeRegistrationKind::GoogleGeminiCli,
      runtime_provider_id: Some("google-gemini-cli"),
      oauth_client_env: Some(GOOGLE_GEMINI_CLIENT_ENV),
      visible_in_connect_catalog: true,
      plugin_kind: Some(ProviderPluginKind::GoogleGeminiCli),
    },
  },
  ProviderPlugin {
    kind: ProviderPluginKind::GoogleAntigravity,
    catalog: ProviderCatalogEntry {
      id: "google-antigravity",
      name: "Antigravity (Gemini 3, Claude, GPT-OSS)",
      connect_method: ProviderConnectMethod::OAuth,
      env_vars: &[],
      default_models: GOOGLE_ANTIGRAVITY_DEFAULT_MODELS,
      runtime_registration: RuntimeRegistrationKind::GoogleAntigravity,
      runtime_provider_id: Some("google-antigravity"),
      oauth_client_env: Some(GOOGLE_ANTIGRAVITY_CLIENT_ENV),
      visible_in_connect_catalog: true,
      plugin_kind: Some(ProviderPluginKind::GoogleAntigravity),
    },
  },
  ProviderPlugin {
    kind: ProviderPluginKind::OpenAICodex,
    catalog: ProviderCatalogEntry {
      id: "openai-codex",
      name: "ChatGPT Plus/Pro (Codex Subscription)",
      connect_method: ProviderConnectMethod::OAuth,
      env_vars: &[],
      default_models: OPENAI_CODEX_DEFAULT_MODELS,
      runtime_registration: RuntimeRegistrationKind::OpenAICodex,
      runtime_provider_id: Some("openai"),
      oauth_client_env: None,
      visible_in_connect_catalog: true,
      plugin_kind: Some(ProviderPluginKind::OpenAICodex),
    },
  },
];

pub struct PluginRegistry;

impl PluginRegistry {
  pub fn plugins() -> &'static [ProviderPlugin] {
    PROVIDER_PLUGINS
  }

  pub fn find(provider_id: &str) -> Option<&'static ProviderPlugin> {
    PROVIDER_PLUGINS
      .iter()
      .find(|plugin| plugin.catalog.id == provider_id)
  }

  pub fn find_by_kind(kind: ProviderPluginKind) -> Option<&'static ProviderPlugin> {
    PROVIDER_PLUGINS.iter().find(|plugin| plugin.kind == kind)
  }
}
