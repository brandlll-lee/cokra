use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use std::error::Error as _;

use reqwest::Url;
use serde::Deserialize;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::auth::AuthError;
use super::auth::Credentials;
use super::auth::OAuthConfig;
use super::auth::StoredCredentials;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthProviderKind {
  Anthropic,
  GitHubCopilot,
  GoogleGeminiCli,
  GoogleAntigravity,
  OpenAICodex,
}

#[derive(Debug, Clone)]
pub struct PendingOAuthConnect {
  pub provider_id: String,
  pub kind: OAuthProviderKind,
  pub verifier: Option<String>,
  pub state: Option<String>,
  pub device_code: Option<String>,
  pub user_code: Option<String>,
  pub verification_uri: Option<String>,
  pub interval: Option<u64>,
  pub expires_in: Option<u64>,
  /// GitHub Enterprise domain (e.g., "github.mycompany.com")
  pub enterprise_domain: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OAuthConnectStart {
  pub provider_name: String,
  pub auth_url: String,
  pub instructions: String,
  pub prompt: Option<String>,
  pub pending: PendingOAuthConnect,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
  device_code: String,
  user_code: String,
  verification_uri: String,
  interval: u64,
  expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
  access_token: String,
  #[serde(default)]
  refresh_token: String,
  expires_in: u64,
  #[serde(default)]
  id_token: String,
}

#[derive(Debug, Deserialize)]
struct OpenAITokenExchangeResponse {
  access_token: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct OpenAIIdTokenInfo {
  email: Option<String>,
  plan_type: Option<String>,
  user_id: Option<String>,
  account_id: Option<String>,
  organization_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIIdTokenClaims {
  #[serde(default)]
  email: Option<String>,
  #[serde(default)]
  organization_id: Option<String>,
  #[serde(default)]
  organizations: Vec<OpenAIOrganizationClaim>,
  #[serde(rename = "https://api.openai.com/profile", default)]
  profile: Option<OpenAIProfileClaims>,
  #[serde(rename = "https://api.openai.com/auth", default)]
  auth: Option<OpenAIAuthClaims>,
}

#[derive(Debug, Deserialize)]
struct OpenAIProfileClaims {
  #[serde(default)]
  email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIAuthClaims {
  #[serde(default)]
  chatgpt_plan_type: Option<String>,
  #[serde(default)]
  chatgpt_user_id: Option<String>,
  #[serde(default)]
  user_id: Option<String>,
  #[serde(default)]
  chatgpt_account_id: Option<String>,
  #[serde(default)]
  organization_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIOrganizationClaim {
  #[serde(default)]
  id: String,
}

const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const OPENAI_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const OPENAI_SCOPE: &str = "openid profile email offline_access";
const OPENAI_JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";

const ANTHROPIC_CLIENT_ID_B64: &str = "OWQxYzI1MGEtZTYxYi00NGQ5LTg4ZWQtNTk0NGQxOTYyZjVl";
const ANTHROPIC_AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const ANTHROPIC_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const ANTHROPIC_REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const ANTHROPIC_SCOPE: &str = "org:create_api_key user:profile user:inference";

const GITHUB_COPILOT_CLIENT_ID_B64: &str = "SXYxLmI1MDdhMDhjODdlY2ZlOTg=";

// GitHub Copilot API headers (matching pi-mono)
const COPILOT_HEADERS: [(&str, &str); 4] = [
  ("User-Agent", "GitHubCopilotChat/0.35.0"),
  ("Editor-Version", "vscode/1.107.0"),
  ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
  ("Copilot-Integration-Id", "vscode-chat"),
];

/// Normalize a GitHub Enterprise domain input.
/// Accepts formats like:
/// - "github.com" or "github.com/"
/// - "https://github.mycompany.com"
/// - "github.mycompany.com"
/// Returns the hostname or None if invalid.
pub fn normalize_github_domain(input: &str) -> Option<String> {
  let trimmed = input.trim();
  if trimmed.is_empty() {
    return None;
  }
  // Try parsing as a full URL first
  if trimmed.contains("://") {
    if let Ok(url) = Url::parse(trimmed) {
      return Some(url.host_str()?.to_string());
    }
  }
  // Try parsing as a hostname with https prefix
  if let Ok(url) = Url::parse(&format!("https://{}", trimmed)) {
    if let Some(host) = url.host_str() {
      // Validate it looks like a domain
      if host.contains('.') && !host.contains(' ') {
        return Some(host.to_string());
      }
    }
  }
  // Fallback: check if it looks like a valid domain
  if trimmed.contains('.') && !trimmed.contains(' ') && !trimmed.starts_with('.') {
    // Extract just the hostname if there's a path
    let domain = trimmed.split('/').next().unwrap_or(trimmed);
    return Some(domain.to_string());
  }
  None
}

/// Get GitHub API URLs based on domain (enterprise or github.com)
fn get_github_urls(domain: &str) -> GitHubUrls {
  GitHubUrls {
    device_code_url: format!("https://{}/login/device/code", domain),
    access_token_url: format!("https://{}/login/oauth/access_token", domain),
    copilot_token_url: format!("https://api.{}/copilot_internal/v2/token", domain),
    models_base_url: format!("https://api.{}", domain),
  }
}

struct GitHubUrls {
  device_code_url: String,
  access_token_url: String,
  copilot_token_url: String,
  models_base_url: String,
}

/// Extract base URL from GitHub Copilot token's proxy-ep field.
/// Token format: tid=...;exp=...;proxy-ep=proxy.individual.githubcopilot.com;...
/// Returns API URL like https://api.individual.githubcopilot.com
pub fn get_github_base_url_from_token(token: &str) -> Option<String> {
  // Find proxy-ep in the token
  for part in token.split(';') {
    if let Some((key, value)) = part.split_once('=') {
      if key.trim() == "proxy-ep" {
        let proxy_host = value.trim();
        // Convert proxy.xxx to api.xxx
        let api_host = proxy_host.replacen("proxy.", "api.", 1);
        return Some(format!("https://{}", api_host));
      }
    }
  }
  None
}

/// Get the base URL for GitHub Copilot API, preferring token extraction.
pub fn get_github_copilot_base_url(token: Option<&str>, enterprise_domain: Option<&str>) -> String {
  // If we have a token, try to extract the base URL from proxy-ep
  if let Some(t) = token {
    if let Some(url) = get_github_base_url_from_token(t) {
      return url;
    }
  }
  // Fallback for enterprise or default
  if let Some(domain) = enterprise_domain {
    return format!("https://copilot-api.{}", domain);
  }
  "https://api.individual.githubcopilot.com".to_string()
}

/// Enable a single GitHub Copilot model by setting its policy to "enabled".
async fn enable_github_copilot_model(
  client: &reqwest::Client,
  base_url: &str,
  model_id: &str,
  token: &str,
) -> Result<bool, AuthError> {
  let url = format!("{}/models/{}/policy", base_url, model_id);
  let mut headers = reqwest::header::HeaderMap::new();
  headers.insert(
    reqwest::header::CONTENT_TYPE,
    "application/json".parse().unwrap(),
  );
  headers.insert(
    reqwest::header::AUTHORIZATION,
    format!("Bearer {}", token).parse().unwrap(),
  );
  for (key, value) in COPILOT_HEADERS {
    headers.insert(
      reqwest::header::HeaderName::from_static(key),
      value.parse().unwrap(),
    );
  }
  headers.insert(
    reqwest::header::HeaderName::from_static("openai-intent"),
    "chat-policy".parse().unwrap(),
  );
  headers.insert(
    reqwest::header::HeaderName::from_static("x-interaction-type"),
    "chat-policy".parse().unwrap(),
  );

  let response = client
    .post(&url)
    .headers(headers)
    .json(&serde_json::json!({"state": "enabled"}))
    .send()
    .await
    .map_err(|e| AuthError::OAuthError(format!("failed to enable model {}: {}", model_id, e)))?;

  Ok(response.status().is_success())
}

/// Enable all known GitHub Copilot models after successful login.
/// This is required for some models (like Claude, Grok) before they can be used.
pub async fn enable_all_github_copilot_models(
  token: &str,
  enterprise_domain: Option<&str>,
) -> Vec<(String, bool)> {
  let client = oauth_http_client();
  let base_url = get_github_copilot_base_url(Some(token), enterprise_domain);

  // List of models that may require policy acceptance
  let models = [
    "gpt-4o",
    "gpt-4.1",
    "gpt-4.5-turbo",
    "claude-3.5-sonnet",
    "claude-3.7-sonnet",
    "claude-sonnet-4",
    "claude-opus-4",
    "o1",
    "o1-preview",
    "o1-mini",
    "o3-mini",
    "o4-mini",
    "gemini-2.0-flash",
    "gemini-2.5-pro",
    "grok-2-1212",
  ];

  let mut results = Vec::new();
  for model_id in models {
    match enable_github_copilot_model(&client, &base_url, model_id, token).await {
      Ok(success) => results.push((model_id.to_string(), success)),
      Err(_) => results.push((model_id.to_string(), false)),
    }
  }
  results
}
const ENV_GOOGLE_GEMINI_CLIENT_ID: &str = "COKRA_GOOGLE_GEMINI_CLIENT_ID";
const ENV_GOOGLE_GEMINI_CLIENT_SECRET: &str = "COKRA_GOOGLE_GEMINI_CLIENT_SECRET";
const GOOGLE_GEMINI_REDIRECT_URI: &str = "http://localhost:8085/oauth2callback";
const GOOGLE_GEMINI_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_GEMINI_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_GEMINI_SCOPES: &[&str] = &[
  "https://www.googleapis.com/auth/cloud-platform",
  "https://www.googleapis.com/auth/userinfo.email",
  "https://www.googleapis.com/auth/userinfo.profile",
];

const ENV_GOOGLE_ANTIGRAVITY_CLIENT_ID: &str = "COKRA_GOOGLE_ANTIGRAVITY_CLIENT_ID";
const ENV_GOOGLE_ANTIGRAVITY_CLIENT_SECRET: &str = "COKRA_GOOGLE_ANTIGRAVITY_CLIENT_SECRET";
const GOOGLE_ANTIGRAVITY_REDIRECT_URI: &str = "http://localhost:51121/oauth-callback";
const GOOGLE_ANTIGRAVITY_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_ANTIGRAVITY_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_ANTIGRAVITY_SCOPES: &[&str] = &[
  "https://www.googleapis.com/auth/cloud-platform",
  "https://www.googleapis.com/auth/userinfo.email",
  "https://www.googleapis.com/auth/userinfo.profile",
  "https://www.googleapis.com/auth/cclog",
  "https://www.googleapis.com/auth/experimentsandconfigs",
];

fn env_required(name: &'static str) -> Result<String, AuthError> {
  std::env::var(name).map_err(|_| {
    AuthError::OAuthError(format!(
      "missing OAuth client configuration: set {name} in your environment"
    ))
  })
}

fn env_optional(name: &'static str) -> Option<String> {
  std::env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn google_oauth_client(kind: OAuthProviderKind) -> Result<(String, Option<String>), AuthError> {
  match kind {
    OAuthProviderKind::GoogleGeminiCli => Ok((
      env_required(ENV_GOOGLE_GEMINI_CLIENT_ID)?,
      env_optional(ENV_GOOGLE_GEMINI_CLIENT_SECRET),
    )),
    OAuthProviderKind::GoogleAntigravity => Ok((
      env_required(ENV_GOOGLE_ANTIGRAVITY_CLIENT_ID)?,
      env_optional(ENV_GOOGLE_ANTIGRAVITY_CLIENT_SECRET),
    )),
    _ => Err(AuthError::OAuthError(
      "provider is not a Google OAuth flow".to_string(),
    )),
  }
}

const CALLBACK_WAIT_TIMEOUT: Duration = Duration::from_secs(600);
const CALLBACK_READ_TIMEOUT: Duration = Duration::from_secs(10);
const CALLBACK_SUCCESS_HTML: &str = "<!doctype html><html><body><h1>Authentication Successful</h1><p>You can close this window and return to the terminal.</p></body></html>";
const CALLBACK_FAILURE_HTML: &str = "<!doctype html><html><body><h1>Authentication Failed</h1><p>You can close this window and return to the terminal.</p></body></html>";

#[derive(Debug, Clone, Copy)]
struct CallbackBinding {
  port: u16,
  path: &'static str,
}

fn oauth_http_client() -> reqwest::Client {
  // Align with opencode plugin auth flows: use the default fetch-like client
  // and avoid imposing custom connect/total timeouts that can break token
  // exchange on slower networks or under WSL proxy/DNS setups.
  reqwest::Client::new()
}

fn reqwest_error_message(prefix: &str, err: &reqwest::Error) -> String {
  let mut msg = format!("{prefix}: {err}");
  let mut source = err.source();
  while let Some(cause) = source {
    msg.push_str(&format!(" | caused by: {cause}"));
    source = cause.source();
  }
  msg
}

/// Start GitHub Copilot OAuth with optional enterprise domain.
/// For GitHub Enterprise, users should call this function with their enterprise domain.
pub async fn start_github_copilot_connect_with_domain(
  provider_id: &str,
  provider_name: &str,
  enterprise_domain: Option<&str>,
) -> Result<OAuthConnectStart, AuthError> {
  start_github_copilot_connect(provider_id, provider_name, enterprise_domain).await
}

pub async fn start_oauth_connect(
  provider_id: &str,
  provider_name: &str,
  kind: OAuthProviderKind,
) -> Result<OAuthConnectStart, AuthError> {
  match kind {
    OAuthProviderKind::GitHubCopilot => {
      start_github_copilot_connect(provider_id, provider_name, None).await
    }
    OAuthProviderKind::Anthropic => start_anthropic_connect(provider_id, provider_name),
    OAuthProviderKind::OpenAICodex => start_openai_codex_connect(provider_id, provider_name),
    OAuthProviderKind::GoogleGeminiCli => {
      let (client_id, _secret) = google_oauth_client(OAuthProviderKind::GoogleGeminiCli)?;
      start_google_connect(
        provider_id,
        provider_name,
        kind,
        client_id,
        GOOGLE_GEMINI_AUTH_URL,
        GOOGLE_GEMINI_REDIRECT_URI,
        GOOGLE_GEMINI_SCOPES,
      )
    }
    OAuthProviderKind::GoogleAntigravity => {
      let (client_id, _secret) = google_oauth_client(OAuthProviderKind::GoogleAntigravity)?;
      start_google_connect(
        provider_id,
        provider_name,
        kind,
        client_id,
        GOOGLE_ANTIGRAVITY_AUTH_URL,
        GOOGLE_ANTIGRAVITY_REDIRECT_URI,
        GOOGLE_ANTIGRAVITY_SCOPES,
      )
    }
  }
}

pub async fn complete_oauth_connect(
  pending: &PendingOAuthConnect,
  input: Option<&str>,
) -> Result<StoredCredentials, AuthError> {
  match pending.kind {
    OAuthProviderKind::GitHubCopilot => complete_github_copilot_connect(pending).await,
    OAuthProviderKind::Anthropic => {
      complete_anthropic_connect(pending, input.unwrap_or_default()).await
    }
    OAuthProviderKind::OpenAICodex => {
      complete_openai_codex_connect(pending, input.unwrap_or_default()).await
    }
    OAuthProviderKind::GoogleGeminiCli => {
      let (client_id, client_secret) = google_oauth_client(OAuthProviderKind::GoogleGeminiCli)?;
      complete_google_connect(
        pending,
        input.unwrap_or_default(),
        client_id,
        client_secret,
        GOOGLE_GEMINI_TOKEN_URL,
        GOOGLE_GEMINI_REDIRECT_URI,
        false,
      )
      .await
    }
    OAuthProviderKind::GoogleAntigravity => {
      let (client_id, client_secret) = google_oauth_client(OAuthProviderKind::GoogleAntigravity)?;
      complete_google_connect(
        pending,
        input.unwrap_or_default(),
        client_id,
        client_secret,
        GOOGLE_ANTIGRAVITY_TOKEN_URL,
        GOOGLE_ANTIGRAVITY_REDIRECT_URI,
        true,
      )
      .await
    }
  }
}

pub fn uses_local_callback(kind: OAuthProviderKind) -> bool {
  callback_binding(kind).is_some()
}

pub fn oauth_refresh_config_for_provider(
  provider_id: &str,
) -> Result<Option<OAuthConfig>, AuthError> {
  let config = match provider_id {
    "anthropic-oauth" => Some(OAuthConfig {
      provider_id: provider_id.to_string(),
      client_id: decode_b64(ANTHROPIC_CLIENT_ID_B64)?,
      client_secret: None,
      auth_url: ANTHROPIC_AUTHORIZE_URL.to_string(),
      token_url: ANTHROPIC_TOKEN_URL.to_string(),
      scopes: ANTHROPIC_SCOPE
        .split_whitespace()
        .map(ToString::to_string)
        .collect(),
      redirect_uri: ANTHROPIC_REDIRECT_URI.to_string(),
    }),
    "openai-codex" => Some(OAuthConfig {
      provider_id: provider_id.to_string(),
      client_id: OPENAI_CLIENT_ID.to_string(),
      client_secret: None,
      auth_url: OPENAI_AUTHORIZE_URL.to_string(),
      token_url: OPENAI_TOKEN_URL.to_string(),
      scopes: OPENAI_SCOPE
        .split_whitespace()
        .map(ToString::to_string)
        .collect(),
      redirect_uri: OPENAI_REDIRECT_URI.to_string(),
    }),
    "google-gemini-cli" => {
      let client_id = env_optional(ENV_GOOGLE_GEMINI_CLIENT_ID);
      client_id.map(|client_id| OAuthConfig {
        provider_id: provider_id.to_string(),
        client_id,
        client_secret: env_optional(ENV_GOOGLE_GEMINI_CLIENT_SECRET),
        auth_url: GOOGLE_GEMINI_AUTH_URL.to_string(),
        token_url: GOOGLE_GEMINI_TOKEN_URL.to_string(),
        scopes: GOOGLE_GEMINI_SCOPES
          .iter()
          .map(|scope| (*scope).to_string())
          .collect(),
        redirect_uri: GOOGLE_GEMINI_REDIRECT_URI.to_string(),
      })
    }
    "google-antigravity" => {
      let client_id = env_optional(ENV_GOOGLE_ANTIGRAVITY_CLIENT_ID);
      client_id.map(|client_id| OAuthConfig {
        provider_id: provider_id.to_string(),
        client_id,
        client_secret: env_optional(ENV_GOOGLE_ANTIGRAVITY_CLIENT_SECRET),
        auth_url: GOOGLE_ANTIGRAVITY_AUTH_URL.to_string(),
        token_url: GOOGLE_ANTIGRAVITY_TOKEN_URL.to_string(),
        scopes: GOOGLE_ANTIGRAVITY_SCOPES
          .iter()
          .map(|scope| (*scope).to_string())
          .collect(),
        redirect_uri: GOOGLE_ANTIGRAVITY_REDIRECT_URI.to_string(),
      })
    }
    _ => None,
  };

  Ok(config)
}

pub async fn wait_for_local_callback(
  pending: &PendingOAuthConnect,
  cancel: CancellationToken,
) -> Result<String, AuthError> {
  let Some(binding) = callback_binding(pending.kind) else {
    return Err(AuthError::OAuthError(
      "provider does not use localhost callback".to_string(),
    ));
  };

  let listener = TcpListener::bind(("127.0.0.1", binding.port))
    .await
    .map_err(|e| AuthError::OAuthError(format!("failed to bind localhost callback server: {e}")))?;

  let accept_future = async {
    loop {
      let (mut socket, _) = listener
        .accept()
        .await
        .map_err(|e| AuthError::OAuthError(format!("failed to accept localhost callback: {e}")))?;

      let request = read_http_request(&mut socket).await?;
      let Some((path, query)) = parse_http_request_target(&request) else {
        write_http_response(&mut socket, 400, CALLBACK_FAILURE_HTML).await?;
        continue;
      };

      if path != binding.path {
        write_http_response(&mut socket, 404, CALLBACK_FAILURE_HTML).await?;
        continue;
      }

      let callback_url = if query.is_empty() {
        format!("http://localhost:{}{}", binding.port, path)
      } else {
        format!("http://localhost:{}{}?{}", binding.port, path, query)
      };

      let parsed = parse_auth_input(&callback_url);
      if parsed.code.is_none() {
        write_http_response(&mut socket, 400, CALLBACK_FAILURE_HTML).await?;
        continue;
      }

      write_http_response(&mut socket, 200, CALLBACK_SUCCESS_HTML).await?;
      return Ok(callback_url);
    }
  };

  tokio::select! {
    _ = cancel.cancelled() => Err(AuthError::OAuthError("OAuth callback cancelled".to_string())),
    result = timeout(CALLBACK_WAIT_TIMEOUT, accept_future) => {
      result
        .map_err(|_| AuthError::Timeout)?
    }
  }
}

fn start_anthropic_connect(
  provider_id: &str,
  provider_name: &str,
) -> Result<OAuthConnectStart, AuthError> {
  let verifier = generate_verifier();
  let challenge = pkce_challenge(&verifier);
  let mut url = Url::parse(ANTHROPIC_AUTHORIZE_URL)
    .map_err(|e| AuthError::OAuthError(format!("invalid anthropic authorize url: {e}")))?;
  url
    .query_pairs_mut()
    .append_pair("code", "true")
    .append_pair("client_id", &decode_b64(ANTHROPIC_CLIENT_ID_B64)?)
    .append_pair("response_type", "code")
    .append_pair("redirect_uri", ANTHROPIC_REDIRECT_URI)
    .append_pair("scope", ANTHROPIC_SCOPE)
    .append_pair("code_challenge", &challenge)
    .append_pair("code_challenge_method", "S256")
    .append_pair("state", &verifier);

  Ok(OAuthConnectStart {
    provider_name: provider_name.to_string(),
    auth_url: url.to_string(),
    instructions: "Complete login in your browser, then paste the returned authorization code (or full redirect URL).".to_string(),
    prompt: Some("Paste the authorization code or redirect URL:".to_string()),
    pending: PendingOAuthConnect {
      provider_id: provider_id.to_string(),
      kind: OAuthProviderKind::Anthropic,
      verifier: Some(verifier.clone()),
      state: Some(verifier),
      device_code: None,
      user_code: None,
      verification_uri: None,
      interval: None,
      expires_in: None,
      enterprise_domain: None,
    },
  })
}

fn start_openai_codex_connect(
  provider_id: &str,
  provider_name: &str,
) -> Result<OAuthConnectStart, AuthError> {
  let verifier = generate_verifier();
  let challenge = pkce_challenge(&verifier);
  let state = uuid::Uuid::new_v4().simple().to_string();
  let mut url = Url::parse(OPENAI_AUTHORIZE_URL)
    .map_err(|e| AuthError::OAuthError(format!("invalid openai authorize url: {e}")))?;
  url
    .query_pairs_mut()
    .append_pair("response_type", "code")
    .append_pair("client_id", OPENAI_CLIENT_ID)
    .append_pair("redirect_uri", OPENAI_REDIRECT_URI)
    .append_pair("scope", OPENAI_SCOPE)
    .append_pair("code_challenge", &challenge)
    .append_pair("code_challenge_method", "S256")
    .append_pair("state", &state)
    .append_pair("id_token_add_organizations", "true")
    .append_pair("codex_cli_simplified_flow", "true")
    .append_pair("originator", "cokra");

  Ok(OAuthConnectStart {
    provider_name: provider_name.to_string(),
    auth_url: url.to_string(),
    instructions: "Complete login in your browser. Cokra will try to capture the localhost callback automatically; if that does not work, paste the authorization code or full redirect URL here.".to_string(),
    prompt: Some("Paste the authorization code or redirect URL:".to_string()),
    pending: PendingOAuthConnect {
      provider_id: provider_id.to_string(),
      kind: OAuthProviderKind::OpenAICodex,
      verifier: Some(verifier),
      state: Some(state),
      device_code: None,
      user_code: None,
      verification_uri: None,
      interval: None,
      expires_in: None,
      enterprise_domain: None,
    },
  })
}

fn start_google_connect(
  provider_id: &str,
  provider_name: &str,
  kind: OAuthProviderKind,
  client_id: String,
  auth_url: &str,
  redirect_uri: &str,
  scopes: &[&str],
) -> Result<OAuthConnectStart, AuthError> {
  let verifier = generate_verifier();
  let challenge = pkce_challenge(&verifier);
  let mut url = Url::parse(auth_url)
    .map_err(|e| AuthError::OAuthError(format!("invalid google authorize url: {e}")))?;
  url
    .query_pairs_mut()
    .append_pair("client_id", &client_id)
    .append_pair("response_type", "code")
    .append_pair("redirect_uri", redirect_uri)
    .append_pair("scope", &scopes.join(" "))
    .append_pair("code_challenge", &challenge)
    .append_pair("code_challenge_method", "S256")
    .append_pair("state", &verifier)
    .append_pair("access_type", "offline")
    .append_pair("prompt", "consent");

  Ok(OAuthConnectStart {
    provider_name: provider_name.to_string(),
    auth_url: url.to_string(),
    instructions: "Complete sign-in in your browser. Cokra will try to capture the localhost callback automatically; if that does not work, paste the full redirect URL from the browser address bar.".to_string(),
    prompt: Some("Paste the redirect URL:".to_string()),
    pending: PendingOAuthConnect {
      provider_id: provider_id.to_string(),
      kind,
      verifier: Some(verifier.clone()),
      state: Some(verifier),
      device_code: None,
      user_code: None,
      verification_uri: None,
      interval: None,
      expires_in: None,
      enterprise_domain: None,
    },
  })
}

async fn start_github_copilot_connect(
  provider_id: &str,
  provider_name: &str,
  enterprise_domain: Option<&str>,
) -> Result<OAuthConnectStart, AuthError> {
  // Use provided domain or default to github.com
  let domain = enterprise_domain.unwrap_or("github.com");
  let urls = get_github_urls(domain);

  let client = oauth_http_client();
  let response = client
    .post(&urls.device_code_url)
    .header("Accept", "application/json")
    .header("Content-Type", "application/json")
    .header("User-Agent", "GitHubCopilotChat/0.35.0")
    .json(&serde_json::json!({
      "client_id": decode_b64(GITHUB_COPILOT_CLIENT_ID_B64)?,
      "scope": "read:user",
    }))
    .send()
    .await
    .map_err(|e| {
      AuthError::OAuthError(reqwest_error_message(
        "failed to start GitHub Copilot device flow",
        &e,
      ))
    })?;

  if !response.status().is_success() {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    return Err(AuthError::OAuthError(format!(
      "GitHub Copilot device flow failed (HTTP {status}): {body}"
    )));
  }

  let device = response.json::<DeviceCodeResponse>().await.map_err(|e| {
    AuthError::OAuthError(format!("failed to parse GitHub device flow response: {e}"))
  })?;

  Ok(OAuthConnectStart {
    provider_name: provider_name.to_string(),
    auth_url: device.verification_uri.clone(),
    instructions: format!(
      "Open the URL in your browser and enter code {}. Cokra will keep polling until login completes.",
      device.user_code
    ),
    prompt: None,
    pending: PendingOAuthConnect {
      provider_id: provider_id.to_string(),
      kind: OAuthProviderKind::GitHubCopilot,
      verifier: None,
      state: None,
      device_code: Some(device.device_code),
      user_code: Some(device.user_code),
      verification_uri: Some(device.verification_uri),
      interval: Some(device.interval),
      expires_in: Some(device.expires_in),
      enterprise_domain: enterprise_domain.map(ToString::to_string),
    },
  })
}

async fn complete_anthropic_connect(
  pending: &PendingOAuthConnect,
  input: &str,
) -> Result<StoredCredentials, AuthError> {
  let parsed = parse_auth_input(input);
  let code = parsed
    .code
    .ok_or_else(|| AuthError::OAuthError("missing authorization code".to_string()))?;
  let expected_state = pending.state.as_deref().unwrap_or_default();
  if let Some(state) = parsed.state
    && state != expected_state
  {
    return Err(AuthError::OAuthError(
      "Anthropic OAuth state mismatch".to_string(),
    ));
  }

  let client = oauth_http_client();
  let response = client
    .post(ANTHROPIC_TOKEN_URL)
    .header("Content-Type", "application/json")
    .json(&serde_json::json!({
      "grant_type": "authorization_code",
      "client_id": decode_b64(ANTHROPIC_CLIENT_ID_B64)?,
      "code": code,
      "state": expected_state,
      "redirect_uri": ANTHROPIC_REDIRECT_URI,
      "code_verifier": pending.verifier.clone().unwrap_or_default(),
    }))
    .send()
    .await
    .map_err(|e| {
      AuthError::OAuthError(reqwest_error_message("Anthropic token exchange failed", &e))
    })?;

  if !response.status().is_success() {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    return Err(AuthError::OAuthError(format!(
      "Anthropic token exchange failed (HTTP {status}): {body}"
    )));
  }

  let token = response
    .json::<TokenResponse>()
    .await
    .map_err(|e| AuthError::OAuthError(format!("failed to parse Anthropic token response: {e}")))?;

  Ok(oauth_stored_credentials(
    &pending.provider_id,
    token,
    None,
    None,
    Value::Null,
  ))
}

async fn complete_openai_codex_connect(
  pending: &PendingOAuthConnect,
  input: &str,
) -> Result<StoredCredentials, AuthError> {
  let parsed = parse_auth_input(input);
  let code = parsed
    .code
    .ok_or_else(|| AuthError::OAuthError("missing authorization code".to_string()))?;
  if let Some(state) = parsed.state
    && Some(state) != pending.state
  {
    return Err(AuthError::OAuthError(
      "OpenAI OAuth state mismatch".to_string(),
    ));
  }

  let client = oauth_http_client();
  let response = client
    .post(OPENAI_TOKEN_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .form(&[
      ("grant_type", "authorization_code"),
      ("client_id", OPENAI_CLIENT_ID),
      ("code", &code),
      (
        "code_verifier",
        pending.verifier.as_deref().unwrap_or_default(),
      ),
      ("redirect_uri", OPENAI_REDIRECT_URI),
    ])
    .send()
    .await
    .map_err(|e| {
      AuthError::OAuthError(reqwest_error_message("OpenAI token exchange failed", &e))
    })?;

  if !response.status().is_success() {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    return Err(AuthError::OAuthError(format!(
      "OpenAI token exchange failed (HTTP {status}): {body}"
    )));
  }

  let token = response
    .json::<TokenResponse>()
    .await
    .map_err(|e| AuthError::OAuthError(format!("failed to parse OpenAI token response: {e}")))?;

  if token.id_token.is_empty() {
    return Err(AuthError::OAuthError(
      "OpenAI token response did not include an id_token".to_string(),
    ));
  }

  let id_info = parse_openai_id_token_info(&token.id_token)?;
  let account_id = id_info
    .account_id
    .clone()
    .or_else(|| extract_openai_account_id(&token.access_token));
  let organization_id = id_info
    .organization_id
    .clone()
    .or_else(|| account_id.clone());
  let account_name = id_info.email.clone();
  let api_key = exchange_openai_api_key(&client, &token.id_token, organization_id.as_deref()).await;
  let mut metadata = serde_json::json!({
    "email": id_info.email,
    "plan_type": id_info.plan_type,
    "chatgpt_user_id": id_info.user_id,
    "chatgpt_account_id": account_id.clone(),
    "organization_id": organization_id,
    "id_token": token.id_token,
    "oauth_mode": "chatgpt_access_token",
  });
  match api_key {
    Ok(api_key) => {
      metadata["api_key"] = Value::String(api_key);
    }
    Err(err) => {
      metadata["api_key_exchange_error"] = Value::String(err.to_string());
    }
  }
  let mut stored =
    oauth_stored_credentials(&pending.provider_id, token, account_id, None, metadata);
  stored.account_name = account_name;
  Ok(stored)
}

async fn complete_google_connect(
  pending: &PendingOAuthConnect,
  input: &str,
  client_id: String,
  client_secret: Option<String>,
  token_url: &str,
  redirect_uri: &str,
  antigravity: bool,
) -> Result<StoredCredentials, AuthError> {
  let parsed = parse_auth_input(input);
  let code = parsed
    .code
    .ok_or_else(|| AuthError::OAuthError("missing authorization code".to_string()))?;
  if let Some(state) = parsed.state
    && Some(state) != pending.state
  {
    return Err(AuthError::OAuthError(
      "Google OAuth state mismatch".to_string(),
    ));
  }

  let client = oauth_http_client();
  let mut form = vec![
    ("client_id", client_id.as_str()),
    ("code", code.as_str()),
    ("grant_type", "authorization_code"),
    ("redirect_uri", redirect_uri),
    (
      "code_verifier",
      pending.verifier.as_deref().unwrap_or_default(),
    ),
  ];
  if let Some(secret) = client_secret.as_deref() {
    form.push(("client_secret", secret));
  }

  let response = client
    .post(token_url)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .form(&form)
    .send()
    .await
    .map_err(|e| {
      AuthError::OAuthError(reqwest_error_message("Google token exchange failed", &e))
    })?;

  if !response.status().is_success() {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    return Err(AuthError::OAuthError(format!(
      "Google token exchange failed (HTTP {status}): {body}"
    )));
  }

  let token = response
    .json::<TokenResponse>()
    .await
    .map_err(|e| AuthError::OAuthError(format!("failed to parse Google token response: {e}")))?;

  let email = get_google_user_email(&client, &token.access_token).await;
  let project_id = if antigravity {
    discover_antigravity_project(&client, &token.access_token).await
  } else {
    discover_google_project(&client, &token.access_token).await
  };

  Ok(oauth_stored_credentials(
    &pending.provider_id,
    token,
    None,
    None,
    serde_json::json!({
      "email": email,
      "project_id": project_id,
    }),
  ))
}

async fn complete_github_copilot_connect(
  pending: &PendingOAuthConnect,
) -> Result<StoredCredentials, AuthError> {
  let device_code = pending
    .device_code
    .as_deref()
    .ok_or_else(|| AuthError::OAuthError("missing GitHub device code".to_string()))?;
  let interval = pending.interval.unwrap_or(5).max(1);
  let expires_in = pending.expires_in.unwrap_or(900);
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(expires_in);
  let client_id = decode_b64(GITHUB_COPILOT_CLIENT_ID_B64)?;
  let client = oauth_http_client();
  let mut wait_interval = interval;

  // Use enterprise domain if provided, otherwise default to github.com
  let domain = pending.enterprise_domain.as_deref().unwrap_or("github.com");
  let urls = get_github_urls(domain);

  let github_access_token = loop {
    if std::time::Instant::now() >= deadline {
      return Err(AuthError::Timeout);
    }

    let response = client
      .post(&urls.access_token_url)
      .header("Accept", "application/json")
      .header("Content-Type", "application/json")
      .header("User-Agent", "GitHubCopilotChat/0.35.0")
      .json(&serde_json::json!({
        "client_id": client_id,
        "device_code": device_code,
        "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
      }))
      .send()
      .await
      .map_err(|e| {
        AuthError::OAuthError(reqwest_error_message(
          "GitHub device token polling failed",
          &e,
        ))
      })?;

    let body = response.text().await.unwrap_or_default();
    if let Ok(json) = serde_json::from_str::<Value>(&body) {
      if let Some(access_token) = json.get("access_token").and_then(Value::as_str) {
        break access_token.to_string();
      }
      match json.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => {}
        Some("slow_down") => {
          wait_interval += 5;
        }
        Some(error) => {
          return Err(AuthError::OAuthError(format!(
            "GitHub device flow failed: {error}"
          )));
        }
        None => {}
      }
    }

    tokio::time::sleep(std::time::Duration::from_secs(wait_interval)).await;
  };

  // Enable all GitHub Copilot models after successful login
  let enterprise_domain_ref = pending.enterprise_domain.as_deref();
  let enablement_results =
    enable_all_github_copilot_models(&github_access_token, enterprise_domain_ref).await;
  let enabled_count = enablement_results
    .iter()
    .filter(|(_, success)| *success)
    .count();

  // Extract base URL from token for future API calls
  let base_url = get_github_copilot_base_url(Some(&github_access_token), enterprise_domain_ref);

  Ok(
    StoredCredentials::new(
      pending.provider_id.clone(),
      Credentials::OAuth {
        access_token: github_access_token.clone(),
        refresh_token: github_access_token,
        expires_at: u64::MAX.saturating_sub(1),
        account_id: None,
        enterprise_url: pending.enterprise_domain.clone(),
      },
    )
    .with_metadata(serde_json::json!({
      "oauth_mode": "github_device_token",
      "base_url": base_url,
      "models_enabled": enabled_count,
      "enterprise_domain": pending.enterprise_domain,
    })),
  )
}

fn oauth_stored_credentials(
  provider_id: &str,
  token: TokenResponse,
  account_id: Option<String>,
  enterprise_url: Option<String>,
  metadata: Value,
) -> StoredCredentials {
  let expires_at = chrono::Utc::now().timestamp() as u64 + token.expires_in.saturating_sub(300);
  let mut stored = StoredCredentials::new(
    provider_id.to_string(),
    Credentials::OAuth {
      access_token: token.access_token,
      refresh_token: token.refresh_token,
      expires_at,
      account_id,
      enterprise_url,
    },
  );
  if !metadata.is_null() {
    stored.metadata = metadata;
  }
  stored
}

fn parse_auth_input(input: &str) -> ParsedAuthInput {
  let value = input.trim();
  if value.is_empty() {
    return ParsedAuthInput::default();
  }

  if let Ok(url) = Url::parse(value) {
    return ParsedAuthInput {
      code: url
        .query_pairs()
        .find_map(|(key, value)| (key == "code").then(|| value.to_string())),
      state: url
        .query_pairs()
        .find_map(|(key, value)| (key == "state").then(|| value.to_string())),
    };
  }

  if let Some((code, state)) = value.split_once('#') {
    return ParsedAuthInput {
      code: Some(code.to_string()),
      state: Some(state.to_string()),
    };
  }

  if value.contains("code=") {
    let mut code = None;
    let mut state = None;
    for part in value.split('&') {
      let mut pieces = part.splitn(2, '=');
      let Some(key) = pieces.next() else {
        continue;
      };
      let Some(raw_value) = pieces.next() else {
        continue;
      };
      if key == "code" {
        code = Some(raw_value.to_string());
      }
      if key == "state" {
        state = Some(raw_value.to_string());
      }
    }
    return ParsedAuthInput { code, state };
  }

  ParsedAuthInput {
    code: Some(value.to_string()),
    state: None,
  }
}

#[derive(Default)]
struct ParsedAuthInput {
  code: Option<String>,
  state: Option<String>,
}

fn generate_verifier() -> String {
  format!(
    "{}{}",
    uuid::Uuid::new_v4().simple(),
    uuid::Uuid::new_v4().simple()
  )
}

fn pkce_challenge(verifier: &str) -> String {
  let digest = Sha256::digest(verifier.as_bytes());
  URL_SAFE_NO_PAD.encode(digest)
}

fn decode_b64(input: &str) -> Result<String, AuthError> {
  let bytes = STANDARD
    .decode(input)
    .map_err(|e| AuthError::OAuthError(format!("failed to decode oauth constant: {e}")))?;
  String::from_utf8(bytes)
    .map_err(|e| AuthError::OAuthError(format!("oauth constant is not utf8: {e}")))
}

fn extract_openai_account_id(access_token: &str) -> Option<String> {
  let part = access_token.split('.').nth(1)?;
  let bytes = URL_SAFE_NO_PAD.decode(part.as_bytes()).ok()?;
  let payload = serde_json::from_slice::<Value>(&bytes).ok()?;
  extract_openai_claim(&payload, "chatgpt_account_id")
    .or_else(|| extract_openai_claim(&payload, "organization_id"))
    .or_else(|| first_openai_organization_id(&payload))
}

fn parse_openai_id_token_info(id_token: &str) -> Result<OpenAIIdTokenInfo, AuthError> {
  let payload = id_token
    .split('.')
    .nth(1)
    .ok_or_else(|| AuthError::OAuthError("invalid OpenAI id_token format".to_string()))?;
  let bytes = URL_SAFE_NO_PAD
    .decode(payload.as_bytes())
    .map_err(|e| AuthError::OAuthError(format!("failed to decode OpenAI id_token: {e}")))?;
  let claims = serde_json::from_slice::<OpenAIIdTokenClaims>(&bytes)
    .map_err(|e| AuthError::OAuthError(format!("failed to parse OpenAI id_token: {e}")))?;
  let email = claims
    .email
    .or_else(|| claims.profile.and_then(|profile| profile.email));
  let plan_type = claims
    .auth
    .as_ref()
    .and_then(|auth| auth.chatgpt_plan_type.clone());
  let user_id = claims.auth.as_ref().and_then(|auth| {
    auth
      .chatgpt_user_id
      .clone()
      .or_else(|| auth.user_id.clone())
  });
  let account_id = claims
    .auth
    .as_ref()
    .and_then(|auth| auth.chatgpt_account_id.clone())
    .or_else(|| {
      claims
        .organizations
        .first()
        .map(|org| org.id.clone())
        .filter(|id| !id.is_empty())
    });
  let organization_id = claims
    .organization_id
    .or_else(|| {
      claims
        .auth
        .as_ref()
        .and_then(|auth| auth.organization_id.clone())
    })
    .or_else(|| {
      claims
        .organizations
        .first()
        .map(|org| org.id.clone())
        .filter(|id| !id.is_empty())
    });

  Ok(OpenAIIdTokenInfo {
    email,
    plan_type,
    user_id,
    account_id,
    organization_id,
  })
}

async fn exchange_openai_api_key(
  client: &reqwest::Client,
  id_token: &str,
  organization_id: Option<&str>,
) -> Result<String, AuthError> {
  let mut form = vec![
    (
      "grant_type",
      "urn:ietf:params:oauth:grant-type:token-exchange",
    ),
    ("client_id", OPENAI_CLIENT_ID),
    ("requested_token", "openai-api-key"),
    ("subject_token", id_token),
    (
      "subject_token_type",
      "urn:ietf:params:oauth:token-type:id_token",
    ),
  ];
  if let Some(org_id) = organization_id.filter(|value| !value.is_empty()) {
    form.push(("organization_id", org_id));
  }

  let response = client
    .post(OPENAI_TOKEN_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .form(&form)
    .send()
    .await
    .map_err(|e| {
      AuthError::OAuthError(reqwest_error_message("OpenAI API key exchange failed", &e))
    })?;

  if !response.status().is_success() {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    return Err(AuthError::OAuthError(format!(
      "OpenAI API key exchange failed (HTTP {status}): {body}"
    )));
  }

  response
    .json::<OpenAITokenExchangeResponse>()
    .await
    .map(|token| token.access_token)
    .map_err(|e| {
      AuthError::OAuthError(format!(
        "failed to parse OpenAI API key exchange response: {e}"
      ))
    })
}

fn extract_openai_claim(payload: &Value, key: &str) -> Option<String> {
  payload
    .get(key)
    .and_then(Value::as_str)
    .map(ToString::to_string)
    .or_else(|| {
      payload
        .get(OPENAI_JWT_CLAIM_PATH)
        .and_then(|auth| auth.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
    })
}

fn first_openai_organization_id(payload: &Value) -> Option<String> {
  payload
    .get("organizations")
    .and_then(Value::as_array)
    .and_then(|orgs| orgs.first())
    .and_then(|org| org.get("id"))
    .and_then(Value::as_str)
    .map(ToString::to_string)
}

async fn get_google_user_email(client: &reqwest::Client, access_token: &str) -> Option<String> {
  let response = client
    .get("https://www.googleapis.com/oauth2/v1/userinfo?alt=json")
    .header("Authorization", format!("Bearer {access_token}"))
    .send()
    .await
    .ok()?;
  if !response.status().is_success() {
    return None;
  }
  let payload = response.json::<Value>().await.ok()?;
  payload
    .get("email")
    .and_then(Value::as_str)
    .map(ToString::to_string)
}

/// Tier IDs for Google Cloud Code Assist
const TIER_FREE: &str = "free-tier";
const TIER_LEGACY: &str = "legacy-tier";
const TIER_STANDARD: &str = "standard-tier";

/// Long-running operation response from onboardUser
#[derive(Debug, Deserialize)]
struct LongRunningOperationResponse {
  #[serde(default)]
  done: bool,
  #[serde(default)]
  name: Option<String>,
  #[serde(default)]
  response: Option<CodeAssistOnboardResponse>,
}

#[derive(Debug, Deserialize)]
struct CodeAssistOnboardResponse {
  #[serde(default)]
  cloudaicompanion_project: Option<CloudAiCompanionProject>,
}

#[derive(Debug, Deserialize)]
struct CloudAiCompanionProject {
  #[serde(default)]
  id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoadCodeAssistResponse {
  #[serde(default)]
  cloudaicompanion_project: Option<CloudAiCompanionProjectValue>,
  #[serde(default)]
  current_tier: Option<TierInfo>,
  #[serde(default)]
  allowed_tiers: Vec<TierInfo>,
}

#[derive(Debug, Deserialize)]
struct CloudAiCompanionProjectValue {
  #[serde(default)]
  id: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct TierInfo {
  #[serde(default)]
  id: Option<String>,
  #[serde(default)]
  is_default: Option<bool>,
}

/// Check if the error indicates VPC-SC (security policy) restrictions
fn is_vpc_sc_affected_user(payload: &Value) -> bool {
  if let Some(error) = payload.get("error") {
    if let Some(details) = error.get("details") {
      if let Some(details_arr) = details.as_array() {
        return details_arr.iter().any(|detail| {
          detail
            .get("reason")
            .and_then(Value::as_str)
            .map(|r| r == "SECURITY_POLICY_VIOLATED")
            .unwrap_or(false)
        });
      }
    }
  }
  false
}

/// Get the default tier from allowed tiers list
fn get_default_tier(allowed_tiers: &[TierInfo]) -> Option<&TierInfo> {
  if allowed_tiers.is_empty() {
    return None;
  }
  // Find the default tier
  allowed_tiers
    .iter()
    .find(|t| t.is_default.unwrap_or(false))
    .or_else(|| allowed_tiers.first())
}

/// Poll a long-running operation until completion
async fn poll_google_operation(
  client: &reqwest::Client,
  operation_name: &str,
  headers: &reqwest::header::HeaderMap,
) -> Result<Option<String>, AuthError> {
  let url = format!(
    "https://cloudcode-pa.googleapis.com/v1internal/{}",
    operation_name
  );

  for _attempt in 0..60 {
    // Max 5 minutes (60 * 5 seconds)
    let response = client
      .get(&url)
      .headers(headers.clone())
      .send()
      .await
      .map_err(|e| AuthError::OAuthError(format!("failed to poll Google operation: {e}")))?;

    if !response.status().is_success() {
      continue;
    }

    let payload = response.json::<LongRunningOperationResponse>().await.ok();

    if let Some(data) = payload {
      if data.done {
        if let Some(response) = data.response {
          if let Some(project) = response.cloudaicompanion_project {
            if let Some(id) = project.id {
              return Ok(Some(id));
            }
          }
        }
        return Ok(None);
      }
    }

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
  }

  Ok(None)
}

async fn discover_google_project(client: &reqwest::Client, access_token: &str) -> String {
  let env_project = std::env::var("GOOGLE_CLOUD_PROJECT")
    .ok()
    .or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT_ID").ok());
  let headers = google_project_headers(access_token, false);

  // Try to load existing project via loadCodeAssist
  let response = client
    .post("https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist")
    .headers(headers.clone())
    .json(&serde_json::json!({
      "cloudaicompanionProject": env_project,
      "metadata": {
        "ideType": "IDE_UNSPECIFIED",
        "platform": "PLATFORM_UNSPECIFIED",
        "pluginType": "GEMINI",
        "duetProject": env_project,
      }
    }))
    .send()
    .await;

  if let Ok(response) = response {
    let status = response.status();

    // Try to parse response
    if let Ok(payload) = response.json::<Value>().await {
      // Check for VPC-SC restriction
      if is_vpc_sc_affected_user(&payload) {
        // VPC-SC affected users need standard tier
        return env_project.unwrap_or_else(|| "cokra-vpc-sc-project".to_string());
      }

      // Try to get project ID from response
      if let Some(project_id) = payload
        .get("cloudaicompanionProject")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
      {
        return project_id.to_string();
      }

      if let Some(project_id) = payload
        .get("cloudaicompanionProject")
        .and_then(|v| v.get("id"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
      {
        return project_id.to_string();
      }

      // Check if user already has a tier (existing user)
      if payload.get("currentTier").is_some() {
        // User has tier but no managed project - they need to provide env var
        if let Some(env) = env_project {
          return env;
        }
        // Return default project ID for existing users
        return "cokra-google-cloud-code-assist".to_string();
      }

      // User needs onboarding - get the default tier
      let allowed_tiers: Vec<TierInfo> = payload
        .get("allowedTiers")
        .and_then(Value::as_array)
        .map(|arr| {
          arr
            .iter()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect()
        })
        .unwrap_or_default();

      let tier = get_default_tier(&allowed_tiers);
      let tier_id = tier
        .and_then(|t| t.id.as_ref())
        .map(|s| s.as_str())
        .unwrap_or(TIER_FREE);

      // For free tier, we can onboard without project ID
      // For other tiers, require env var
      if tier_id != TIER_FREE && env_project.is_none() {
        return env_project.unwrap_or_else(|| "cokra-google-cloud-code-assist".to_string());
      }

      // Start onboarding
      let mut onboard_body = serde_json::json!({
        "tierId": tier_id,
        "metadata": {
          "ideType": "IDE_UNSPECIFIED",
          "platform": "PLATFORM_UNSPECIFIED",
          "pluginType": "GEMINI",
        }
      });

      if let Some(env) = &env_project {
        onboard_body["cloudaicompanionProject"] = Value::String(env.clone());
        onboard_body["metadata"]["duetProject"] = Value::String(env.clone());
      }

      let onboard_response = client
        .post("https://cloudcode-pa.googleapis.com/v1internal:onboardUser")
        .headers(headers.clone())
        .json(&onboard_body)
        .send()
        .await;

      if let Ok(onboard_response) = onboard_response {
        if onboard_response.status().is_success() {
          if let Ok(lro_data) = onboard_response
            .json::<LongRunningOperationResponse>()
            .await
          {
            // If operation is not done, poll for completion
            if !lro_data.done {
              if let Some(op_name) = &lro_data.name {
                if let Ok(Some(project_id)) = poll_google_operation(client, op_name, &headers).await
                {
                  return project_id;
                }
              }
            } else if let Some(response) = &lro_data.response {
              if let Some(project) = &response.cloudaicompanion_project {
                if let Some(id) = &project.id {
                  return id.clone();
                }
              }
            }
          }
        }
      }
    }

    // If HTTP was successful but we couldn't extract project, check status
    if status.is_success() {
      return env_project.unwrap_or_else(|| "cokra-google-cloud-code-assist".to_string());
    }
  }

  env_project.unwrap_or_else(|| "cokra-google-cloud-code-assist".to_string())
}

async fn discover_antigravity_project(client: &reqwest::Client, access_token: &str) -> String {
  let headers = google_project_headers(access_token, true);
  for endpoint in [
    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
    "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:loadCodeAssist",
  ] {
    let response = client
      .post(endpoint)
      .headers(headers.clone())
      .json(&serde_json::json!({
        "metadata": {
          "ideType": "IDE_UNSPECIFIED",
          "platform": "PLATFORM_UNSPECIFIED",
          "pluginType": "GEMINI",
        }
      }))
      .send()
      .await;

    if let Ok(response) = response
      && response.status().is_success()
      && let Ok(payload) = response.json::<Value>().await
    {
      if let Some(project_id) = payload
        .get("cloudaicompanionProject")
        .and_then(Value::as_str)
      {
        return project_id.to_string();
      }
      if let Some(project_id) = payload
        .get("cloudaicompanionProject")
        .and_then(|v| v.get("id"))
        .and_then(Value::as_str)
      {
        return project_id.to_string();
      }
    }
  }

  "rising-fact-p41fc".to_string()
}

fn google_project_headers(access_token: &str, antigravity: bool) -> reqwest::header::HeaderMap {
  use reqwest::header::HeaderMap;
  use reqwest::header::HeaderValue;

  let mut headers = HeaderMap::new();
  headers.insert(
    reqwest::header::AUTHORIZATION,
    HeaderValue::from_str(&format!("Bearer {access_token}"))
      .unwrap_or_else(|_| HeaderValue::from_static("")),
  );
  headers.insert(
    reqwest::header::CONTENT_TYPE,
    HeaderValue::from_static("application/json"),
  );
  headers.insert(
    reqwest::header::USER_AGENT,
    HeaderValue::from_static("google-api-nodejs-client/9.15.1"),
  );
  if antigravity {
    headers.insert(
      reqwest::header::HeaderName::from_static("x-goog-api-client"),
      HeaderValue::from_static("google-cloud-sdk vscode_cloudshelleditor/0.1"),
    );
    headers.insert(
      reqwest::header::HeaderName::from_static("client-metadata"),
      HeaderValue::from_static(
        r#"{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}"#,
      ),
    );
    return headers;
  }
  headers.insert(
    reqwest::header::HeaderName::from_static("x-goog-api-client"),
    HeaderValue::from_static("gl-node/22.17.0"),
  );
  headers
}

fn callback_binding(kind: OAuthProviderKind) -> Option<CallbackBinding> {
  match kind {
    OAuthProviderKind::OpenAICodex => Some(CallbackBinding {
      port: 1455,
      path: "/auth/callback",
    }),
    OAuthProviderKind::GoogleGeminiCli => Some(CallbackBinding {
      port: 8085,
      path: "/oauth2callback",
    }),
    OAuthProviderKind::GoogleAntigravity => Some(CallbackBinding {
      port: 51121,
      path: "/oauth-callback",
    }),
    OAuthProviderKind::Anthropic | OAuthProviderKind::GitHubCopilot => None,
  }
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Result<String, AuthError> {
  let mut buf = vec![0_u8; 8192];
  let read = timeout(CALLBACK_READ_TIMEOUT, socket.read(&mut buf))
    .await
    .map_err(|_| AuthError::OAuthError("timed out reading localhost callback request".to_string()))?
    .map_err(|e| {
      AuthError::OAuthError(format!("failed reading localhost callback request: {e}"))
    })?;
  if read == 0 {
    return Err(AuthError::OAuthError(
      "localhost callback connection closed before request".to_string(),
    ));
  }
  String::from_utf8(buf[..read].to_vec())
    .map_err(|e| AuthError::OAuthError(format!("callback request was not utf8: {e}")))
}

fn parse_http_request_target(request: &str) -> Option<(String, String)> {
  let line = request.lines().next()?;
  let mut parts = line.split_whitespace();
  let method = parts.next()?;
  if method != "GET" {
    return None;
  }
  let target = parts.next()?;
  let url = Url::parse(&format!("http://localhost{target}")).ok()?;
  let path = url.path().to_string();
  let query = url.query().unwrap_or_default().to_string();
  Some((path, query))
}

async fn write_http_response(
  socket: &mut tokio::net::TcpStream,
  status: u16,
  body: &str,
) -> Result<(), AuthError> {
  let status_text = match status {
    200 => "OK",
    400 => "Bad Request",
    404 => "Not Found",
    _ => "OK",
  };
  let response = format!(
    "HTTP/1.1 {status} {status_text}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
    body.len(),
    body
  );
  socket
    .write_all(response.as_bytes())
    .await
    .map_err(|e| AuthError::OAuthError(format!("failed writing localhost callback response: {e}")))
}

// ============================================================================
// Token Refresh Infrastructure
// ============================================================================

/// Expiry buffer in seconds (5 minutes)
const TOKEN_EXPIRY_BUFFER_SECS: u64 = 300;

/// Check if credentials need refresh (expired or about to expire)
pub fn credentials_need_refresh(expires_at: u64) -> bool {
  let now = chrono::Utc::now().timestamp() as u64;
  now.saturating_add(TOKEN_EXPIRY_BUFFER_SECS) >= expires_at
}

/// GitHub Copilot token refresh via copilot_internal/v2/token
pub async fn refresh_github_copilot_token(
  refresh_token: &str,
  enterprise_domain: Option<&str>,
) -> Result<(String, u64), AuthError> {
  let domain = enterprise_domain.unwrap_or("github.com");
  let urls = get_github_urls(domain);

  let client = oauth_http_client();
  let response = client
    .get(&urls.copilot_token_url)
    .header("Authorization", format!("Bearer {}", refresh_token))
    .header("User-Agent", "GitHubCopilotChat/0.35.0")
    .header("Editor-Version", "vscode/1.107.0")
    .header("Editor-Plugin-Version", "copilot-chat/0.35.0")
    .header("Copilot-Integration-Id", "vscode-chat")
    .send()
    .await
    .map_err(|e| {
      AuthError::OAuthError(reqwest_error_message(
        "GitHub Copilot token refresh failed",
        &e,
      ))
    })?;

  if !response.status().is_success() {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    return Err(AuthError::OAuthError(format!(
      "GitHub Copilot token refresh failed (HTTP {status}): {body}"
    )));
  }

  #[derive(Debug, Deserialize)]
  struct CopilotTokenResponse {
    token: String,
    expires_at: u64,
  }

  let token_data = response.json::<CopilotTokenResponse>().await.map_err(|e| {
    AuthError::OAuthError(format!(
      "failed to parse GitHub Copilot token response: {e}"
    ))
  })?;

  // Apply expiry buffer
  let expires_at = token_data
    .expires_at
    .saturating_sub(TOKEN_EXPIRY_BUFFER_SECS);

  Ok((token_data.token, expires_at))
}

/// Anthropic token refresh
pub async fn refresh_anthropic_token(refresh_token: &str) -> Result<(String, u64), AuthError> {
  let client = oauth_http_client();
  let response = client
    .post(ANTHROPIC_TOKEN_URL)
    .header("Content-Type", "application/json")
    .json(&serde_json::json!({
      "grant_type": "refresh_token",
      "client_id": decode_b64(ANTHROPIC_CLIENT_ID_B64)?,
      "refresh_token": refresh_token,
    }))
    .send()
    .await
    .map_err(|e| {
      AuthError::OAuthError(reqwest_error_message("Anthropic token refresh failed", &e))
    })?;

  if !response.status().is_success() {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    return Err(AuthError::OAuthError(format!(
      "Anthropic token refresh failed (HTTP {status}): {body}"
    )));
  }

  let token = response
    .json::<TokenResponse>()
    .await
    .map_err(|e| AuthError::OAuthError(format!("failed to parse Anthropic token response: {e}")))?;

  // Apply expiry buffer
  let expires_at = chrono::Utc::now().timestamp() as u64
    + token.expires_in.saturating_sub(TOKEN_EXPIRY_BUFFER_SECS);

  Ok((token.access_token, expires_at))
}

/// Google token refresh (works for both Gemini CLI and Antigravity)
pub async fn refresh_google_token(
  refresh_token: &str,
  client_id: &str,
  client_secret: &str,
  token_url: &str,
) -> Result<(String, u64), AuthError> {
  let client = oauth_http_client();
  let response = client
    .post(token_url)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .form(&[
      ("client_id", client_id),
      ("client_secret", client_secret),
      ("refresh_token", refresh_token),
      ("grant_type", "refresh_token"),
    ])
    .send()
    .await
    .map_err(|e| AuthError::OAuthError(reqwest_error_message("Google token refresh failed", &e)))?;

  if !response.status().is_success() {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    return Err(AuthError::OAuthError(format!(
      "Google token refresh failed (HTTP {status}): {body}"
    )));
  }

  let token = response
    .json::<TokenResponse>()
    .await
    .map_err(|e| AuthError::OAuthError(format!("failed to parse Google token response: {e}")))?;

  // Apply expiry buffer
  let expires_at = chrono::Utc::now().timestamp() as u64
    + token.expires_in.saturating_sub(TOKEN_EXPIRY_BUFFER_SECS);

  Ok((token.access_token, expires_at))
}

/// OpenAI Codex token refresh
pub async fn refresh_openai_codex_token(refresh_token: &str) -> Result<(String, u64), AuthError> {
  let client = oauth_http_client();
  let response = client
    .post(OPENAI_TOKEN_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .form(&[
      ("grant_type", "refresh_token"),
      ("client_id", OPENAI_CLIENT_ID),
      ("refresh_token", refresh_token),
    ])
    .send()
    .await
    .map_err(|e| {
      AuthError::OAuthError(reqwest_error_message(
        "OpenAI Codex token refresh failed",
        &e,
      ))
    })?;

  if !response.status().is_success() {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    return Err(AuthError::OAuthError(format!(
      "OpenAI Codex token refresh failed (HTTP {status}): {body}"
    )));
  }

  let token = response.json::<TokenResponse>().await.map_err(|e| {
    AuthError::OAuthError(format!("failed to parse OpenAI Codex token response: {e}"))
  })?;

  // Apply expiry buffer
  let expires_at = chrono::Utc::now().timestamp() as u64
    + token.expires_in.saturating_sub(TOKEN_EXPIRY_BUFFER_SECS);

  Ok((token.access_token, expires_at))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn callback_bindings_match_expected_routes() {
    let openai = callback_binding(OAuthProviderKind::OpenAICodex).expect("openai binding");
    let gemini = callback_binding(OAuthProviderKind::GoogleGeminiCli).expect("gemini binding");
    let antigravity =
      callback_binding(OAuthProviderKind::GoogleAntigravity).expect("antigravity binding");

    assert_eq!(openai.port, 1455);
    assert_eq!(openai.path, "/auth/callback");
    assert_eq!(gemini.port, 8085);
    assert_eq!(gemini.path, "/oauth2callback");
    assert_eq!(antigravity.port, 51121);
    assert_eq!(antigravity.path, "/oauth-callback");
    assert!(callback_binding(OAuthProviderKind::Anthropic).is_none());
  }

  #[test]
  fn parses_http_request_target_for_callback_path() {
    let parsed = parse_http_request_target(
      "GET /auth/callback?code=abc&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .expect("request target");
    assert_eq!(parsed.0, "/auth/callback");
    assert_eq!(parsed.1, "code=abc&state=xyz");
  }

  #[test]
  fn parses_openai_id_token_info() {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(
      br#"{"email":"user@example.com","organization_id":"org_789","organizations":[{"id":"org_789"}],"https://api.openai.com/auth":{"chatgpt_plan_type":"pro","chatgpt_user_id":"user_123","chatgpt_account_id":"acct_456"}}"#,
    );
    let token = format!("{header}.{payload}.sig");

    let info = parse_openai_id_token_info(&token).expect("openai id token info");

    assert_eq!(
      info,
      OpenAIIdTokenInfo {
        email: Some("user@example.com".to_string()),
        plan_type: Some("pro".to_string()),
        user_id: Some("user_123".to_string()),
        account_id: Some("acct_456".to_string()),
        organization_id: Some("org_789".to_string()),
      }
    );
  }

  #[test]
  fn parses_openai_account_id_from_organizations_fallback() {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(
      br#"{"organizations":[{"id":"org_fallback"}],"https://api.openai.com/auth":{"chatgpt_plan_type":"pro"}}"#,
    );
    let token = format!("{header}.{payload}.sig");

    let info = parse_openai_id_token_info(&token).expect("openai id token info");

    assert_eq!(info.account_id.as_deref(), Some("org_fallback"));
    assert_eq!(info.organization_id.as_deref(), Some("org_fallback"));
  }
}
