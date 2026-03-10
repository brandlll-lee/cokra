use crate::model::oauth_connect::OAuthProviderKind;
use crate::model::provider::ProviderConnectMethod;

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

#[derive(Debug, Clone)]
pub struct AuthProviderDescriptor {
  pub id: &'static str,
  pub name: &'static str,
  pub connect_method: ProviderConnectMethod,
  pub env_vars: &'static [&'static str],
  pub default_models: &'static [&'static str],
  pub runtime_registration: RuntimeRegistrationKind,
  pub runtime_provider_id: Option<&'static str>,
  pub oauth_provider: Option<OAuthProviderKind>,
  pub oauth_client_env: Option<OAuthClientEnv>,
  pub visible_in_connect_catalog: bool,
}

impl AuthProviderDescriptor {
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
const OPENROUTER_DEFAULT_MODELS: &[&str] = &["anthropic/claude-haiku-4.5"];
const OPENAI_DEFAULT_MODELS: &[&str] = &["gpt-5"];
const ANTHROPIC_DEFAULT_MODELS: &[&str] = &["claude-sonnet-4-5"];
const GOOGLE_DEFAULT_MODELS: &[&str] = &["gemini-2.5-pro"];

const OPENAI_ENV_VARS: &[&str] = &["OPENAI_API_KEY"];
const ANTHROPIC_ENV_VARS: &[&str] = &["ANTHROPIC_API_KEY"];
const GOOGLE_ENV_VARS: &[&str] = &["GOOGLE_API_KEY", "GEMINI_API_KEY"];
const OPENROUTER_ENV_VARS: &[&str] = &["OPENROUTER_API_KEY"];
const GITHUB_ENV_VARS: &[&str] = &["GITHUB_COPILOT_TOKEN", "GITHUB_TOKEN"];

const GOOGLE_GEMINI_CLIENT_ENV: OAuthClientEnv = OAuthClientEnv {
  client_id_env: "COKRA_GOOGLE_GEMINI_CLIENT_ID",
  client_secret_env: Some("COKRA_GOOGLE_GEMINI_CLIENT_SECRET"),
};

const GOOGLE_ANTIGRAVITY_CLIENT_ENV: OAuthClientEnv = OAuthClientEnv {
  client_id_env: "COKRA_GOOGLE_ANTIGRAVITY_CLIENT_ID",
  client_secret_env: Some("COKRA_GOOGLE_ANTIGRAVITY_CLIENT_SECRET"),
};

const AUTH_PROVIDER_DESCRIPTORS: &[AuthProviderDescriptor] = &[
  AuthProviderDescriptor {
    id: "anthropic-oauth",
    name: "Anthropic (Claude Pro/Max)",
    connect_method: ProviderConnectMethod::OAuth,
    env_vars: EMPTY_ENV_VARS,
    default_models: ANTHROPIC_OAUTH_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::Anthropic,
    runtime_provider_id: Some("anthropic"),
    oauth_provider: Some(OAuthProviderKind::Anthropic),
    oauth_client_env: None,
    visible_in_connect_catalog: true,
  },
  AuthProviderDescriptor {
    id: "github-copilot",
    name: "GitHub Copilot",
    connect_method: ProviderConnectMethod::OAuth,
    env_vars: EMPTY_ENV_VARS,
    default_models: GITHUB_COPILOT_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::GitHubCopilot,
    runtime_provider_id: Some("github"),
    oauth_provider: Some(OAuthProviderKind::GitHubCopilot),
    oauth_client_env: None,
    visible_in_connect_catalog: true,
  },
  AuthProviderDescriptor {
    id: "github-copilot-enterprise",
    name: "GitHub Copilot Enterprise",
    connect_method: ProviderConnectMethod::OAuth,
    env_vars: EMPTY_ENV_VARS,
    default_models: GITHUB_COPILOT_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::GitHubCopilot,
    runtime_provider_id: Some("github"),
    oauth_provider: Some(OAuthProviderKind::GitHubCopilot),
    oauth_client_env: None,
    visible_in_connect_catalog: false,
  },
  AuthProviderDescriptor {
    id: "google-gemini-cli",
    name: "Google Cloud Code Assist (Gemini CLI)",
    connect_method: ProviderConnectMethod::OAuth,
    env_vars: EMPTY_ENV_VARS,
    default_models: GOOGLE_GEMINI_CLI_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::GoogleGeminiCli,
    runtime_provider_id: Some("google-gemini-cli"),
    oauth_provider: Some(OAuthProviderKind::GoogleGeminiCli),
    oauth_client_env: Some(GOOGLE_GEMINI_CLIENT_ENV),
    visible_in_connect_catalog: true,
  },
  AuthProviderDescriptor {
    id: "google-antigravity",
    name: "Antigravity (Gemini 3, Claude, GPT-OSS)",
    connect_method: ProviderConnectMethod::OAuth,
    env_vars: EMPTY_ENV_VARS,
    default_models: GOOGLE_ANTIGRAVITY_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::GoogleAntigravity,
    runtime_provider_id: Some("google-antigravity"),
    oauth_provider: Some(OAuthProviderKind::GoogleAntigravity),
    oauth_client_env: Some(GOOGLE_ANTIGRAVITY_CLIENT_ENV),
    visible_in_connect_catalog: true,
  },
  AuthProviderDescriptor {
    id: "openai-codex",
    name: "ChatGPT Plus/Pro (Codex Subscription)",
    connect_method: ProviderConnectMethod::OAuth,
    env_vars: EMPTY_ENV_VARS,
    default_models: OPENAI_CODEX_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::OpenAICodex,
    runtime_provider_id: Some("openai"),
    oauth_provider: Some(OAuthProviderKind::OpenAICodex),
    oauth_client_env: None,
    visible_in_connect_catalog: true,
  },
  AuthProviderDescriptor {
    id: "openrouter",
    name: "OpenRouter",
    connect_method: ProviderConnectMethod::ApiKey,
    env_vars: OPENROUTER_ENV_VARS,
    default_models: OPENROUTER_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::OpenRouter,
    runtime_provider_id: Some("openrouter"),
    oauth_provider: None,
    oauth_client_env: None,
    visible_in_connect_catalog: true,
  },
  AuthProviderDescriptor {
    id: "openai",
    name: "OpenAI",
    connect_method: ProviderConnectMethod::ApiKey,
    env_vars: OPENAI_ENV_VARS,
    default_models: OPENAI_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::OpenAI,
    runtime_provider_id: Some("openai"),
    oauth_provider: None,
    oauth_client_env: None,
    visible_in_connect_catalog: true,
  },
  AuthProviderDescriptor {
    id: "anthropic",
    name: "Anthropic",
    connect_method: ProviderConnectMethod::ApiKey,
    env_vars: ANTHROPIC_ENV_VARS,
    default_models: ANTHROPIC_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::Anthropic,
    runtime_provider_id: Some("anthropic"),
    oauth_provider: None,
    oauth_client_env: None,
    visible_in_connect_catalog: true,
  },
  AuthProviderDescriptor {
    id: "google",
    name: "Google",
    connect_method: ProviderConnectMethod::ApiKey,
    env_vars: GOOGLE_ENV_VARS,
    default_models: GOOGLE_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::Google,
    runtime_provider_id: Some("google"),
    oauth_provider: None,
    oauth_client_env: None,
    visible_in_connect_catalog: true,
  },
  AuthProviderDescriptor {
    id: "github",
    name: "GitHub",
    connect_method: ProviderConnectMethod::ApiKey,
    env_vars: GITHUB_ENV_VARS,
    default_models: EMPTY_DEFAULT_MODELS,
    runtime_registration: RuntimeRegistrationKind::None,
    runtime_provider_id: Some("github"),
    oauth_provider: None,
    oauth_client_env: None,
    visible_in_connect_catalog: false,
  },
];

pub fn auth_provider_descriptors() -> &'static [AuthProviderDescriptor] {
  AUTH_PROVIDER_DESCRIPTORS
}

pub fn find_auth_provider(provider_id: &str) -> Option<&'static AuthProviderDescriptor> {
  AUTH_PROVIDER_DESCRIPTORS
    .iter()
    .find(|descriptor| descriptor.id == provider_id)
}

pub fn find_auth_provider_by_oauth_kind(
  kind: OAuthProviderKind,
) -> Option<&'static AuthProviderDescriptor> {
  AUTH_PROVIDER_DESCRIPTORS
    .iter()
    .find(|descriptor| descriptor.oauth_provider == Some(kind))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn visible_connect_catalog_entries_are_connectable() {
    let visible = auth_provider_descriptors()
      .iter()
      .filter(|descriptor| descriptor.visible_in_connect_catalog)
      .collect::<Vec<_>>();

    assert!(!visible.is_empty());
    assert!(visible.iter().all(|descriptor| {
      descriptor.connect_method == ProviderConnectMethod::ApiKey
        || descriptor.connect_method == ProviderConnectMethod::OAuth
    }));
  }

  #[test]
  fn antigravity_descriptor_declares_env_mapping_for_oauth_client() {
    let descriptor = find_auth_provider("google-antigravity").expect("descriptor");
    assert!(descriptor.oauth_client_env.is_some());
  }

  #[test]
  fn github_runtime_provider_uses_explicit_env_mapping() {
    let descriptor = find_auth_provider("github").expect("descriptor");

    assert_eq!(
      descriptor.env_vars(),
      vec!["GITHUB_COPILOT_TOKEN", "GITHUB_TOKEN"]
    );
    assert!(!descriptor.visible_in_connect_catalog);
  }
}
