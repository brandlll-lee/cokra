#![allow(dead_code)]

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use std::collections::HashMap;
use std::error::Error as _;
use std::future::Future;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;

use futures::future::select_all;
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
use super::plugin_registry::PluginRegistry;
use super::plugin_registry::ProviderPluginKind;

#[derive(Debug, Clone)]
pub struct PendingOAuthConnect {
  pub provider_id: String,
  pub kind: ProviderPluginKind,
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
// NOTE: header names MUST be lowercase for HeaderName::from_static.
const COPILOT_HEADERS: [(&str, &str); 4] = [
  ("user-agent", "GitHubCopilotChat/0.35.0"),
  ("editor-version", "vscode/1.107.0"),
  ("editor-plugin-version", "copilot-chat/0.35.0"),
  ("copilot-integration-id", "vscode-chat"),
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
  if trimmed.contains("://")
    && let Ok(url) = Url::parse(trimmed)
  {
    return Some(url.host_str()?.to_string());
  }
  // Try parsing as a hostname with https prefix
  if let Ok(url) = Url::parse(&format!("https://{}", trimmed))
    && let Some(host) = url.host_str()
  {
    // Validate it looks like a domain
    if host.contains('.') && !host.contains(' ') {
      return Some(host.to_string());
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
  }
}

struct GitHubUrls {
  device_code_url: String,
  access_token_url: String,
  copilot_token_url: String,
}

/// Extract base URL from GitHub Copilot token's proxy-ep field.
/// Token format: tid=...;exp=...;proxy-ep=proxy.individual.githubcopilot.com;...
/// Returns API URL like https://api.individual.githubcopilot.com
pub fn get_github_base_url_from_token(token: &str) -> Option<String> {
  // Find proxy-ep in the token
  for part in token.split(';') {
    if let Some((key, value)) = part.split_once('=')
      && key.trim() == "proxy-ep"
    {
      let proxy_host = value.trim();
      // Convert proxy.xxx to api.xxx
      let api_host = proxy_host.replacen("proxy.", "api.", 1);
      return Some(format!("https://{}", api_host));
    }
  }
  None
}

/// Get the base URL for GitHub Copilot API, preferring token extraction.
pub fn get_github_copilot_base_url(token: Option<&str>, enterprise_domain: Option<&str>) -> String {
  // If we have a token, try to extract the base URL from proxy-ep
  if let Some(t) = token
    && let Some(url) = get_github_base_url_from_token(t)
  {
    return url;
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
pub(super) const GOOGLE_GEMINI_REDIRECT_URI: &str = "http://localhost:8085/oauth2callback";
pub(super) const GOOGLE_GEMINI_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub(super) const GOOGLE_GEMINI_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub(super) const GOOGLE_GEMINI_SCOPES: &[&str] = &[
  "https://www.googleapis.com/auth/cloud-platform",
  "https://www.googleapis.com/auth/userinfo.email",
  "https://www.googleapis.com/auth/userinfo.profile",
];

#[cfg(test)]
const ENV_GOOGLE_ANTIGRAVITY_CLIENT_ID: &str = "COKRA_GOOGLE_ANTIGRAVITY_CLIENT_ID";
#[cfg(test)]
const ENV_GOOGLE_ANTIGRAVITY_CLIENT_SECRET: &str = "COKRA_GOOGLE_ANTIGRAVITY_CLIENT_SECRET";
pub(super) const GOOGLE_ANTIGRAVITY_REDIRECT_URI: &str = "http://localhost:51121/oauth-callback";
pub(super) const GOOGLE_ANTIGRAVITY_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub(super) const GOOGLE_ANTIGRAVITY_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub(super) const GOOGLE_ANTIGRAVITY_SCOPES: &[&str] = &[
  "https://www.googleapis.com/auth/cloud-platform",
  "https://www.googleapis.com/auth/userinfo.email",
  "https://www.googleapis.com/auth/userinfo.profile",
  "https://www.googleapis.com/auth/cclog",
  "https://www.googleapis.com/auth/experimentsandconfigs",
];

const OAUTH_CLIENTS_FILE_NAME: &str = "oauth_clients.json";
const OAUTH_CALLBACK_BIND_MODE_ENV: &str = "COKRA_OAUTH_CALLBACK_BIND_MODE";

#[derive(Debug, Deserialize)]
struct OAuthClientFileEntry {
  #[serde(default)]
  client_id: Option<String>,
  #[serde(default)]
  client_secret: Option<String>,
  #[serde(default)]
  client_id_b64: Option<String>,
  #[serde(default)]
  client_secret_b64: Option<String>,
}

fn env_optional(name: &'static str) -> Option<String> {
  std::env::var(name)
    .ok()
    .filter(|value| !value.trim().is_empty())
}

fn oauth_clients_file_path() -> Option<PathBuf> {
  let home = dirs::home_dir()?;
  Some(home.join(".cokra").join(OAUTH_CLIENTS_FILE_NAME))
}

fn parse_oauth_client_entry(
  entry: &OAuthClientFileEntry,
) -> Result<Option<(String, Option<String>)>, AuthError> {
  let mut client_id = entry
    .client_id
    .as_ref()
    .map(|v| v.trim().to_string())
    .filter(|v| !v.is_empty());

  if client_id.is_none()
    && let Some(b64) = entry
      .client_id_b64
      .as_deref()
      .map(str::trim)
      .filter(|v| !v.is_empty())
  {
    client_id = Some(decode_b64(b64)?);
  }

  let mut client_secret = entry
    .client_secret
    .as_ref()
    .map(|v| v.trim().to_string())
    .filter(|v| !v.is_empty());

  if client_secret.is_none()
    && let Some(b64) = entry
      .client_secret_b64
      .as_deref()
      .map(str::trim)
      .filter(|v| !v.is_empty())
  {
    client_secret = Some(decode_b64(b64)?);
  }

  let Some(client_id) = client_id else {
    return Ok(None);
  };
  Ok(Some((client_id, client_secret)))
}

fn load_oauth_clients_from_file() -> Result<HashMap<String, OAuthClientFileEntry>, AuthError> {
  let Some(path) = oauth_clients_file_path() else {
    return Ok(HashMap::new());
  };
  if !path.exists() {
    return Ok(HashMap::new());
  }
  let content = std::fs::read_to_string(&path).map_err(|e| {
    AuthError::OAuthError(format!(
      "failed to read OAuth clients file {}: {e}",
      path.display()
    ))
  })?;
  if content.trim().is_empty() {
    return Ok(HashMap::new());
  }
  serde_json::from_str::<HashMap<String, OAuthClientFileEntry>>(&content).map_err(|e| {
    AuthError::OAuthError(format!(
      "failed to parse OAuth clients file {}: {e}",
      path.display()
    ))
  })
}

fn oauth_client_from_metadata(metadata: &Value) -> Option<(String, Option<String>)> {
  let client_id = metadata
    .get("oauth_client_id")
    .and_then(Value::as_str)
    .map(str::trim)
    .filter(|v| !v.is_empty())
    .map(ToString::to_string)?;

  let client_secret = metadata
    .get("oauth_client_secret")
    .and_then(Value::as_str)
    .map(str::trim)
    .filter(|v| !v.is_empty())
    .map(ToString::to_string);

  Some((client_id, client_secret))
}

fn google_oauth_provider_descriptor(
  kind: ProviderPluginKind,
) -> Result<super::provider_catalog::ProviderCatalogEntry, AuthError> {
  let descriptor = PluginRegistry::find_by_kind(kind)
    .ok_or_else(|| {
      AuthError::OAuthError(format!("missing auth provider descriptor for {:?}", kind))
    })?
    .catalog;

  if descriptor.id != "google-gemini-cli" && descriptor.id != "google-antigravity" {
    return Err(AuthError::OAuthError(
      "provider is not a Google OAuth flow".to_string(),
    ));
  }

  Ok(descriptor)
}

fn google_oauth_client_optional(
  kind: ProviderPluginKind,
  stored: Option<&StoredCredentials>,
) -> Result<Option<(String, Option<String>)>, AuthError> {
  let descriptor = google_oauth_provider_descriptor(kind)?;
  let oauth_client_env = descriptor.oauth_client_env.ok_or_else(|| {
    AuthError::OAuthError(format!(
      "missing OAuth client environment mapping for {}",
      descriptor.id
    ))
  })?;
  let provider_id = descriptor.id;
  let env_id = oauth_client_env.client_id_env;
  let env_secret = oauth_client_env.client_secret_env;

  // 1) Env vars (explicit and immediate)
  if let Some(client_id) = env_optional(env_id) {
    return Ok(Some((client_id, env_secret.and_then(env_optional))));
  }

  // 2) Local oauth_clients.json (~/.cokra/oauth_clients.json). This matches pi-mono's
  // "works out of the box" behavior without committing secrets into the repository.
  let map = load_oauth_clients_from_file()?;
  if let Some(entry) = map.get(provider_id)
    && let Some(pair) = parse_oauth_client_entry(entry)?
  {
    return Ok(Some(pair));
  }

  // 3) Stored metadata fallback (post-connect refresh parity without requiring env vars).
  if let Some(stored) = stored
    && let Some(pair) = oauth_client_from_metadata(&stored.metadata)
  {
    return Ok(Some(pair));
  }

  Ok(None)
}

pub(super) fn google_oauth_client(
  kind: ProviderPluginKind,
  stored: Option<&StoredCredentials>,
) -> Result<(String, Option<String>), AuthError> {
  let descriptor = google_oauth_provider_descriptor(kind)?;
  let oauth_client_env = descriptor.oauth_client_env.ok_or_else(|| {
    AuthError::OAuthError(format!(
      "missing OAuth client environment mapping for {}",
      descriptor.id
    ))
  })?;
  let provider_id = descriptor.id;
  let env_id = oauth_client_env.client_id_env;
  let env_secret = oauth_client_env.client_secret_env.unwrap_or("<none>");

  google_oauth_client_optional(kind, stored)?.ok_or_else(|| {
    AuthError::OAuthError(format!(
      "missing OAuth client configuration for {provider_id}: set {env_id} (and optionally {env_secret}) or provide ~/.cokra/{OAUTH_CLIENTS_FILE_NAME}"
    ))
  })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalCallbackBindPreference {
  Auto,
  Loopback,
  Wildcard,
}

impl LocalCallbackBindPreference {
  fn parse(raw: &str) -> Option<Self> {
    match raw.trim().to_ascii_lowercase().as_str() {
      "auto" => Some(Self::Auto),
      "loopback" | "local" | "localhost" => Some(Self::Loopback),
      "wildcard" | "all" | "any" => Some(Self::Wildcard),
      _ => None,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalCallbackBindStrategy {
  Loopback,
  Wildcard,
}

impl LocalCallbackBindStrategy {
  fn name(self) -> &'static str {
    match self {
      Self::Loopback => "loopback",
      Self::Wildcard => "wildcard",
    }
  }

  fn listener_endpoints(self, port: u16) -> Vec<LocalCallbackListenerEndpoint> {
    match self {
      Self::Loopback => vec![
        LocalCallbackListenerEndpoint::new(
          "ipv4-loopback",
          SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        ),
        LocalCallbackListenerEndpoint::new(
          "ipv6-loopback",
          SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
        ),
      ],
      Self::Wildcard => vec![
        LocalCallbackListenerEndpoint::new(
          "ipv4-wildcard",
          SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port),
        ),
        // Tradeoff: keep IPv6 on loopback even in wildcard mode. Binding [::]
        // can monopolize the port on Linux and block the IPv4 wildcard
        // listener that WSL localhost forwarding depends on.
        LocalCallbackListenerEndpoint::new(
          "ipv6-loopback",
          SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
        ),
      ],
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LocalCallbackRuntime {
  is_wsl: bool,
}

impl LocalCallbackRuntime {
  fn detect() -> Self {
    let env_wsl_distro = std::env::var("WSL_DISTRO_NAME").ok();
    let env_wsl_interop = std::env::var("WSL_INTEROP").ok();
    let proc_version = std::fs::read_to_string("/proc/version").ok();
    let proc_osrelease = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok();

    Self::from_signals(
      env_wsl_distro.as_deref(),
      env_wsl_interop.as_deref(),
      proc_version.as_deref(),
      proc_osrelease.as_deref(),
    )
  }

  fn from_signals(
    env_wsl_distro: Option<&str>,
    env_wsl_interop: Option<&str>,
    proc_version: Option<&str>,
    proc_osrelease: Option<&str>,
  ) -> Self {
    let has_wsl_env = env_wsl_distro.is_some() || env_wsl_interop.is_some();
    let proc_mentions_wsl = proc_version
      .into_iter()
      .chain(proc_osrelease)
      .any(|value| value.to_ascii_lowercase().contains("microsoft"));

    Self {
      is_wsl: has_wsl_env || proc_mentions_wsl,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LocalCallbackListenerEndpoint {
  label: &'static str,
  addr: SocketAddr,
}

impl LocalCallbackListenerEndpoint {
  const fn new(label: &'static str, addr: SocketAddr) -> Self {
    Self { label, addr }
  }
}

#[derive(Debug)]
struct LocalCallbackListener {
  endpoint: LocalCallbackListenerEndpoint,
  listener: TcpListener,
}

#[derive(Debug)]
struct LocalCallbackServer {
  binding: CallbackBinding,
  strategy: LocalCallbackBindStrategy,
  listeners: Vec<LocalCallbackListener>,
}

impl LocalCallbackServer {
  async fn bind(binding: CallbackBinding) -> Result<Self, AuthError> {
    let runtime = LocalCallbackRuntime::detect();
    let strategy = resolve_local_callback_bind_strategy(runtime)?;
    let endpoints = strategy.listener_endpoints(binding.port);
    let mut listeners = Vec::new();
    let mut errors = Vec::new();

    tracing::debug!(
      "starting OAuth callback server on port {} with {} strategy (wsl={})",
      binding.port,
      strategy.name(),
      runtime.is_wsl
    );

    for endpoint in endpoints {
      match TcpListener::bind(endpoint.addr).await {
        Ok(listener) => {
          tracing::debug!(
            "bound localhost callback server endpoint {} on {}",
            endpoint.label,
            endpoint.addr
          );
          listeners.push(LocalCallbackListener { endpoint, listener });
        }
        Err(err) => {
          tracing::debug!(
            "failed to bind localhost callback endpoint {} on {}: {}",
            endpoint.label,
            endpoint.addr,
            err
          );
          errors.push(format!("{} ({}): {}", endpoint.label, endpoint.addr, err));
        }
      }
    }

    if listeners.is_empty() {
      return Err(AuthError::OAuthError(format!(
        "failed to bind localhost callback server on port {} using {} strategy: {}",
        binding.port,
        strategy.name(),
        errors.join(" | ")
      )));
    }

    Ok(Self {
      binding,
      strategy,
      listeners,
    })
  }

  async fn wait_for_callback(self, cancel: CancellationToken) -> Result<String, AuthError> {
    tracing::debug!(
      "OAuth callback server ready with {} strategy; waiting for connections...",
      self.strategy.name()
    );

    let accept_future = async {
      loop {
        let (mut socket, addr, endpoint) = self.accept_socket(&cancel).await?;

        tracing::debug!(
          "received localhost callback connection from {} via {} ({})",
          addr,
          endpoint.label,
          endpoint.addr
        );

        let request = read_http_request(&mut socket).await?;
        tracing::debug!(
          "received HTTP request for path: {:?}",
          request.lines().next()
        );

        let Some((path, query)) = parse_http_request_target(&request) else {
          write_http_response(&mut socket, 400, CALLBACK_FAILURE_HTML).await?;
          continue;
        };

        if path != self.binding.path {
          write_http_response(&mut socket, 404, CALLBACK_FAILURE_HTML).await?;
          continue;
        }

        let callback_url = if query.is_empty() {
          format!("http://localhost:{}{}", self.binding.port, path)
        } else {
          format!("http://localhost:{}{}?{}", self.binding.port, path, query)
        };

        let parsed = parse_auth_input(&callback_url);
        if parsed.code.is_none() {
          tracing::warn!("OAuth callback missing authorization code");
          write_http_response(&mut socket, 400, CALLBACK_FAILURE_HTML).await?;
          continue;
        }

        tracing::info!("OAuth callback received successfully, returning authorization code");
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

  async fn accept_socket(
    &self,
    cancel: &CancellationToken,
  ) -> Result<
    (
      tokio::net::TcpStream,
      std::net::SocketAddr,
      LocalCallbackListenerEndpoint,
    ),
    AuthError,
  > {
    let accept_futures = self
      .listeners
      .iter()
      .map(|entry| {
        Box::pin(entry.listener.accept())
          as Pin<
            Box<
              dyn Future<Output = std::io::Result<(tokio::net::TcpStream, SocketAddr)>> + Send + '_,
            >,
          >
      })
      .collect::<Vec<_>>();

    let (result, index, _) = tokio::select! {
      _ = cancel.cancelled() => {
        return Err(AuthError::OAuthError("OAuth callback cancelled".to_string()));
      }
      result = select_all(accept_futures) => result,
    };

    let endpoint = self
      .listeners
      .get(index)
      .map(|entry| entry.endpoint)
      .ok_or_else(|| AuthError::OAuthError("callback listener index out of bounds".to_string()))?;

    let (socket, addr) = result.map_err(|err| {
      AuthError::OAuthError(format!("failed to accept localhost callback: {err}"))
    })?;

    Ok((socket, addr, endpoint))
  }
}

fn resolve_local_callback_bind_strategy(
  runtime: LocalCallbackRuntime,
) -> Result<LocalCallbackBindStrategy, AuthError> {
  let preference = env_optional(OAUTH_CALLBACK_BIND_MODE_ENV)
    .map(|raw| {
      LocalCallbackBindPreference::parse(&raw).ok_or_else(|| {
        AuthError::OAuthError(format!(
          "invalid {} value {:?}; expected one of: auto, loopback, wildcard",
          OAUTH_CALLBACK_BIND_MODE_ENV, raw
        ))
      })
    })
    .transpose()?
    .unwrap_or(LocalCallbackBindPreference::Auto);

  let strategy = match preference {
    LocalCallbackBindPreference::Auto => {
      if runtime.is_wsl {
        // Tradeoff: WSL auto mode binds IPv4 wildcard so Windows localhost
        // forwarding can reach the Linux process. State verification remains
        // the security boundary for this short-lived callback listener.
        LocalCallbackBindStrategy::Wildcard
      } else {
        LocalCallbackBindStrategy::Loopback
      }
    }
    LocalCallbackBindPreference::Loopback => LocalCallbackBindStrategy::Loopback,
    LocalCallbackBindPreference::Wildcard => LocalCallbackBindStrategy::Wildcard,
  };

  Ok(strategy)
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
pub(super) async fn start_github_copilot_connect_with_domain(
  provider_id: &str,
  provider_name: &str,
  enterprise_domain: Option<&str>,
) -> Result<OAuthConnectStart, AuthError> {
  start_github_copilot_connect(provider_id, provider_name, enterprise_domain).await
}

pub(super) fn oauth_refresh_config_for_provider_with_stored(
  provider_id: &str,
  stored: Option<&StoredCredentials>,
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
      google_oauth_client_optional(ProviderPluginKind::GoogleGeminiCli, stored)?.map(
        |(client_id, client_secret)| OAuthConfig {
          provider_id: provider_id.to_string(),
          client_id,
          client_secret,
          auth_url: GOOGLE_GEMINI_AUTH_URL.to_string(),
          token_url: GOOGLE_GEMINI_TOKEN_URL.to_string(),
          scopes: GOOGLE_GEMINI_SCOPES
            .iter()
            .map(|scope| (*scope).to_string())
            .collect(),
          redirect_uri: GOOGLE_GEMINI_REDIRECT_URI.to_string(),
        },
      )
    }
    "google-antigravity" => {
      google_oauth_client_optional(ProviderPluginKind::GoogleAntigravity, stored)?.map(
        |(client_id, client_secret)| OAuthConfig {
          provider_id: provider_id.to_string(),
          client_id,
          client_secret,
          auth_url: GOOGLE_ANTIGRAVITY_AUTH_URL.to_string(),
          token_url: GOOGLE_ANTIGRAVITY_TOKEN_URL.to_string(),
          scopes: GOOGLE_ANTIGRAVITY_SCOPES
            .iter()
            .map(|scope| (*scope).to_string())
            .collect(),
          redirect_uri: GOOGLE_ANTIGRAVITY_REDIRECT_URI.to_string(),
        },
      )
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

  let server = LocalCallbackServer::bind(binding).await?;
  server.wait_for_callback(cancel).await
}

pub(super) fn start_anthropic_connect(
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
      kind: ProviderPluginKind::AnthropicOAuth,
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

pub(super) fn start_openai_codex_connect(
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
      kind: ProviderPluginKind::OpenAICodex,
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

pub(super) fn start_google_connect(
  provider_id: &str,
  provider_name: &str,
  kind: ProviderPluginKind,
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
      kind: ProviderPluginKind::GitHubCopilot,
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

pub(super) async fn complete_anthropic_connect(
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

pub(super) async fn complete_openai_codex_connect(
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

pub(super) async fn complete_google_connect(
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

  let mut metadata = serde_json::json!({
    "email": email,
    "project_id": project_id,
    "oauth_client_id": client_id,
  });
  if let Some(secret) = client_secret {
    metadata["oauth_client_secret"] = Value::String(secret);
  }

  Ok(oauth_stored_credentials(
    &pending.provider_id,
    token,
    None,
    None,
    metadata,
  ))
}

pub(super) async fn complete_github_copilot_connect(
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

  let enterprise_domain_ref = pending.enterprise_domain.as_deref();

  // Exchange the GitHub access token for a Copilot token (pi-mono parity).
  let (copilot_access_token, expires_at) =
    refresh_github_copilot_token(&github_access_token, enterprise_domain_ref).await?;

  // Enable all GitHub Copilot models after successful login.
  let enablement_results =
    enable_all_github_copilot_models(&copilot_access_token, enterprise_domain_ref).await;
  let enabled_count = enablement_results
    .iter()
    .filter(|(_, success)| *success)
    .count();

  // Extract base URL from the Copilot token for future API calls
  let base_url = get_github_copilot_base_url(Some(&copilot_access_token), enterprise_domain_ref);

  Ok(
    StoredCredentials::new(
      pending.provider_id.clone(),
      Credentials::OAuth {
        access_token: copilot_access_token,
        refresh_token: github_access_token,
        expires_at,
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
  if let Some(error) = payload.get("error")
    && let Some(details) = error.get("details")
    && let Some(details_arr) = details.as_array()
  {
    return details_arr.iter().any(|detail| {
      detail
        .get("reason")
        .and_then(Value::as_str)
        .map(|r| r == "SECURITY_POLICY_VIOLATED")
        .unwrap_or(false)
    });
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

    if let Some(data) = payload
      && data.done
    {
      if let Some(response) = data.response
        && let Some(project) = response.cloudaicompanion_project
        && let Some(id) = project.id
      {
        return Ok(Some(id));
      }
      return Ok(None);
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

      if let Ok(onboard_response) = onboard_response
        && onboard_response.status().is_success()
        && let Ok(lro_data) = onboard_response
          .json::<LongRunningOperationResponse>()
          .await
      {
        // If operation is not done, poll for completion
        if !lro_data.done {
          if let Some(op_name) = &lro_data.name
            && let Ok(Some(project_id)) = poll_google_operation(client, op_name, &headers).await
          {
            return project_id;
          }
        } else if let Some(response) = &lro_data.response
          && let Some(project) = &response.cloudaicompanion_project
          && let Some(id) = &project.id
        {
          return id.clone();
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

fn callback_binding(kind: ProviderPluginKind) -> Option<CallbackBinding> {
  match kind {
    ProviderPluginKind::OpenAICodex => Some(CallbackBinding {
      port: 1455,
      path: "/auth/callback",
    }),
    ProviderPluginKind::GoogleGeminiCli => Some(CallbackBinding {
      port: 8085,
      path: "/oauth2callback",
    }),
    ProviderPluginKind::GoogleAntigravity => Some(CallbackBinding {
      port: 51121,
      path: "/oauth-callback",
    }),
    ProviderPluginKind::AnthropicOAuth | ProviderPluginKind::GitHubCopilot => None,
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
  use std::sync::Mutex;

  static OAUTH_ENV_LOCK: Mutex<()> = Mutex::new(());

  #[test]
  fn callback_bindings_match_expected_routes() {
    let openai = callback_binding(ProviderPluginKind::OpenAICodex).expect("openai binding");
    let gemini = callback_binding(ProviderPluginKind::GoogleGeminiCli).expect("gemini binding");
    let antigravity =
      callback_binding(ProviderPluginKind::GoogleAntigravity).expect("antigravity binding");

    assert_eq!(openai.port, 1455);
    assert_eq!(openai.path, "/auth/callback");
    assert_eq!(gemini.port, 8085);
    assert_eq!(gemini.path, "/oauth2callback");
    assert_eq!(antigravity.port, 51121);
    assert_eq!(antigravity.path, "/oauth-callback");
    assert!(callback_binding(ProviderPluginKind::AnthropicOAuth).is_none());
  }

  #[test]
  fn local_callback_runtime_detects_wsl_from_signals() {
    let runtime = LocalCallbackRuntime::from_signals(
      Some("Ubuntu"),
      None,
      Some("Linux version 5.15.167.4-microsoft-standard-WSL2"),
      None,
    );

    assert!(runtime.is_wsl);
  }

  #[test]
  fn local_callback_bind_strategy_defaults_to_loopback_off_wsl() {
    let _guard = OAUTH_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var(OAUTH_CALLBACK_BIND_MODE_ENV).ok();
    unsafe {
      std::env::remove_var(OAUTH_CALLBACK_BIND_MODE_ENV);
    }

    let strategy = resolve_local_callback_bind_strategy(LocalCallbackRuntime { is_wsl: false })
      .expect("bind strategy");

    assert_eq!(strategy, LocalCallbackBindStrategy::Loopback);

    unsafe {
      if let Some(value) = prev {
        std::env::set_var(OAUTH_CALLBACK_BIND_MODE_ENV, value);
      }
    }
  }

  #[test]
  fn local_callback_bind_strategy_defaults_to_wildcard_under_wsl() {
    let _guard = OAUTH_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var(OAUTH_CALLBACK_BIND_MODE_ENV).ok();
    unsafe {
      std::env::remove_var(OAUTH_CALLBACK_BIND_MODE_ENV);
    }

    let strategy = resolve_local_callback_bind_strategy(LocalCallbackRuntime { is_wsl: true })
      .expect("bind strategy");

    assert_eq!(strategy, LocalCallbackBindStrategy::Wildcard);

    unsafe {
      if let Some(value) = prev {
        std::env::set_var(OAUTH_CALLBACK_BIND_MODE_ENV, value);
      }
    }
  }

  #[test]
  fn local_callback_bind_strategy_respects_env_override() {
    let _guard = OAUTH_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var(OAUTH_CALLBACK_BIND_MODE_ENV).ok();
    unsafe {
      std::env::set_var(OAUTH_CALLBACK_BIND_MODE_ENV, "loopback");
    }

    let strategy = resolve_local_callback_bind_strategy(LocalCallbackRuntime { is_wsl: true })
      .expect("bind strategy");

    assert_eq!(strategy, LocalCallbackBindStrategy::Loopback);

    unsafe {
      if let Some(value) = prev {
        std::env::set_var(OAUTH_CALLBACK_BIND_MODE_ENV, value);
      } else {
        std::env::remove_var(OAUTH_CALLBACK_BIND_MODE_ENV);
      }
    }
  }

  #[test]
  fn local_callback_bind_strategy_rejects_invalid_override() {
    let _guard = OAUTH_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var(OAUTH_CALLBACK_BIND_MODE_ENV).ok();
    unsafe {
      std::env::set_var(OAUTH_CALLBACK_BIND_MODE_ENV, "invalid-mode");
    }

    let err = resolve_local_callback_bind_strategy(LocalCallbackRuntime { is_wsl: false })
      .expect_err("invalid bind mode should fail");

    assert!(
      err
        .to_string()
        .contains("invalid COKRA_OAUTH_CALLBACK_BIND_MODE value")
    );

    unsafe {
      if let Some(value) = prev {
        std::env::set_var(OAUTH_CALLBACK_BIND_MODE_ENV, value);
      } else {
        std::env::remove_var(OAUTH_CALLBACK_BIND_MODE_ENV);
      }
    }
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

  #[test]
  fn google_refresh_config_can_use_stored_oauth_client_metadata() {
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
      "oauth_client_id": "client-id-from-metadata",
      "oauth_client_secret": "client-secret-from-metadata",
    });

    let cfg = oauth_refresh_config_for_provider_with_stored("google-antigravity", Some(&stored))
      .expect("config resolution")
      .expect("oauth config");

    assert_eq!(cfg.client_id, "client-id-from-metadata");
    assert_eq!(
      cfg.client_secret.as_deref(),
      Some("client-secret-from-metadata")
    );
  }

  #[test]
  fn google_refresh_config_prefers_env_over_stored_oauth_client_metadata() {
    let _guard = OAUTH_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev_client_id = std::env::var(ENV_GOOGLE_ANTIGRAVITY_CLIENT_ID).ok();
    let prev_client_secret = std::env::var(ENV_GOOGLE_ANTIGRAVITY_CLIENT_SECRET).ok();
    unsafe {
      std::env::set_var(ENV_GOOGLE_ANTIGRAVITY_CLIENT_ID, "client-id-from-env");
      std::env::set_var(
        ENV_GOOGLE_ANTIGRAVITY_CLIENT_SECRET,
        "client-secret-from-env",
      );
    }

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
      "oauth_client_id": "client-id-from-metadata",
      "oauth_client_secret": "client-secret-from-metadata",
    });

    let cfg = oauth_refresh_config_for_provider_with_stored("google-antigravity", Some(&stored))
      .expect("config resolution")
      .expect("oauth config");

    assert_eq!(cfg.client_id, "client-id-from-env");
    assert_eq!(cfg.client_secret.as_deref(), Some("client-secret-from-env"));

    unsafe {
      if let Some(value) = prev_client_id {
        std::env::set_var(ENV_GOOGLE_ANTIGRAVITY_CLIENT_ID, value);
      } else {
        std::env::remove_var(ENV_GOOGLE_ANTIGRAVITY_CLIENT_ID);
      }
      if let Some(value) = prev_client_secret {
        std::env::set_var(ENV_GOOGLE_ANTIGRAVITY_CLIENT_SECRET, value);
      } else {
        std::env::remove_var(ENV_GOOGLE_ANTIGRAVITY_CLIENT_SECRET);
      }
    }
  }

  #[test]
  fn google_oauth_client_uses_env_when_set() {
    let _guard = OAUTH_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev_client_id = std::env::var(ENV_GOOGLE_ANTIGRAVITY_CLIENT_ID).ok();
    let prev_client_secret = std::env::var(ENV_GOOGLE_ANTIGRAVITY_CLIENT_SECRET).ok();
    unsafe {
      std::env::set_var(ENV_GOOGLE_ANTIGRAVITY_CLIENT_ID, "client-id-from-env");
      std::env::set_var(
        ENV_GOOGLE_ANTIGRAVITY_CLIENT_SECRET,
        "client-secret-from-env",
      );
    }

    let (client_id, client_secret) =
      google_oauth_client(ProviderPluginKind::GoogleAntigravity, None).expect("oauth client");
    assert_eq!(client_id, "client-id-from-env");
    assert_eq!(client_secret.as_deref(), Some("client-secret-from-env"));

    unsafe {
      if let Some(value) = prev_client_id {
        std::env::set_var(ENV_GOOGLE_ANTIGRAVITY_CLIENT_ID, value);
      } else {
        std::env::remove_var(ENV_GOOGLE_ANTIGRAVITY_CLIENT_ID);
      }
      if let Some(value) = prev_client_secret {
        std::env::set_var(ENV_GOOGLE_ANTIGRAVITY_CLIENT_SECRET, value);
      } else {
        std::env::remove_var(ENV_GOOGLE_ANTIGRAVITY_CLIENT_SECRET);
      }
    }
  }
}
pub(super) fn uses_local_callback(kind: ProviderPluginKind) -> bool {
  callback_binding(kind).is_some()
}
