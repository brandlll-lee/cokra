//! GitHub Copilot Provider
//!
//! Support for GitHub Copilot models (uses OAuth or token-based auth)

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

use super::super::error::{ModelError, Result};
use super::super::provider::ModelProvider;
use super::super::types::{ChatRequest, ChatResponse, Chunk, ListModelsResponse, ProviderConfig};
use super::{create_client, create_response_stream};

/// GitHub Copilot provider
pub struct GitHubCopilotProvider {
  client: Client,
  config: ProviderConfig,
  token: String,
  base_url: String,
  use_responses_api: bool,
}

impl GitHubCopilotProvider {
  /// Create a new GitHub Copilot provider with a token
  pub fn new(token: String, config: ProviderConfig) -> Self {
    let base_url = config
      .base_url
      .clone()
      .unwrap_or_else(|| "https://api.githubcopilot.com".to_string());

    let client = create_client(config.timeout);

    Self {
      client,
      config,
      token,
      base_url,
      use_responses_api: true, // Use Responses API by default
    }
  }

  /// Get the API endpoint URL
  fn endpoint(&self, path: &str) -> String {
    format!("{}/{}", self.base_url.trim_end_matches('/'), path)
  }

  /// Check if we should use the Responses API for this model
  fn should_use_responses_api(&self, model: &str) -> bool {
    // Use Responses API for o1 models
    model.starts_with("o1-") || model.contains("o1")
  }

  /// Build authorization header
  fn auth_header(&self) -> String {
    format!("Bearer {}", self.token)
  }
}

/// Default models for GitHub Copilot
pub const GITHUB_MODELS: &[&str] = &[
  // OpenAI models via Copilot
  "gpt-4o",
  "gpt-4o-mini",
  "gpt-4-turbo",
  "gpt-3.5-turbo",
  // O-series reasoning models
  "o1-2024-12-17",
  "o1-mini-2024-09-12",
  "o1-preview-2024-09-12",
  // Claude models (if available via Copilot)
  "claude-sonnet-4-20250514",
  "claude-3-5-sonnet-20241022",
];

// GitHub Copilot API types

#[derive(Debug, Serialize)]
struct CopilotRequest {
  messages: Vec<CopilotMessage>,
  model: String,
  stream: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  temperature: Option<f32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  max_tokens: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  top_p: Option<f32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  n: Option<u32>,
}

#[derive(Debug, Serialize, Clone)]
struct CopilotMessage {
  role: String,
  content: String,
}

#[derive(Debug, Deserialize)]
struct CopilotResponse {
  id: String,
  choices: Vec<CopilotChoice>,
  model: String,
  usage: CopilotUsage,
}

#[derive(Debug, Deserialize)]
struct CopilotChoice {
  message: CopilotResponseMessage,
  finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CopilotResponseMessage {
  role: String,
  content: Option<String>,
  tool_calls: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
struct CopilotUsage {
  prompt_tokens: u32,
  completion_tokens: u32,
  total_tokens: u32,
}

// Responses API types

#[derive(Debug, Serialize)]
struct ResponsesApiRequest {
  messages: Vec<CopilotMessage>,
  model: String,
  stream: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  temperature: Option<f32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  max_tokens: Option<u32>,
}

#[async_trait]
impl ModelProvider for GitHubCopilotProvider {
  fn provider_id(&self) -> &'static str {
    "github"
  }

  fn provider_name(&self) -> &'static str {
    "GitHub Copilot"
  }

  fn required_env_vars(&self) -> Vec<&'static str> {
    vec!["GITHUB_TOKEN", "GITHUB_COPILOT_TOKEN"]
  }

  fn default_models(&self) -> Vec<&'static str> {
    GITHUB_MODELS.to_vec()
  }

  async fn chat_completion(&self, request: ChatRequest) -> Result<ChatResponse> {
    // Choose API based on model
    let use_responses = self.should_use_responses_api(&request.model);

    let messages: Vec<CopilotMessage> = request
      .messages
      .iter()
      .map(|m| match m {
        crate::model::types::Message::System(s) => CopilotMessage {
          role: "system".to_string(),
          content: s.clone(),
        },
        crate::model::types::Message::User(s) => CopilotMessage {
          role: "user".to_string(),
          content: s.clone(),
        },
        crate::model::types::Message::Assistant { content, .. } => CopilotMessage {
          role: "assistant".to_string(),
          content: content.clone().unwrap_or_default(),
        },
        crate::model::types::Message::Tool {
          tool_call_id,
          content,
        } => CopilotMessage {
          role: "user".to_string(),
          content: format!("[Tool Result for {}]: {}", tool_call_id, content),
        },
      })
      .collect();

    if use_responses {
      self.chat_completion_responses(request, messages).await
    } else {
      self.chat_completion_chat(request, messages).await
    }
  }

  async fn chat_completion_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>> {
    let url = self.endpoint("chat/completions");

    let messages: Vec<CopilotMessage> = request
      .messages
      .iter()
      .map(|m| CopilotMessage {
        role: match m {
          crate::model::types::Message::System(_) => "system",
          crate::model::types::Message::User(_) => "user",
          crate::model::types::Message::Assistant { .. } => "assistant",
          crate::model::types::Message::Tool { .. } => "user",
        }
        .to_string(),
        content: m.text().unwrap_or("").to_string(),
      })
      .collect();

    let body = CopilotRequest {
      messages,
      model: request.model,
      stream: true,
      temperature: request.temperature,
      max_tokens: request.max_tokens,
      top_p: request.top_p,
      n: Some(1),
    };

    let response = self
      .client
      .post(&url)
      .header("Authorization", self.auth_header())
      .header("Content-Type", "application/json")
      .json(&body)
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    Ok(create_response_stream(response))
  }

  async fn list_models(&self) -> Result<ListModelsResponse> {
    // Return static list of known Copilot models
    Ok(ListModelsResponse {
      object_type: "list".to_string(),
      data: GITHUB_MODELS
        .iter()
        .map(|&id| crate::model::types::ModelInfo {
          id: id.to_string(),
          object_type: "model".to_string(),
          created: 1704067200,
          owned_by: Some("github".to_string()),
        })
        .collect(),
    })
  }

  async fn validate_auth(&self) -> Result<()> {
    // Try a simple request to validate
    let url = self.endpoint("chat/completions");

    let body = CopilotRequest {
      messages: vec![CopilotMessage {
        role: "user".to_string(),
        content: "Hi".to_string(),
      }],
      model: "gpt-4o-mini".to_string(),
      stream: false,
      temperature: Some(0.0),
      max_tokens: Some(10),
      top_p: None,
      n: None,
    };

    let response = self
      .client
      .post(&url)
      .header("Authorization", self.auth_header())
      .header("Content-Type", "application/json")
      .json(&body)
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    if response.status().is_success() {
      Ok(())
    } else if response.status().as_u16() == 401 {
      Err(ModelError::AuthError("Invalid GitHub token".to_string()))
    } else {
      Err(ModelError::AuthError("Authentication failed".to_string()))
    }
  }

  fn client(&self) -> &Client {
    &self.client
  }

  fn config(&self) -> &ProviderConfig {
    &self.config
  }
}

impl GitHubCopilotProvider {
  /// Chat completion using the Chat API
  async fn chat_completion_chat(
    &self,
    request: ChatRequest,
    messages: Vec<CopilotMessage>,
  ) -> Result<ChatResponse> {
    let url = self.endpoint("chat/completions");
    let model = request.model.clone();

    let body = CopilotRequest {
      messages,
      model: model.clone(),
      stream: false,
      temperature: request.temperature,
      max_tokens: request.max_tokens,
      top_p: request.top_p,
      n: Some(1),
    };

    let response = self
      .client
      .post(&url)
      .header("Authorization", self.auth_header())
      .header("Content-Type", "application/json")
      .json(&body)
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(ModelError::ApiError(format!("HTTP {}: {}", status, body)));
    }

    let copilot_response: CopilotResponse = response.json().await?;

    Ok(convert_copilot_response(copilot_response, &model))
  }

  /// Chat completion using the Responses API (for o1 models)
  async fn chat_completion_responses(
    &self,
    request: ChatRequest,
    messages: Vec<CopilotMessage>,
  ) -> Result<ChatResponse> {
    let url = format!("{}/responses", self.base_url);
    let model = request.model.clone();

    let body = ResponsesApiRequest {
      messages,
      model: model.clone(),
      stream: false,
      temperature: request.temperature,
      max_tokens: request.max_tokens,
    };

    let response = self
      .client
      .post(&url)
      .header("Authorization", self.auth_header())
      .header("Content-Type", "application/json")
      .json(&body)
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(ModelError::ApiError(format!("HTTP {}: {}", status, body)));
    }

    #[derive(Deserialize)]
    struct ResponsesApiResponse {
      id: String,
      body: ResponsesApiBody,
    }

    #[derive(Deserialize)]
    struct ResponsesApiBody {
      content: String,
    }

    let resp: ResponsesApiResponse = response.json().await?;

    Ok(ChatResponse {
      id: resp.id,
      object_type: "chat.completion".to_string(),
      created: chrono::Utc::now().timestamp() as u64,
      model,
      choices: vec![crate::model::types::Choice {
        index: 0,
        message: crate::model::types::ChoiceMessage {
          role: "assistant".to_string(),
          content: Some(resp.body.content),
          tool_calls: None,
        },
        finish_reason: Some("stop".to_string()),
      }],
      usage: crate::model::types::Usage::default(),
      extra: Default::default(),
    })
  }
}

/// Convert Copilot response to ChatResponse
fn convert_copilot_response(resp: CopilotResponse, model: &str) -> ChatResponse {
  ChatResponse {
    id: resp.id,
    object_type: "chat.completion".to_string(),
    created: chrono::Utc::now().timestamp() as u64,
    model: model.to_string(),
    choices: resp
      .choices
      .into_iter()
      .map(|c| crate::model::types::Choice {
        index: 0,
        message: crate::model::types::ChoiceMessage {
          role: c.message.role,
          content: c.message.content,
          tool_calls: c.message.tool_calls.and_then(|vals| {
            let parsed: Vec<crate::model::types::ToolCall> = vals
              .into_iter()
              .filter_map(|v| serde_json::from_value(v).ok())
              .collect();
            if parsed.is_empty() {
              None
            } else {
              Some(parsed)
            }
          }),
        },
        finish_reason: c.finish_reason,
      })
      .collect(),
    usage: crate::model::types::Usage {
      input_tokens: resp.usage.prompt_tokens,
      output_tokens: resp.usage.completion_tokens,
      total_tokens: resp.usage.total_tokens,
    },
    extra: Default::default(),
  }
}
