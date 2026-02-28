//! Model provider implementations
//!
//! Individual implementations for each supported LLM provider

use reqwest::Client;
use serde_json::json;

use super::error::{ModelError, Result};
use super::registry::ProviderRegistry;
use super::streaming::{OpenAIUsageParser, StreamingConfig, StreamingProcessor, UsageParser};
use super::types::{ChatRequest, ChatResponse, Chunk, ProviderConfig};

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

/// Build OpenAI-compatible request body
pub fn build_openai_request(request: ChatRequest, model: &str) -> serde_json::Value {
  json!({
      "model": model,
      "messages": request.messages,
      "temperature": request.temperature,
      "max_tokens": request.max_tokens,
      "stream": request.stream,
      "tools": request.tools,
      "stop": request.stop,
      "presence_penalty": request.presence_penalty,
      "frequency_penalty": request.frequency_penalty,
      "top_p": request.top_p,
  })
}

/// Parse OpenAI-compatible response
pub fn parse_openai_response(body: &str) -> Result<ChatResponse> {
  Ok(serde_json::from_str(body)?)
}

// =============================================================================
// Re-export stream type
// =============================================================================

use futures::{Stream, StreamExt};
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
