//! OpenRouter Provider
//!
//! OpenRouter exposes an OpenAI-compatible API surface with routing across many models.

use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use std::pin::Pin;

use super::super::error::{ModelError, Result};
use super::super::provider::ModelProvider;
use super::super::types::{
  ChatRequest, ChatResponse, Chunk, ListModelsResponse, ModelInfo, ProviderConfig,
};
use super::{build_openai_request, create_client, create_response_stream, parse_openai_response};

/// OpenRouter provider.
pub struct OpenRouterProvider {
  client: Client,
  config: ProviderConfig,
  api_key: String,
  base_url: String,
  site_url: Option<String>,
  site_name: Option<String>,
}

impl OpenRouterProvider {
  /// Creates a new OpenRouter provider.
  pub fn new(api_key: String, config: ProviderConfig) -> Self {
    let base_url = config
      .base_url
      .clone()
      .unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());

    let client = create_client(config.timeout);
    let site_url = std::env::var("OPENROUTER_SITE_URL").ok();
    let site_name = std::env::var("OPENROUTER_SITE_NAME").ok();

    Self {
      client,
      config,
      api_key,
      base_url,
      site_url,
      site_name,
    }
  }

  fn endpoint(&self, path: &str) -> String {
    format!("{}/{}", self.base_url.trim_end_matches('/'), path)
  }
}

/// Commonly used OpenRouter models.
pub const OPENROUTER_MODELS: &[&str] = &[
  "openai/gpt-4o",
  "openai/gpt-4-turbo",
  "openai/o1-preview",
  "anthropic/claude-sonnet-4",
  "anthropic/claude-3.5-sonnet",
  "anthropic/claude-3-opus",
  "google/gemini-pro-1.5",
  "google/gemini-2.0-flash-exp",
  "meta-llama/llama-3.1-405b-instruct",
  "meta-llama/llama-3.1-70b-instruct",
  "mistralai/mistral-large",
  "mistralai/codestral-latest",
  "x-ai/grok-beta",
];

#[async_trait]
impl ModelProvider for OpenRouterProvider {
  fn provider_id(&self) -> &'static str {
    "openrouter"
  }

  fn provider_name(&self) -> &'static str {
    "OpenRouter"
  }

  fn required_env_vars(&self) -> Vec<&'static str> {
    vec!["OPENROUTER_API_KEY"]
  }

  fn default_models(&self) -> Vec<&'static str> {
    OPENROUTER_MODELS.to_vec()
  }

  async fn chat_completion(&self, request: ChatRequest) -> Result<ChatResponse> {
    let url = self.endpoint("chat/completions");
    let model = request.model.clone();
    let mut body = build_openai_request(request, &model);

    if let Some(site_url) = &self.site_url {
      body["site_url"] = serde_json::json!(site_url);
    }
    if let Some(site_name) = &self.site_name {
      body["site_name"] = serde_json::json!(site_name);
    }
    body["usage"] = serde_json::json!({ "include": true });

    let response = self
      .client
      .post(&url)
      .header("Authorization", format!("Bearer {}", self.api_key))
      .header(
        "HTTP-Referer",
        self
          .site_url
          .clone()
          .unwrap_or_else(|| "https://cokra.ai".to_string()),
      )
      .header(
        "X-Title",
        self
          .site_name
          .clone()
          .unwrap_or_else(|| "Cokra".to_string()),
      )
      .header("Content-Type", "application/json")
      .json(&body)
      .send()
      .await
      .map_err(ModelError::NetworkError)?;

    if !response.status().is_success() {
      let status = response.status();
      let text = response.text().await.unwrap_or_default();
      return Err(ModelError::ApiError(format!("HTTP {}: {}", status, text)));
    }

    let text = response.text().await.map_err(ModelError::NetworkError)?;
    parse_openai_response(&text)
  }

  async fn chat_completion_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>> {
    let url = self.endpoint("chat/completions");
    let model = request.model.clone();
    let mut body = build_openai_request(request, &model);
    body["stream"] = serde_json::json!(true);
    body["usage"] = serde_json::json!({ "include": true });

    if let Some(site_url) = &self.site_url {
      body["site_url"] = serde_json::json!(site_url);
    }
    if let Some(site_name) = &self.site_name {
      body["site_name"] = serde_json::json!(site_name);
    }

    let response = self
      .client
      .post(&url)
      .header("Authorization", format!("Bearer {}", self.api_key))
      .header(
        "HTTP-Referer",
        self
          .site_url
          .clone()
          .unwrap_or_else(|| "https://cokra.ai".to_string()),
      )
      .header(
        "X-Title",
        self
          .site_name
          .clone()
          .unwrap_or_else(|| "Cokra".to_string()),
      )
      .header("Content-Type", "application/json")
      .json(&body)
      .send()
      .await
      .map_err(ModelError::NetworkError)?;

    Ok(create_response_stream(response))
  }

  async fn list_models(&self) -> Result<ListModelsResponse> {
    let url = self.endpoint("models");
    let response = self
      .client
      .get(&url)
      .header("Authorization", format!("Bearer {}", self.api_key))
      .send()
      .await
      .map_err(ModelError::NetworkError)?;

    if response.status().is_success() {
      let body = response.text().await.map_err(ModelError::NetworkError)?;
      let parsed = serde_json::from_str::<ListModelsResponse>(&body)
        .map_err(|e| ModelError::InvalidResponse(format!("failed to parse models: {e}")));
      if let Ok(models) = parsed {
        return Ok(models);
      }
    }

    Ok(ListModelsResponse {
      object_type: "list".to_string(),
      data: OPENROUTER_MODELS
        .iter()
        .map(|model| ModelInfo {
          id: (*model).to_string(),
          object_type: "model".to_string(),
          created: 0,
          owned_by: Some("openrouter".to_string()),
        })
        .collect(),
    })
  }

  async fn validate_auth(&self) -> Result<()> {
    let url = self.endpoint("models");
    let response = self
      .client
      .get(&url)
      .header("Authorization", format!("Bearer {}", self.api_key))
      .send()
      .await
      .map_err(ModelError::NetworkError)?;

    if response.status().is_success() {
      Ok(())
    } else {
      Err(ModelError::AuthError(
        "Invalid OpenRouter API key".to_string(),
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_openrouter_models_present() {
    assert!(OPENROUTER_MODELS.contains(&"openai/gpt-4o"));
    assert!(OPENROUTER_MODELS.contains(&"anthropic/claude-sonnet-4"));
    assert!(OPENROUTER_MODELS.contains(&"google/gemini-2.0-flash-exp"));
  }
}
