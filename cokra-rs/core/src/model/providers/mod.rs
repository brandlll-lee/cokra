//! Model provider implementations
//!
//! Individual implementations for each supported LLM provider

use reqwest::Client;
use serde_json::Value;
use serde_json::json;

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
pub mod github;
pub mod google;
pub mod lmstudio;
pub mod ollama;
pub mod openai;
pub mod openrouter;

pub use anthropic::AnthropicProvider;
pub use github::GitHubCopilotProvider;
pub use google::GoogleProvider;
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
}
