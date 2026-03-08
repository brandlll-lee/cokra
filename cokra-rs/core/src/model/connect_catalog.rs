use super::oauth_connect::OAuthProviderKind;
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

#[derive(Debug, Clone)]
pub struct ConnectProviderCatalogEntry {
  pub id: &'static str,
  pub name: &'static str,
  pub connect_method: ProviderConnectMethod,
  pub env_vars: Vec<String>,
  pub default_models: Vec<String>,
  pub runtime_registration: RuntimeRegistrationKind,
  pub runtime_provider_id: Option<&'static str>,
  pub oauth_provider: Option<OAuthProviderKind>,
}

impl ConnectProviderCatalogEntry {
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

pub fn connect_provider_catalog() -> Vec<ConnectProviderCatalogEntry> {
  vec![
    ConnectProviderCatalogEntry {
      id: "anthropic-oauth",
      name: "Anthropic (Claude Pro/Max)",
      connect_method: ProviderConnectMethod::OAuth,
      env_vars: Vec::new(),
      default_models: vec!["claude-sonnet-4-5".to_string()],
      runtime_registration: RuntimeRegistrationKind::Anthropic,
      runtime_provider_id: Some("anthropic"),
      oauth_provider: Some(OAuthProviderKind::Anthropic),
    },
    ConnectProviderCatalogEntry {
      id: "github-copilot",
      name: "GitHub Copilot",
      connect_method: ProviderConnectMethod::OAuth,
      env_vars: Vec::new(),
      default_models: vec!["gpt-4o".to_string()],
      runtime_registration: RuntimeRegistrationKind::GitHubCopilot,
      runtime_provider_id: Some("github"),
      oauth_provider: Some(OAuthProviderKind::GitHubCopilot),
    },
    ConnectProviderCatalogEntry {
      id: "google-gemini-cli",
      name: "Google Cloud Code Assist (Gemini CLI)",
      connect_method: ProviderConnectMethod::OAuth,
      env_vars: Vec::new(),
      default_models: vec![
        "gemini-2.5-pro".to_string(),
        "gemini-2.5-flash".to_string(),
        "gemini-3-flash-preview".to_string(),
        "gemini-3-pro-preview".to_string(),
      ],
      runtime_registration: RuntimeRegistrationKind::GoogleGeminiCli,
      runtime_provider_id: Some("google-gemini-cli"),
      oauth_provider: Some(OAuthProviderKind::GoogleGeminiCli),
    },
    ConnectProviderCatalogEntry {
      id: "google-antigravity",
      name: "Antigravity (Gemini 3, Claude, GPT-OSS)",
      connect_method: ProviderConnectMethod::OAuth,
      env_vars: Vec::new(),
      default_models: vec![
        "gemini-3-pro-high".to_string(),
        "gemini-3-flash".to_string(),
        "claude-sonnet-4-5-thinking".to_string(),
        "claude-opus-4-5-thinking".to_string(),
        "gpt-oss-120b-medium".to_string(),
      ],
      runtime_registration: RuntimeRegistrationKind::GoogleAntigravity,
      runtime_provider_id: Some("google-antigravity"),
      oauth_provider: Some(OAuthProviderKind::GoogleAntigravity),
    },
    ConnectProviderCatalogEntry {
      id: "openai-codex",
      name: "ChatGPT Plus/Pro (Codex Subscription)",
      connect_method: ProviderConnectMethod::OAuth,
      env_vars: Vec::new(),
      default_models: vec![
        "gpt-5.3-codex".to_string(),
        "gpt-5.2-codex".to_string(),
        "gpt-5.1-codex".to_string(),
        "gpt-5.1-codex-mini".to_string(),
      ],
      runtime_registration: RuntimeRegistrationKind::OpenAICodex,
      runtime_provider_id: Some("openai"),
      oauth_provider: Some(OAuthProviderKind::OpenAICodex),
    },
    ConnectProviderCatalogEntry {
      id: "openrouter",
      name: "OpenRouter",
      connect_method: ProviderConnectMethod::ApiKey,
      env_vars: vec!["OPENROUTER_API_KEY".to_string()],
      default_models: vec!["anthropic/claude-haiku-4.5".to_string()],
      runtime_registration: RuntimeRegistrationKind::OpenRouter,
      runtime_provider_id: Some("openrouter"),
      oauth_provider: None,
    },
    ConnectProviderCatalogEntry {
      id: "openai",
      name: "OpenAI",
      connect_method: ProviderConnectMethod::ApiKey,
      env_vars: vec!["OPENAI_API_KEY".to_string()],
      default_models: vec!["gpt-5".to_string()],
      runtime_registration: RuntimeRegistrationKind::OpenAI,
      runtime_provider_id: Some("openai"),
      oauth_provider: None,
    },
    ConnectProviderCatalogEntry {
      id: "anthropic",
      name: "Anthropic",
      connect_method: ProviderConnectMethod::ApiKey,
      env_vars: vec!["ANTHROPIC_API_KEY".to_string()],
      default_models: vec!["claude-sonnet-4-5".to_string()],
      runtime_registration: RuntimeRegistrationKind::Anthropic,
      runtime_provider_id: Some("anthropic"),
      oauth_provider: None,
    },
    ConnectProviderCatalogEntry {
      id: "google",
      name: "Google",
      connect_method: ProviderConnectMethod::ApiKey,
      env_vars: vec!["GOOGLE_API_KEY".to_string()],
      default_models: vec!["gemini-2.5-pro".to_string()],
      runtime_registration: RuntimeRegistrationKind::Google,
      runtime_provider_id: Some("google"),
      oauth_provider: None,
    },
  ]
}

pub fn find_connect_provider(provider_id: &str) -> Option<ConnectProviderCatalogEntry> {
  connect_provider_catalog()
    .into_iter()
    .find(|provider| provider.id == provider_id)
}
