//! Anthropic Provider
//!
//! Supports Claude models including Claude 3.5 Sonnet, Claude 3 Opus, etc.

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

use super::super::error::{ModelError, Result};
use super::super::provider::ModelProvider;
use super::super::streaming::AnthropicUsageParser;
use super::super::types::{
  ChatRequest, ChatResponse, Chunk, ListModelsResponse, Message, ProviderConfig,
};
use super::{create_client, create_response_stream_with_usage_parser};

/// Anthropic provider
pub struct AnthropicProvider {
  client: Client,
  config: ProviderConfig,
  api_key: String,
  base_url: String,
  version: String,
  beta_headers: Vec<String>,
}

impl AnthropicProvider {
  /// Create a new Anthropic provider
  pub fn new(api_key: String, config: ProviderConfig) -> Self {
    let base_url = config
      .base_url
      .clone()
      .unwrap_or_else(|| "https://api.anthropic.com".to_string());

    let client = create_client(config.timeout);

    Self {
      client,
      config,
      api_key,
      base_url,
      version: "2023-06-01".to_string(),
      beta_headers: vec![
        "prompt-caching-2024-01-09".to_string(),
        "token-counting-2024-01-09".to_string(),
      ],
    }
  }

  /// Get the API endpoint URL
  fn endpoint(&self, path: &str) -> String {
    format!("{}/v1/{}", self.base_url.trim_end_matches('/'), path)
  }

  /// Convert message to Anthropic format
  fn convert_message(msg: &Message) -> AnthropicMessage {
    match msg {
      Message::System(content) => AnthropicMessage {
        role: "user".to_string(),
        content: vec![AnthropicContent::Text {
          text: format!("<system_prompt>{}</system_prompt>", content),
          type_: "text".to_string(),
        }],
      },
      Message::User(content) => AnthropicMessage {
        role: "user".to_string(),
        content: vec![AnthropicContent::Text {
          text: content.clone(),
          type_: "text".to_string(),
        }],
      },
      Message::Assistant {
        content,
        tool_calls,
      } => {
        let mut parts = Vec::new();

        if let Some(text) = content {
          parts.push(AnthropicContent::Text {
            text: text.clone(),
            type_: "text".to_string(),
          });
        }

        if let Some(calls) = tool_calls {
          for call in calls {
            parts.push(AnthropicContent::ToolUse {
              id: call.id.clone(),
              name: call.function.name.clone(),
              input: call.function.arguments.clone(),
              type_: "tool_use".to_string(),
            });
          }
        }

        AnthropicMessage {
          role: "assistant".to_string(),
          content: parts,
        }
      }
      Message::Tool {
        tool_call_id,
        content,
      } => AnthropicMessage {
        role: "user".to_string(),
        content: vec![AnthropicContent::ToolResult {
          tool_use_id: tool_call_id.clone(),
          content: content.clone(),
          type_: "tool_result".to_string(),
        }],
      },
    }
  }
}

/// Default models for Anthropic
pub const ANTHROPIC_MODELS: &[&str] = &[
  // Claude 4 Sonnet (latest)
  "claude-sonnet-4-20250514",
  // Claude 3.5 Sonnet
  "claude-3-5-sonnet-20241022",
  "claude-3-5-sonnet-20240620",
  // Claude 3 Opus
  "claude-3-opus-20240229",
  // Claude 3 Haiku
  "claude-3-haiku-20240307",
  "claude-3-5-haiku-20241022",
];

// Anthropic-specific types

#[derive(Debug, Serialize)]
struct AnthropicRequest {
  model: String,
  messages: Vec<AnthropicMessage>,
  max_tokens: u32,
  #[serde(skip_serializing_if = "Option::is_none")]
  temperature: Option<f32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  top_p: Option<f32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  top_k: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  system: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  tools: Option<Vec<AnthropicTool>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  stream: Option<bool>,
}

#[derive(Debug, Serialize, Clone)]
struct AnthropicMessage {
  role: String,
  content: Vec<AnthropicContent>,
}

#[derive(Debug, Serialize, Clone, Deserialize)]
#[serde(untagged)]
enum AnthropicContent {
  #[serde(rename = "text")]
  Text {
    text: String,
    #[serde(rename = "type")]
    type_: String,
  },
  #[serde(rename = "tool_use")]
  ToolUse {
    id: String,
    name: String,
    input: String,
    #[serde(rename = "type")]
    type_: String,
  },
  #[serde(rename = "tool_result")]
  ToolResult {
    tool_use_id: String,
    content: String,
    #[serde(rename = "type")]
    type_: String,
  },
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
  name: String,
  description: String,
  input_schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
  id: String,
  role: String,
  content: Vec<AnthropicContent>,
  stop_reason: Option<String>,
  usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
  input_tokens: u32,
  output_tokens: u32,
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
  fn provider_id(&self) -> &'static str {
    "anthropic"
  }

  fn provider_name(&self) -> &'static str {
    "Anthropic"
  }

  fn required_env_vars(&self) -> Vec<&'static str> {
    vec!["ANTHROPIC_API_KEY"]
  }

  fn default_models(&self) -> Vec<&'static str> {
    ANTHROPIC_MODELS.to_vec()
  }

  async fn chat_completion(&self, request: ChatRequest) -> Result<ChatResponse> {
    let url = self.endpoint("messages");

    // Convert messages
    let messages: Vec<AnthropicMessage> =
      request.messages.iter().map(Self::convert_message).collect();

    // Extract system message
    let system = request.messages.iter().find_map(|m| match m {
      Message::System(s) => Some(s.clone()),
      _ => None,
    });

    let anthropic_request = AnthropicRequest {
      model: request.model.clone(),
      messages,
      max_tokens: request.max_tokens.unwrap_or(4096),
      temperature: request.temperature,
      top_p: request.top_p,
      top_k: None,
      system,
      tools: request.tools.map(|t| {
        t.into_iter()
          .filter_map(|tool| tool.function)
          .map(|f| AnthropicTool {
            name: f.name,
            description: f.description,
            input_schema: f.parameters,
          })
          .collect()
      }),
      stream: Some(false),
    };

    let mut req_builder = self
      .client
      .post(&url)
      .header("x-api-key", &self.api_key)
      .header("anthropic-version", &self.version)
      .header("Content-Type", "application/json");

    // Add beta headers for extended features
    for beta in &self.beta_headers {
      req_builder = req_builder.header("anthropic-beta", beta);
    }

    let response = req_builder
      .json(&anthropic_request)
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(ModelError::ApiError(format!("HTTP {}: {}", status, body)));
    }

    let anthropic_response: AnthropicResponse = response.json().await?;

    // Convert to ChatResponse
    Ok(convert_anthropic_response(
      anthropic_response,
      &request.model,
    ))
  }

  async fn chat_completion_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>> {
    let url = self.endpoint("messages");

    let messages: Vec<AnthropicMessage> =
      request.messages.iter().map(Self::convert_message).collect();

    let anthropic_request = AnthropicRequest {
      model: request.model.clone(),
      messages,
      max_tokens: request.max_tokens.unwrap_or(4096),
      temperature: request.temperature,
      top_p: request.top_p,
      top_k: None,
      stream: Some(true),
      ..Default::default()
    };

    let mut req_builder = self
      .client
      .post(&url)
      .header("x-api-key", &self.api_key)
      .header("anthropic-version", &self.version)
      .header("Content-Type", "application/json");

    for beta in &self.beta_headers {
      req_builder = req_builder.header("anthropic-beta", beta);
    }

    let response = req_builder
      .json(&anthropic_request)
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    Ok(create_response_stream_with_usage_parser(
      response,
      Box::new(AnthropicUsageParser::default()),
    ))
  }

  async fn list_models(&self) -> Result<ListModelsResponse> {
    // Anthropic doesn't have a models endpoint
    // Return static list of known models
    Ok(ListModelsResponse {
      object_type: "list".to_string(),
      data: ANTHROPIC_MODELS
        .iter()
        .map(|&id| crate::model::types::ModelInfo {
          id: id.to_string(),
          object_type: "model".to_string(),
          created: 1704067200, // Approximate
          owned_by: Some("anthropic".to_string()),
        })
        .collect(),
    })
  }

  async fn validate_auth(&self) -> Result<()> {
    // Try a simple request to validate
    let url = self.endpoint("messages");

    let request = AnthropicRequest {
      model: "claude-3-haiku-20240307".to_string(),
      messages: vec![AnthropicMessage {
        role: "user".to_string(),
        content: vec![AnthropicContent::Text {
          text: "Hi".to_string(),
          type_: "text".to_string(),
        }],
      }],
      max_tokens: 10,
      ..Default::default()
    };

    let response = self
      .client
      .post(&url)
      .header("x-api-key", &self.api_key)
      .header("anthropic-version", &self.version)
      .json(&request)
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    if response.status().is_success() {
      Ok(())
    } else {
      Err(ModelError::AuthError("Invalid API key".to_string()))
    }
  }

  fn client(&self) -> &Client {
    &self.client
  }

  fn config(&self) -> &ProviderConfig {
    &self.config
  }
}

impl Default for AnthropicRequest {
  fn default() -> Self {
    Self {
      model: String::new(),
      messages: Vec::new(),
      max_tokens: 4096,
      temperature: None,
      top_p: None,
      top_k: None,
      system: None,
      tools: None,
      stream: None,
    }
  }
}

/// Convert Anthropic response to ChatResponse
fn convert_anthropic_response(resp: AnthropicResponse, model: &str) -> ChatResponse {
  use crate::model::types::{Choice, ChoiceMessage, ToolCall, ToolCallFunction, Usage};

  let content = resp
    .content
    .iter()
    .filter_map(|c| match c {
      AnthropicContent::Text { text, .. } => Some(text.clone()),
      _ => None,
    })
    .collect::<Vec<_>>()
    .join("");

  let tool_calls: Vec<ToolCall> = resp
    .content
    .iter()
    .filter_map(|c| match c {
      AnthropicContent::ToolUse {
        id, name, input, ..
      } => Some(ToolCall {
        id: id.clone(),
        call_type: "function".to_string(),
        function: ToolCallFunction {
          name: name.clone(),
          arguments: input.clone(),
        },
      }),
      _ => None,
    })
    .collect();

  ChatResponse {
    id: resp.id,
    object_type: "chat.completion".to_string(),
    created: chrono::Utc::now().timestamp() as u64,
    model: model.to_string(),
    choices: vec![Choice {
      index: 0,
      message: ChoiceMessage {
        role: "assistant".to_string(),
        content: Some(content),
        tool_calls: if tool_calls.is_empty() {
          None
        } else {
          Some(tool_calls)
        },
      },
      finish_reason: resp.stop_reason,
    }],
    usage: Usage {
      input_tokens: resp.usage.input_tokens,
      output_tokens: resp.usage.output_tokens,
      total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
    },
    extra: Default::default(),
  }
}
