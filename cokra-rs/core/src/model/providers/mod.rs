//! Model provider implementations
//!
//! Individual implementations for each supported LLM provider

use reqwest::Client;
use serde_json::Value;
use serde_json::json;

use super::connect_catalog::RuntimeRegistrationKind;
use super::error::ModelError;
use super::error::Result;
use super::registry::ProviderRegistry;
use super::streaming::OpenAIUsageParser;
use super::streaming::StreamingConfig;
use super::streaming::StreamingProcessor;
use super::streaming::UsageParser;
use super::types::ChatRequest;
use super::types::ChatResponse;
use super::types::Chunk;
use super::types::Message;
use super::types::ProviderConfig;

pub mod anthropic;
pub mod codex;
pub mod github;
pub mod google;
pub mod google_cloud_code;
pub mod lmstudio;
pub mod ollama;
pub mod openai;
pub mod openrouter;

pub use anthropic::AnthropicProvider;
pub use codex::OpenAICodexProvider;
pub use github::GitHubCopilotProvider;
pub use google::GoogleProvider;
pub use google_cloud_code::GoogleCloudCodeProvider;
pub use lmstudio::LMStudioProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAIProvider;
pub use openrouter::OpenRouterProvider;

/// Register all default providers
pub async fn register_all_providers(
  registry: &ProviderRegistry,
  config: &cokra_config::Config,
) -> Result<()> {
  // OpenAI will be registered if credentials are found
  if let Ok(openai_key) = std::env::var("OPENAI_API_KEY") {
    let openai = OpenAIProvider::new(
      openai_key.clone(),
      ProviderConfig {
        provider_id: "openai".to_string(),
        api_key: Some(openai_key),
        base_url: provider_base_url(config, "openai"),
        ..Default::default()
      },
    );
    registry
      .register_with_config(openai, config_to_provider_config(config, "openai"))
      .await;
  }

  // Anthropic
  if let Ok(anthropic_key) = std::env::var("ANTHROPIC_API_KEY") {
    let anthropic = AnthropicProvider::new(
      anthropic_key.clone(),
      ProviderConfig {
        provider_id: "anthropic".to_string(),
        api_key: Some(anthropic_key),
        base_url: provider_base_url(config, "anthropic"),
        ..Default::default()
      },
    );
    registry
      .register_with_config(anthropic, config_to_provider_config(config, "anthropic"))
      .await;
  }

  // OpenRouter
  if let Ok(openrouter_key) = std::env::var("OPENROUTER_API_KEY") {
    let openrouter = OpenRouterProvider::new(
      openrouter_key.clone(),
      ProviderConfig {
        provider_id: "openrouter".to_string(),
        api_key: Some(openrouter_key),
        base_url: provider_base_url(config, "openrouter"),
        ..Default::default()
      },
    );
    registry
      .register_with_config(openrouter, config_to_provider_config(config, "openrouter"))
      .await;
  }

  // Google Gemini
  if let Ok(google_key) = std::env::var("GOOGLE_API_KEY") {
    let google = GoogleProvider::new(
      google_key.clone(),
      ProviderConfig {
        provider_id: "google".to_string(),
        api_key: Some(google_key),
        base_url: provider_base_url(config, "google"),
        ..Default::default()
      },
    );
    registry
      .register_with_config(google, config_to_provider_config(config, "google"))
      .await;
  }

  // Ollama (local, no auth needed)
  {
    let ollama = OllamaProvider::new(provider_base_url(config, "ollama"));
    registry
      .register_with_config(ollama, config_to_provider_config(config, "ollama"))
      .await;
  }

  // LM Studio (local, no auth needed)
  {
    let lmstudio = LMStudioProvider::new(provider_base_url(config, "lmstudio"));
    registry
      .register_with_config(lmstudio, config_to_provider_config(config, "lmstudio"))
      .await;
  }

  // GitHub Copilot
  if let Ok(copilot_token) =
    std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GITHUB_COPILOT_TOKEN"))
  {
    let copilot = GitHubCopilotProvider::new(
      copilot_token.clone(),
      ProviderConfig {
        provider_id: "github".to_string(),
        api_key: Some(copilot_token),
        base_url: provider_base_url(config, "github"),
        ..Default::default()
      },
    );
    registry
      .register_with_config(copilot, config_to_provider_config(config, "github"))
      .await;
  }

  register_stored_connect_providers(registry, config).await?;

  // Set default provider from config
  if !config.models.provider.is_empty() {
    let default_provider = &config.models.provider;

    // 1:1 opencode: ensure the configured provider is registered even if
    // the corresponding env var was not set. The user explicitly asked for
    // this provider via `-c models.provider=<id>`, so we should honour it.
    // The API key can come from the config, env, or even be absent (for
    // providers like ollama/lmstudio that don't need auth).
    if !registry.has_provider(default_provider).await {
      let api_key = config.models.api_key.clone().unwrap_or_default();
      let provider_config = ProviderConfig {
        provider_id: default_provider.to_string(),
        api_key: if api_key.is_empty() {
          None
        } else {
          Some(api_key.clone())
        },
        base_url: provider_base_url(config, default_provider),
        ..Default::default()
      };
      match default_provider.as_str() {
        "openai" => {
          let p = OpenAIProvider::new(api_key, provider_config);
          registry
            .register_with_config(p, config_to_provider_config(config, default_provider))
            .await;
        }
        "anthropic" => {
          let p = AnthropicProvider::new(api_key, provider_config);
          registry
            .register_with_config(p, config_to_provider_config(config, default_provider))
            .await;
        }
        "openrouter" => {
          let p = OpenRouterProvider::new(api_key, provider_config);
          registry
            .register_with_config(p, config_to_provider_config(config, default_provider))
            .await;
        }
        "google" => {
          let p = GoogleProvider::new(api_key, provider_config);
          registry
            .register_with_config(p, config_to_provider_config(config, default_provider))
            .await;
        }
        "google-gemini-cli" => {
          let p = GoogleCloudCodeProvider::new_gemini_cli(api_key, provider_config)?;
          registry
            .register_with_config(p, config_to_provider_config(config, default_provider))
            .await;
        }
        "google-antigravity" => {
          let p = GoogleCloudCodeProvider::new_antigravity(api_key, provider_config)?;
          registry
            .register_with_config(p, config_to_provider_config(config, default_provider))
            .await;
        }
        "ollama" => {
          let p = OllamaProvider::new(provider_base_url(config, default_provider));
          registry
            .register_with_config(p, config_to_provider_config(config, default_provider))
            .await;
        }
        "lmstudio" => {
          let p = LMStudioProvider::new(provider_base_url(config, default_provider));
          registry
            .register_with_config(p, config_to_provider_config(config, default_provider))
            .await;
        }
        "github" => {
          let p = GitHubCopilotProvider::new(api_key, provider_config);
          registry
            .register_with_config(p, config_to_provider_config(config, default_provider))
            .await;
        }
        _ => {
          // Unknown provider — treat as OpenAI-compatible (like opencode's
          // createOpenAICompatible fallback).
          let p = OpenAIProvider::new(api_key, provider_config);
          registry
            .register_with_config(p, config_to_provider_config(config, default_provider))
            .await;
        }
      }
    }

    registry.set_default(default_provider).await.ok();
  }

  Ok(())
}

pub async fn register_provider_by_registration(
  registry: &ProviderRegistry,
  config: &cokra_config::Config,
  registration: RuntimeRegistrationKind,
  token: String,
  source_entry_id: Option<&str>,
  stored: Option<&super::auth::StoredCredentials>,
  existing_config: Option<&ProviderConfig>,
) -> Result<()> {
  match registration {
    RuntimeRegistrationKind::None => return Ok(()),
    RuntimeRegistrationKind::OpenAI => {
      let runtime_config = registration_config(
        config,
        "openai",
        token.clone(),
        source_entry_id,
        stored,
        existing_config,
      );
      let provider = OpenAIProvider::new(token, runtime_config.clone());
      registry
        .register_with_config(provider, runtime_config)
        .await;
    }
    RuntimeRegistrationKind::OpenAICodex => {
      let stored = stored.ok_or_else(|| {
        ModelError::AuthError(
          "OpenAI Codex runtime registration requires stored OAuth credentials".to_string(),
        )
      })?;
      let runtime_config = registration_config(
        config,
        "openai",
        token,
        source_entry_id,
        Some(stored),
        existing_config,
      );
      let provider = OpenAICodexProvider::new(stored, runtime_config.clone())?;
      registry
        .register_with_config(provider, runtime_config)
        .await;
    }
    RuntimeRegistrationKind::Anthropic => {
      let runtime_config = registration_config(
        config,
        "anthropic",
        token.clone(),
        source_entry_id,
        stored,
        existing_config,
      );
      let provider = AnthropicProvider::new(token, runtime_config.clone());
      registry
        .register_with_config(provider, runtime_config)
        .await;
    }
    RuntimeRegistrationKind::GitHubCopilot => {
      let runtime_config = registration_config(
        config,
        "github",
        token.clone(),
        source_entry_id,
        stored,
        existing_config,
      );
      let provider = if let Some(stored) = stored {
        if matches!(stored.credentials, super::auth::Credentials::OAuth { .. }) {
          GitHubCopilotProvider::new_oauth(stored, runtime_config.clone())?
        } else {
          GitHubCopilotProvider::new(token, runtime_config.clone())
        }
      } else {
        GitHubCopilotProvider::new(token, runtime_config.clone())
      };
      registry
        .register_with_config(provider, runtime_config)
        .await;
    }
    RuntimeRegistrationKind::Google => {
      let runtime_config = registration_config(
        config,
        "google",
        token.clone(),
        source_entry_id,
        stored,
        existing_config,
      );
      let provider = GoogleProvider::new(token, runtime_config.clone());
      registry
        .register_with_config(provider, runtime_config)
        .await;
    }
    RuntimeRegistrationKind::OpenRouter => {
      let runtime_config = registration_config(
        config,
        "openrouter",
        token.clone(),
        source_entry_id,
        stored,
        existing_config,
      );
      let provider = OpenRouterProvider::new(token, runtime_config.clone());
      registry
        .register_with_config(provider, runtime_config)
        .await;
    }
    RuntimeRegistrationKind::GoogleGeminiCli => {
      let runtime_config = registration_config(
        config,
        "google-gemini-cli",
        token.clone(),
        source_entry_id,
        stored,
        existing_config,
      );
      let provider = GoogleCloudCodeProvider::new_gemini_cli(token, runtime_config.clone())?;
      registry
        .register_with_config(provider, runtime_config)
        .await;
    }
    RuntimeRegistrationKind::GoogleAntigravity => {
      let runtime_config = registration_config(
        config,
        "google-antigravity",
        token.clone(),
        source_entry_id,
        stored,
        existing_config,
      );
      let provider = GoogleCloudCodeProvider::new_antigravity(token, runtime_config.clone())?;
      registry
        .register_with_config(provider, runtime_config)
        .await;
    }
  }
  Ok(())
}

async fn register_stored_connect_providers(
  registry: &ProviderRegistry,
  config: &cokra_config::Config,
) -> Result<()> {
  let auth = match super::auth::AuthManager::new() {
    Ok(auth) => auth,
    Err(_) => return Ok(()),
  };

  let openai_codex_present = auth.load("openai-codex").await.ok().flatten().is_some();

  for entry in super::connect_catalog::connect_provider_catalog() {
    if entry.runtime_registration == RuntimeRegistrationKind::None {
      continue;
    }
    if entry.id == "openai" && openai_codex_present {
      continue;
    }
    let stored = match auth.load_for_runtime_registration(entry.id).await {
      Ok(Some(stored)) => stored,
      Ok(None) => continue,
      Err(super::auth::AuthError::TokenExpired(_)) => continue,
      Err(_) => continue,
    };
    let Some(token) = registration_token_for_stored(entry.runtime_registration, &stored) else {
      continue;
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
  }

  Ok(())
}

/// Convert Cokra config to provider config
fn config_to_provider_config(
  config: &cokra_config::Config,
  provider_id: &str,
) -> super::types::ProviderConfig {
  super::types::ProviderConfig {
    provider_id: provider_id.to_string(),
    api_key: None, // Will be loaded from env
    base_url: provider_base_url(config, provider_id),
    ..Default::default()
  }
}

fn provider_base_url(config: &cokra_config::Config, provider_id: &str) -> Option<String> {
  if config.models.provider == provider_id {
    return config.models.base_url.clone();
  }
  None
}

fn registration_config(
  config: &cokra_config::Config,
  provider_id: &str,
  token: String,
  source_entry_id: Option<&str>,
  stored: Option<&super::auth::StoredCredentials>,
  existing_config: Option<&ProviderConfig>,
) -> ProviderConfig {
  let mut runtime_config = existing_config.cloned().unwrap_or_default();
  let mut headers = runtime_config.headers.clone();
  if let Some(source_entry_id) = source_entry_id {
    headers.insert(
      "x-cokra-connect-source".to_string(),
      source_entry_id.to_string(),
    );
  }
  let organization = stored.and_then(stored_openai_organization_id);
  let base_url = stored
    .and_then(stored_github_base_url)
    .or_else(|| existing_config.and_then(|config| config.base_url.clone()))
    .or_else(|| provider_base_url(config, provider_id));
  runtime_config.provider_id = provider_id.to_string();
  runtime_config.api_key = Some(token);
  runtime_config.base_url = base_url;
  runtime_config.organization =
    organization.or_else(|| existing_config.and_then(|config| config.organization.clone()));
  runtime_config.headers = headers;
  runtime_config
}

fn stored_openai_organization_id(stored: &super::auth::StoredCredentials) -> Option<String> {
  stored
    .metadata
    .get("organization_id")
    .and_then(serde_json::Value::as_str)
    .map(ToString::to_string)
    .or_else(|| match &stored.credentials {
      super::auth::Credentials::OAuth { account_id, .. } => account_id.clone(),
      _ => None,
    })
}

fn stored_github_base_url(stored: &super::auth::StoredCredentials) -> Option<String> {
  match &stored.credentials {
    super::auth::Credentials::OAuth {
      enterprise_url: Some(enterprise_url),
      ..
    } => {
      let normalized = enterprise_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
      if normalized.is_empty() {
        None
      } else {
        Some(format!("https://copilot-api.{normalized}"))
      }
    }
    _ => None,
  }
}

pub fn registration_token_for_stored(
  registration: RuntimeRegistrationKind,
  stored: &super::auth::StoredCredentials,
) -> Option<String> {
  match registration {
    RuntimeRegistrationKind::OpenAICodex => match &stored.credentials {
      super::auth::Credentials::OAuth { access_token, .. } => Some(access_token.clone()),
      _ => None,
    },
    RuntimeRegistrationKind::OpenAI => {
      let api_key = stored
        .metadata
        .get("api_key")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty());
      if let Some(api_key) = api_key {
        return Some(api_key.to_string());
      }

      match &stored.credentials {
        super::auth::Credentials::ApiKey { key } => Some(key.clone()),
        super::auth::Credentials::OAuth { access_token, .. } => Some(access_token.clone()),
        super::auth::Credentials::Bearer { token } => Some(token.clone()),
        super::auth::Credentials::DeviceCode { .. } => None,
      }
    }
    RuntimeRegistrationKind::GoogleGeminiCli | RuntimeRegistrationKind::GoogleAntigravity => {
      let access_token = match &stored.credentials {
        super::auth::Credentials::OAuth { access_token, .. } => access_token.clone(),
        _ => return None,
      };
      let project_id = stored
        .metadata
        .get("project_id")
        .and_then(serde_json::Value::as_str)?;
      Some(
        serde_json::json!({
          "token": access_token,
          "projectId": project_id,
        })
        .to_string(),
      )
    }
    RuntimeRegistrationKind::None => None,
    _ => match &stored.credentials {
      super::auth::Credentials::ApiKey { key } => Some(key.clone()),
      super::auth::Credentials::OAuth { access_token, .. } => Some(access_token.clone()),
      super::auth::Credentials::Bearer { token } => Some(token.clone()),
      super::auth::Credentials::DeviceCode { .. } => None,
    },
  }
}

// =============================================================================
// Helper functions for providers
// =============================================================================

/// Create a default HTTP client for providers
pub fn create_client(timeout: Option<u64>) -> Client {
  let timeout = std::time::Duration::from_secs(timeout.unwrap_or(120));

  Client::builder()
    .timeout(timeout)
    .build()
    .unwrap_or_else(|_| Client::new())
}

/// Build OpenAI-compatible request body.
///
/// Optional fields (`temperature`, `max_tokens`, `tools`, etc.) are omitted
/// entirely when `None` instead of being serialized as JSON `null`. Some
/// providers (notably OpenRouter) reject requests that contain explicit
/// `null` values for these fields.
pub fn build_openai_request(request: ChatRequest, model: &str) -> serde_json::Value {
  let messages: Vec<Value> = request
    .messages
    .iter()
    .map(openai_compatible_message)
    .collect();

  let mut body = serde_json::Map::new();
  body.insert("model".to_string(), json!(model));
  body.insert("messages".to_string(), json!(messages));
  body.insert("stream".to_string(), json!(request.stream));

  if let Some(temperature) = request.temperature {
    body.insert("temperature".to_string(), json!(temperature));
  }
  if let Some(max_tokens) = request.max_tokens {
    body.insert("max_tokens".to_string(), json!(max_tokens));
  }
  if let Some(tools) = &request.tools {
    body.insert("tools".to_string(), json!(tools));
  }
  if let Some(tool_choice) = &request.tool_choice {
    body.insert("tool_choice".to_string(), json!(tool_choice));
  }
  if let Some(stop) = &request.stop {
    body.insert("stop".to_string(), json!(stop));
  }
  if let Some(presence_penalty) = request.presence_penalty {
    body.insert("presence_penalty".to_string(), json!(presence_penalty));
  }
  if let Some(frequency_penalty) = request.frequency_penalty {
    body.insert("frequency_penalty".to_string(), json!(frequency_penalty));
  }
  if let Some(top_p) = request.top_p {
    body.insert("top_p".to_string(), json!(top_p));
  }

  Value::Object(body)
}

/// Log a summary of the outbound request for debugging tool-call issues.
/// Emits tool names and tool_choice at DEBUG level to avoid overwhelming output.
fn log_request_summary(body: &Value) {
  if tracing::enabled!(tracing::Level::DEBUG) {
    let tool_choice = body.get("tool_choice");
    let tool_names: Vec<&str> = body
      .get("tools")
      .and_then(|t| t.as_array())
      .map(|arr| {
        arr
          .iter()
          .filter_map(|t| {
            t.get("function")
              .and_then(|f| f.get("name"))
              .and_then(|n| n.as_str())
          })
          .collect()
      })
      .unwrap_or_default();
    let msg_count = body
      .get("messages")
      .and_then(|m| m.as_array())
      .map(|a| a.len())
      .unwrap_or(0);
    tracing::debug!(
      tool_choice = ?tool_choice,
      tools = ?tool_names,
      messages = msg_count,
      "outbound chat request summary"
    );
  }
}

fn openai_compatible_message(message: &Message) -> Value {
  match message {
    Message::System(content) => json!({
      "role": "system",
      "content": content,
    }),
    Message::User(content) => json!({
      "role": "user",
      "content": content,
    }),
    Message::Assistant {
      content,
      tool_calls,
    } => {
      let mut out = json!({
        "role": "assistant",
        "content": content,
      });
      if let Some(calls) = tool_calls {
        out["tool_calls"] = json!(calls);
      }
      out
    }
    Message::Tool {
      tool_call_id,
      content,
    } => json!({
      "role": "tool",
      "tool_call_id": tool_call_id,
      "content": content,
    }),
  }
}

/// Parse OpenAI-compatible response
pub fn parse_openai_response(body: &str) -> Result<ChatResponse> {
  Ok(serde_json::from_str(body)?)
}

// =============================================================================
// Re-export stream type
// =============================================================================

use futures::Stream;
use futures::StreamExt;
use std::pin::Pin;

/// Helper function for creating a stream from a response
pub fn create_response_stream(
  response: reqwest::Response,
) -> Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>> {
  create_response_stream_with_usage_parser(response, Box::new(OpenAIUsageParser::default()))
}

/// Create a unified stream parser with a custom usage parser.
pub fn create_response_stream_with_usage_parser(
  response: reqwest::Response,
  usage_parser: Box<dyn UsageParser>,
) -> Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>> {
  Box::pin(async_stream::stream! {
      let status = response.status();
      let mut stream = response.bytes_stream();
      let mut processor = StreamingProcessor::new(StreamingConfig {
          separator: "\n\n",
          usage_parser,
          binary_decoder: None,
      });

      if !status.is_success() {
          let mut body = String::new();
          while let Some(item) = stream.next().await {
              match item {
                  Ok(bytes) => {
                      body.push_str(&String::from_utf8_lossy(&bytes));
                  }
                  Err(err) => {
                      yield Err(ModelError::StreamError(err.to_string()));
                      return;
                  }
              }
          }
          yield Err(ModelError::ApiError(format!("HTTP {}: {}", status, body)));
          return;
      }

      while let Some(item) = stream.next().await {
          match item {
              Ok(bytes) => {
                  let text = String::from_utf8_lossy(&bytes).replace("\r\n\r\n", "\n\n");
                  for event in processor.push_text(&text) {
                      if let Some(chunk) = event.chunk {
                          yield Ok(chunk);
                      }
                  }
              }
              Err(e) => {
                  yield Err(ModelError::StreamError(e.to_string()));
              }
          }
      }

      for event in processor.finish() {
          if let Some(chunk) = event.chunk {
              yield Ok(chunk);
          }
      }
  })
}

#[cfg(test)]
mod tests {
  use super::build_openai_request;
  use super::registration_token_for_stored;
  use crate::model::auth::Credentials;
  use crate::model::auth::StoredCredentials;
  use crate::model::connect_catalog::RuntimeRegistrationKind;
  use crate::model::types::ChatRequest;
  use crate::model::types::Message;
  use serde_json::json;

  #[test]
  fn build_openai_request_uses_lowercase_roles() {
    let request = ChatRequest {
      model: "openrouter/openai/gpt-5.1-codex-mini".to_string(),
      messages: vec![
        Message::System("sys".to_string()),
        Message::User("hi".to_string()),
        Message::Assistant {
          content: Some("ok".to_string()),
          tool_calls: None,
        },
        Message::Tool {
          tool_call_id: "call_1".to_string(),
          content: "tool out".to_string(),
        },
      ],
      stream: false,
      ..Default::default()
    };

    let payload = build_openai_request(request, "openrouter/openai/gpt-5.1-codex-mini");
    let messages = payload["messages"]
      .as_array()
      .expect("messages should be an array");

    let roles: Vec<&str> = messages
      .iter()
      .map(|m| m["role"].as_str().unwrap_or(""))
      .collect();
    assert_eq!(roles, vec!["system", "user", "assistant", "tool"]);
  }

  #[test]
  fn build_openai_request_keeps_tool_fields() {
    let request = ChatRequest {
      model: "openrouter/openai/gpt-5.1-codex-mini".to_string(),
      messages: vec![Message::Tool {
        tool_call_id: "call_1".to_string(),
        content: "done".to_string(),
      }],
      stream: false,
      ..Default::default()
    };

    let payload = build_openai_request(request, "openrouter/openai/gpt-5.1-codex-mini");
    assert_eq!(
      payload["messages"][0],
      json!({
        "role": "tool",
        "tool_call_id": "call_1",
        "content": "done"
      })
    );
  }

  #[test]
  fn registration_token_for_openai_prefers_exchanged_api_key() {
    let mut stored = StoredCredentials::new(
      "openai-codex",
      Credentials::OAuth {
        access_token: "oauth-access".to_string(),
        refresh_token: "oauth-refresh".to_string(),
        expires_at: 1,
        account_id: Some("acc_123".to_string()),
        enterprise_url: None,
      },
    );
    stored.metadata = json!({
      "api_key": "sk-chatgpt-exchanged",
    });

    let token = registration_token_for_stored(RuntimeRegistrationKind::OpenAI, &stored);
    assert_eq!(token.as_deref(), Some("sk-chatgpt-exchanged"));
  }

  #[test]
  fn registration_token_for_google_cloud_code_matches_pi_mono_shape() {
    let mut stored = StoredCredentials::new(
      "google-antigravity",
      Credentials::OAuth {
        access_token: "oauth-access".to_string(),
        refresh_token: "oauth-refresh".to_string(),
        expires_at: 1_777_777_777,
        account_id: None,
        enterprise_url: None,
      },
    );
    stored.metadata = json!({
      "project_id": "proj-123",
    });

    let token = registration_token_for_stored(RuntimeRegistrationKind::GoogleAntigravity, &stored)
      .expect("google cloud code registration token");
    let payload: serde_json::Value =
      serde_json::from_str(&token).expect("google cloud code registration payload");

    assert_eq!(payload["token"], json!("oauth-access"));
    assert_eq!(payload["projectId"], json!("proj-123"));
    assert_eq!(payload.as_object().map(|value| value.len()), Some(2));
  }

  #[test]
  fn registration_token_for_github_copilot_oauth_returns_access_token() {
    let stored = StoredCredentials::new(
      "github-copilot",
      Credentials::OAuth {
        access_token: "ghu_copilot_token".to_string(),
        refresh_token: "ghu_github_token".to_string(),
        expires_at: 1_777_777_777,
        account_id: None,
        enterprise_url: None,
      },
    );

    let token = registration_token_for_stored(RuntimeRegistrationKind::GitHubCopilot, &stored);
    assert_eq!(token.as_deref(), Some("ghu_copilot_token"));
  }

  #[test]
  fn registration_token_for_anthropic_oauth_returns_access_token() {
    let stored = StoredCredentials::new(
      "anthropic-oauth",
      Credentials::OAuth {
        access_token: "claude_access_token".to_string(),
        refresh_token: "claude_refresh_token".to_string(),
        expires_at: 1_777_777_777,
        account_id: None,
        enterprise_url: None,
      },
    );

    let token = registration_token_for_stored(RuntimeRegistrationKind::Anthropic, &stored);
    assert_eq!(token.as_deref(), Some("claude_access_token"));
  }

  #[test]
  fn registration_token_for_openrouter_api_key_returns_key() {
    let stored = StoredCredentials::new(
      "openrouter",
      Credentials::ApiKey {
        key: "sk-or-v1-abc123".to_string(),
      },
    );

    let token = registration_token_for_stored(RuntimeRegistrationKind::OpenRouter, &stored);
    assert_eq!(token.as_deref(), Some("sk-or-v1-abc123"));
  }

  #[test]
  fn registration_token_for_none_returns_none_for_any_credentials() {
    let stored = StoredCredentials::new(
      "no-runtime",
      Credentials::ApiKey {
        key: "sk-some-key".to_string(),
      },
    );

    let token = registration_token_for_stored(RuntimeRegistrationKind::None, &stored);
    assert!(token.is_none());
  }

  #[test]
  fn registration_token_for_github_copilot_device_code_returns_none() {
    let stored = StoredCredentials::new(
      "github-copilot",
      Credentials::DeviceCode {
        device_code: "device_abc".to_string(),
        user_code: "USER-CODE".to_string(),
        verification_url: "https://github.com/login/device".to_string(),
        expires_in: 900,
        interval: 5,
      },
    );

    let token = registration_token_for_stored(RuntimeRegistrationKind::GitHubCopilot, &stored);
    assert!(token.is_none());
  }
}
