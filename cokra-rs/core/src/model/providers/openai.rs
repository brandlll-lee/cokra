//! OpenAI Provider
//!
//! Supports all OpenAI models including GPT-4, GPT-3.5, and O-series

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use std::pin::Pin;

use super::super::error::{ModelError, Result};
use super::super::provider::ModelProvider;
use super::super::types::{
  ChatRequest, ChatResponse, Chunk, ListModelsResponse, ModelInfo, ProviderConfig,
};
use super::{build_openai_request, create_client, create_response_stream, parse_openai_response};

/// OpenAI provider
pub struct OpenAIProvider {
  client: Client,
  config: ProviderConfig,
  api_key: String,
  base_url: String,
  organization: Option<String>,
}

impl OpenAIProvider {
  /// Create a new OpenAI provider
  pub fn new(api_key: String, config: ProviderConfig) -> Self {
    let base_url = config
      .base_url
      .clone()
      .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let organization = config.organization.clone();

    let client = create_client(config.timeout);

    Self {
      client,
      config,
      api_key,
      base_url,
      organization,
    }
  }

  /// Get the API endpoint URL
  fn endpoint(&self, path: &str) -> String {
    format!("{}/{}", self.base_url.trim_end_matches('/'), path)
  }

  /// Build authorization header
  fn auth_header(&self) -> String {
    format!("Bearer {}", self.api_key)
  }
}

/// Default models for OpenAI
pub const OPENAI_MODELS: &[&str] = &[
  // GPT-4O series
  "gpt-4o",
  "gpt-4o-mini",
  // GPT-4 Turbo
  "gpt-4-turbo",
  "gpt-4-turbo-preview",
  // GPT-4
  "gpt-4",
  "gpt-4-32k",
  // GPT-3.5
  "gpt-3.5-turbo",
  "gpt-3.5-turbo-16k",
  // O-series (reasoning)
  "o1",
  "o1-mini",
  "o1-preview",
  "o3",
  "o3-mini",
];

#[async_trait]
impl ModelProvider for OpenAIProvider {
  fn provider_id(&self) -> &'static str {
    "openai"
  }

  fn provider_name(&self) -> &'static str {
    "OpenAI"
  }

  fn required_env_vars(&self) -> Vec<&'static str> {
    vec!["OPENAI_API_KEY"]
  }

  fn default_models(&self) -> Vec<&'static str> {
    OPENAI_MODELS.to_vec()
  }

  async fn chat_completion(&self, request: ChatRequest) -> Result<ChatResponse> {
    let url = self.endpoint("chat/completions");

    let model = request.model.clone();
    let body = build_openai_request(request, &model);

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

    let response_text = response.text().await?;
    parse_openai_response(&response_text)
  }

  async fn chat_completion_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>> {
    let url = self.endpoint("chat/completions");

    let model = request.model.clone();
    let mut body = build_openai_request(request, &model);
    body["stream"] = serde_json::json!(true);

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
    let url = self.endpoint("models");

    let response = self
      .client
      .get(&url)
      .header("Authorization", self.auth_header())
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    if !response.status().is_success() {
      return Err(ModelError::AuthError("Failed to list models".to_string()));
    }

    #[derive(Deserialize)]
    struct OpenAIModelsResponse {
      data: Vec<OpenAIModel>,
      object: String,
    }

    #[derive(Deserialize)]
    struct OpenAIModel {
      id: String,
      object: String,
      created: u64,
      owned_by: String,
    }

    let openai_response: OpenAIModelsResponse = response.json().await?;

    Ok(ListModelsResponse {
      object_type: openai_response.object,
      data: openai_response
        .data
        .into_iter()
        .map(|m| ModelInfo {
          id: m.id,
          object_type: m.object,
          created: m.created,
          owned_by: Some(m.owned_by),
        })
        .collect(),
    })
  }

  async fn validate_auth(&self) -> Result<()> {
    let url = self.endpoint("models");

    let response = self
      .client
      .get(&url)
      .header("Authorization", self.auth_header())
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_openai_models() {
    assert_eq!(OPENAI_MODELS.len(), 13);
    assert!(OPENAI_MODELS.contains(&"gpt-4o"));
    assert!(OPENAI_MODELS.contains(&"o1"));
  }
}
