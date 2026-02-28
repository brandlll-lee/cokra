//! LM Studio Provider
//!
//! Support for running local models via LM Studio (OpenAI-compatible API)

use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use std::pin::Pin;

use super::super::error::{ModelError, Result};
use super::super::provider::ModelProvider;
use super::super::types::{ChatRequest, ChatResponse, Chunk, ListModelsResponse, ProviderConfig};
use super::create_client;
use super::openai::OpenAIProvider;

/// LM Studio provider (OpenAI-compatible local models)
pub struct LMStudioProvider {
  base_url: String,
  client: Client,
  config: ProviderConfig,
}

impl LMStudioProvider {
  /// Create a new LM Studio provider
  pub fn new(base_url: Option<String>) -> Self {
    let base_url = base_url.unwrap_or_else(|| "http://localhost:1234/v1".to_string());
    let client = create_client(Some(600)); // 10 minute timeout for local models

    let config = ProviderConfig {
      provider_id: "lmstudio".to_string(),
      base_url: Some(base_url.clone()),
      timeout: Some(600),
      ..Default::default()
    };

    Self {
      base_url,
      client,
      config,
    }
  }

  /// Get the API endpoint URL
  fn endpoint(&self, path: &str) -> String {
    format!("{}/{}", self.base_url.trim_end_matches('/'), path)
  }

  /// List available models
  pub async fn list_available_models(&self) -> Result<Vec<LMStudioModel>> {
    let url = self.endpoint("models");

    let response = self
      .client
      .get(&url)
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    if !response.status().is_success() {
      return Err(ModelError::ApiError("LM Studio not reachable".to_string()));
    }

    #[derive(serde::Deserialize)]
    struct ModelsResponse {
      data: Vec<LMStudioModel>,
    }

    let resp: ModelsResponse = response.json().await?;
    Ok(resp.data)
  }
}

/// LM Studio model information
#[derive(Debug, Clone, serde::Deserialize)]
pub struct LMStudioModel {
  pub id: String,
  pub object: String,
  pub created: u64,
  pub owned_by: String,
}

/// Default model name for LM Studio
pub const LMSTUDIO_DEFAULT_MODEL: &str = "local-model";

#[async_trait]
impl ModelProvider for LMStudioProvider {
  fn provider_id(&self) -> &'static str {
    "lmstudio"
  }

  fn provider_name(&self) -> &'static str {
    "LM Studio"
  }

  fn required_env_vars(&self) -> Vec<&'static str> {
    vec![]
  }

  fn default_models(&self) -> Vec<&'static str> {
    vec![LMSTUDIO_DEFAULT_MODEL]
  }

  async fn chat_completion(&self, request: ChatRequest) -> Result<ChatResponse> {
    // LM Studio uses OpenAI-compatible API
    let url = self.endpoint("chat/completions");

    let model = request.model.clone();
    let body = super::super::providers::build_openai_request(request, &model);

    let response = self
      .client
      .post(&url)
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
    super::super::providers::parse_openai_response(&response_text)
  }

  async fn chat_completion_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>> {
    let url = self.endpoint("chat/completions");

    let model = request.model.clone();
    let mut body = super::super::providers::build_openai_request(request, &model);
    body["stream"] = serde_json::json!(true);

    let response = self
      .client
      .post(&url)
      .header("Content-Type", "application/json")
      .json(&body)
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    Ok(super::super::providers::create_response_stream(response))
  }

  async fn list_models(&self) -> Result<ListModelsResponse> {
    let models = self.list_available_models().await?;

    Ok(ListModelsResponse {
      object_type: "list".to_string(),
      data: models
        .into_iter()
        .map(|m| crate::model::types::ModelInfo {
          id: m.id,
          object_type: m.object,
          created: m.created,
          owned_by: Some(m.owned_by),
        })
        .collect(),
    })
  }

  async fn validate_auth(&self) -> Result<()> {
    // LM Studio doesn't use authentication
    // Just check if the server is reachable
    let url = self.endpoint("models");

    let response = self
      .client
      .get(&url)
      .send()
      .await
      .map_err(|e| ModelError::NetworkError(e))?;

    if response.status().is_success() {
      Ok(())
    } else {
      Err(ModelError::ApiError("LM Studio not reachable".to_string()))
    }
  }

  fn client(&self) -> &Client {
    &self.client
  }

  fn config(&self) -> &ProviderConfig {
    &self.config
  }
}
