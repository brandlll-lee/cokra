//! Ollama Provider
//!
//! Support for running local models via Ollama

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use std::pin::Pin;

use super::super::error::{ModelError, Result};
use super::super::provider::ModelProvider;
use super::super::types::{ChatRequest, ChatResponse, Chunk, ListModelsResponse, ProviderConfig};
use super::{create_client, create_response_stream};

/// Ollama provider (local models)
pub struct OllamaProvider {
  client: Client,
  config: ProviderConfig,
  base_url: String,
}

impl OllamaProvider {
  /// Create a new Ollama provider
  pub fn new(base_url: Option<String>) -> Self {
    let base_url = base_url.unwrap_or_else(|| "http://localhost:11434".to_string());
    let client = create_client(Some(600)); // 10 minute timeout for local models

    let config = ProviderConfig {
      provider_id: "ollama".to_string(),
      base_url: Some(base_url.clone()),
      timeout: Some(600),
      ..Default::default()
    };

    Self {
      client,
      config,
      base_url,
    }
  }

  /// Get the API endpoint URL
  fn endpoint(&self, path: &str) -> String {
    format!("{}/api/{}", self.base_url.trim_end_matches('/'), path)
  }

  /// List available models
  pub async fn list_available_models(&self) -> Result<Vec<OllamaModel>> {
    let url = self.endpoint("tags");

    let response = self
      .client
      .get(&url)
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    #[derive(Deserialize)]
    struct TagsResponse {
      models: Vec<OllamaModel>,
    }

    let resp: TagsResponse = response.json().await?;
    Ok(resp.models)
  }

  /// Pull a model
  pub async fn pull_model(&self, model: &str) -> Result<()> {
    let url = self.endpoint("pull");

    #[derive(serde::Serialize)]
    struct PullRequest {
      name: String,
      #[serde(skip_serializing_if = "Option::is_none")]
      stream: Option<bool>,
    }

    let _ = self
      .client
      .post(&url)
      .json(&PullRequest {
        name: model.to_string(),
        stream: Some(false),
      })
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    Ok(())
  }
}

/// Ollama model information
#[derive(Debug, Clone, Deserialize)]
pub struct OllamaModel {
  pub name: String,
  pub modified_at: String,
  pub size: u64,
  pub digest: String,
}

/// Default models for Ollama
pub const OLLAMA_MODELS: &[&str] = &[
  "llama3",
  "llama3:70b",
  "codellama",
  "mistral",
  "neural-chat",
  "starcoder2",
  "gemma:2b",
  "gemma:7b",
];

#[async_trait]
impl ModelProvider for OllamaProvider {
  fn provider_id(&self) -> &'static str {
    "ollama"
  }

  fn provider_name(&self) -> &'static str {
    "Ollama"
  }

  fn required_env_vars(&self) -> Vec<&'static str> {
    vec![]
  }

  fn default_models(&self) -> Vec<&'static str> {
    OLLAMA_MODELS.to_vec()
  }

  async fn chat_completion(&self, request: ChatRequest) -> Result<ChatResponse> {
    let url = self.endpoint("chat");

    #[derive(serde::Serialize)]
    struct OllamaRequest {
      model: String,
      messages: Vec<OllamaMessage>,
      stream: bool,
      #[serde(skip_serializing_if = "Option::is_none")]
      options: Option<OllamaOptions>,
    }

    #[derive(serde::Serialize)]
    struct OllamaMessage {
      role: String,
      content: String,
    }

    #[derive(serde::Serialize, Default)]
    struct OllamaOptions {
      #[serde(skip_serializing_if = "Option::is_none")]
      temperature: Option<f32>,
      #[serde(skip_serializing_if = "Option::is_none")]
      num_predict: Option<u32>,
      #[serde(skip_serializing_if = "Option::is_none")]
      top_p: Option<f32>,
    }

    let messages: Vec<OllamaMessage> = request
      .messages
      .iter()
      .map(|m| match m {
        crate::model::types::Message::System(s) => OllamaMessage {
          role: "system".to_string(),
          content: s.clone(),
        },
        crate::model::types::Message::User(s) => OllamaMessage {
          role: "user".to_string(),
          content: s.clone(),
        },
        crate::model::types::Message::Assistant { content, .. } => OllamaMessage {
          role: "assistant".to_string(),
          content: content.clone().unwrap_or_default(),
        },
        crate::model::types::Message::Tool {
          tool_call_id,
          content,
        } => OllamaMessage {
          role: "user".to_string(),
          content: format!("[Tool Result for {}]: {}", tool_call_id, content),
        },
      })
      .collect();

    let ollama_request = OllamaRequest {
      model: request.model.clone(),
      messages,
      stream: false,
      options: Some(OllamaOptions {
        temperature: request.temperature,
        num_predict: request.max_tokens,
        top_p: request.top_p,
      }),
    };

    let response = self
      .client
      .post(&url)
      .json(&ollama_request)
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(ModelError::ApiError(format!("HTTP {}: {}", status, body)));
    }

    #[derive(Deserialize)]
    struct OllamaResponse {
      model: String,
      message: OllamaResponseMessage,
      done: bool,
      #[serde(default)]
      prompt_eval_count: u32,
      #[serde(default)]
      eval_count: u32,
    }

    #[derive(Deserialize)]
    struct OllamaResponseMessage {
      role: String,
      content: String,
    }

    let ollama_response: OllamaResponse = response.json().await?;

    Ok(ChatResponse {
      id: uuid::Uuid::new_v4().to_string(),
      object_type: "chat.completion".to_string(),
      created: chrono::Utc::now().timestamp() as u64,
      model: request.model,
      choices: vec![crate::model::types::Choice {
        index: 0,
        message: crate::model::types::ChoiceMessage {
          role: "assistant".to_string(),
          content: Some(ollama_response.message.content),
          tool_calls: None,
        },
        finish_reason: if ollama_response.done {
          Some("stop".to_string())
        } else {
          None
        },
      }],
      usage: crate::model::types::Usage {
        input_tokens: ollama_response.prompt_eval_count,
        output_tokens: ollama_response.eval_count,
        total_tokens: ollama_response.prompt_eval_count + ollama_response.eval_count,
      },
      extra: Default::default(),
    })
  }

  async fn chat_completion_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>> {
    let url = self.endpoint("chat");

    #[derive(serde::Serialize)]
    struct OllamaRequest {
      model: String,
      messages: Vec<serde_json::Value>,
      stream: bool,
    }

    let messages: Vec<serde_json::Value> = request
      .messages
      .iter()
      .map(|m| {
        serde_json::json!({
            "role": match m {
                crate::model::types::Message::System(_) => "system",
                crate::model::types::Message::User(_) => "user",
                crate::model::types::Message::Assistant { .. } => "assistant",
                crate::model::types::Message::Tool { .. } => "user",
            },
            "content": m.text().unwrap_or(""),
        })
      })
      .collect();

    let ollama_request = OllamaRequest {
      model: request.model.clone(),
      messages,
      stream: true,
    };

    let response = self
      .client
      .post(&url)
      .json(&ollama_request)
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    Ok(create_response_stream(response))
  }

  async fn list_models(&self) -> Result<ListModelsResponse> {
    let models = self.list_available_models().await?;

    Ok(ListModelsResponse {
      object_type: "list".to_string(),
      data: models
        .into_iter()
        .map(|m| crate::model::types::ModelInfo {
          id: m.name.clone(),
          object_type: "model".to_string(),
          created: m
            .modified_at
            .parse::<chrono::DateTime<chrono::Utc>>()
            .map(|dt| dt.timestamp() as u64)
            .unwrap_or(0),
          owned_by: Some("ollama".to_string()),
        })
        .collect(),
    })
  }

  async fn validate_auth(&self) -> Result<()> {
    // Ollama doesn't use authentication
    // Just check if the server is reachable
    let url = self.endpoint("tags");

    let response = self
      .client
      .get(&url)
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    if response.status().is_success() {
      Ok(())
    } else {
      Err(ModelError::ApiError(
        "Ollama server not reachable".to_string(),
      ))
    }
  }

  fn client(&self) -> &Client {
    &self.client
  }

  fn config(&self) -> &ProviderConfig {
    &self.config
  }
}
