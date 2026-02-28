//! Google Gemini Provider
//!
//! Native Gemini REST API integration.

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

use super::super::error::{ModelError, Result};
use super::super::provider::ModelProvider;
use super::super::types::{
  ChatRequest, ChatResponse, Choice, ChoiceMessage, Chunk, ContentDelta, ListModelsResponse,
  Message, ModelInfo, ProviderConfig, Usage,
};
use super::create_client;

/// Google Gemini provider.
pub struct GoogleProvider {
  client: Client,
  config: ProviderConfig,
  api_key: String,
  base_url: String,
  _project_id: Option<String>,
  _location: Option<String>,
}

impl GoogleProvider {
  /// Creates a new Gemini provider.
  pub fn new(api_key: String, config: ProviderConfig) -> Self {
    let base_url = config
      .base_url
      .clone()
      .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta".to_string());

    Self {
      client: create_client(config.timeout),
      config,
      api_key,
      base_url,
      _project_id: std::env::var("GOOGLE_PROJECT_ID").ok(),
      _location: std::env::var("GOOGLE_LOCATION").ok(),
    }
  }

  fn model_endpoint(&self, model: &str, stream: bool) -> String {
    let method = if stream {
      "streamGenerateContent?alt=sse"
    } else {
      "generateContent"
    };
    let query_sep = if stream { "&" } else { "?" };
    format!(
      "{}/models/{}:{}{}key={}",
      self.base_url.trim_end_matches('/'),
      model,
      method,
      query_sep,
      self.api_key
    )
  }

  fn parse_gemini_response(&self, text: &str, model: &str) -> Result<ChatResponse> {
    let gemini: GeminiResponse = serde_json::from_str(text)
      .map_err(|e| ModelError::InvalidResponse(format!("invalid Gemini response: {e}")))?;

    let first = gemini
      .candidates
      .first()
      .ok_or_else(|| ModelError::InvalidResponse("Gemini returned no candidates".to_string()))?;

    let content = first
      .content
      .parts
      .iter()
      .filter_map(|part| part.text.clone())
      .collect::<Vec<_>>()
      .join("");

    let usage = gemini
      .usage_metadata
      .map(|usage| Usage {
        input_tokens: usage.prompt_token_count.unwrap_or(0),
        output_tokens: usage.candidates_token_count.unwrap_or(0),
        total_tokens: usage.total_token_count.unwrap_or(0),
      })
      .unwrap_or_default();

    Ok(ChatResponse {
      id: uuid::Uuid::new_v4().to_string(),
      object_type: "chat.completion".to_string(),
      created: chrono::Utc::now().timestamp() as u64,
      model: model.to_string(),
      choices: vec![Choice {
        index: 0,
        message: ChoiceMessage {
          role: "assistant".to_string(),
          content: if content.is_empty() {
            None
          } else {
            Some(content)
          },
          tool_calls: None,
        },
        finish_reason: first.finish_reason.clone(),
      }],
      usage,
      extra: Default::default(),
    })
  }

  fn to_gemini_request(&self, request: &ChatRequest) -> GeminiRequest {
    let mut contents = Vec::new();

    for message in &request.messages {
      let content = match message {
        Message::System(text) => GeminiContent {
          role: "user".to_string(),
          parts: vec![GeminiPart {
            text: Some(format!("<system_prompt>{text}</system_prompt>")),
          }],
        },
        Message::User(text) => GeminiContent {
          role: "user".to_string(),
          parts: vec![GeminiPart {
            text: Some(text.clone()),
          }],
        },
        Message::Assistant { content, .. } => GeminiContent {
          role: "model".to_string(),
          parts: vec![GeminiPart {
            text: Some(content.clone().unwrap_or_default()),
          }],
        },
        Message::Tool {
          tool_call_id,
          content,
        } => GeminiContent {
          role: "user".to_string(),
          parts: vec![GeminiPart {
            text: Some(format!("[Tool Result for {tool_call_id}]: {content}")),
          }],
        },
      };
      contents.push(content);
    }

    GeminiRequest {
      contents,
      generation_config: Some(GeminiGenerationConfig {
        temperature: request.temperature,
        max_output_tokens: request.max_tokens,
        top_p: request.top_p,
      }),
      safety_settings: None,
      system_instruction: None,
    }
  }

  fn parse_stream_text(value: &serde_json::Value) -> Option<String> {
    let candidates = value.get("candidates")?.as_array()?;
    let first = candidates.first()?;
    let content = first.get("content")?;
    let parts = content.get("parts")?.as_array()?;
    let text = parts
      .iter()
      .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
      .collect::<Vec<_>>()
      .join("");
    if text.is_empty() { None } else { Some(text) }
  }

  fn is_stream_done(value: &serde_json::Value) -> bool {
    value
      .get("candidates")
      .and_then(serde_json::Value::as_array)
      .and_then(|c| c.first())
      .and_then(|c| c.get("finishReason"))
      .and_then(serde_json::Value::as_str)
      .is_some()
  }
}

/// Gemini models.
pub const GOOGLE_MODELS: &[&str] = &[
  "gemini-2.0-flash-exp",
  "gemini-1.5-pro",
  "gemini-1.5-flash",
  "gemini-1.5-flash-8b",
  "gemini-1.0-pro",
];

#[async_trait]
impl ModelProvider for GoogleProvider {
  fn provider_id(&self) -> &'static str {
    "google"
  }

  fn provider_name(&self) -> &'static str {
    "Google Gemini"
  }

  fn required_env_vars(&self) -> Vec<&'static str> {
    vec!["GOOGLE_API_KEY"]
  }

  fn default_models(&self) -> Vec<&'static str> {
    GOOGLE_MODELS.to_vec()
  }

  async fn chat_completion(&self, request: ChatRequest) -> Result<ChatResponse> {
    let model = request.model.clone();
    let url = self.model_endpoint(&model, false);
    let body = self.to_gemini_request(&request);

    let response = self
      .client
      .post(&url)
      .header("Content-Type", "application/json")
      .header("x-goog-api-key", &self.api_key)
      .json(&body)
      .send()
      .await
      .map_err(ModelError::NetworkError)?;

    if !response.status().is_success() {
      let status = response.status();
      let error_text = response.text().await.unwrap_or_default();
      return Err(ModelError::ApiError(format!(
        "HTTP {}: {}",
        status, error_text
      )));
    }

    let text = response.text().await.map_err(ModelError::NetworkError)?;
    self.parse_gemini_response(&text, &model)
  }

  async fn chat_completion_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>> {
    let model = request.model.clone();
    let url = self.model_endpoint(&model, true);
    let body = self.to_gemini_request(&request);

    let response = self
      .client
      .post(&url)
      .header("Content-Type", "application/json")
      .header("x-goog-api-key", &self.api_key)
      .json(&body)
      .send()
      .await
      .map_err(ModelError::NetworkError)?;

    let mut byte_stream = response.bytes_stream();

    let stream = async_stream::stream! {
      let mut buffer = String::new();

      while let Some(item) = byte_stream.next().await {
        match item {
          Ok(bytes) => {
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            loop {
              let mut split_idx = buffer.find("\r\n\r\n");
              let delimiter_len = if split_idx.is_some() { 4 } else { 2 };
              if split_idx.is_none() {
                split_idx = buffer.find("\n\n");
              }
              let Some(idx) = split_idx else {
                break;
              };

              let event = buffer[..idx].to_string();
              buffer.drain(..idx + delimiter_len);

              for line in event.lines() {
                if !line.starts_with("data: ") {
                  continue;
                }
                let payload = line.trim_start_matches("data: ").trim();
                if payload == "[DONE]" {
                  yield Ok(Chunk::MessageStop);
                  continue;
                }
                match serde_json::from_str::<serde_json::Value>(payload) {
                  Ok(value) => {
                    if let Some(text) = Self::parse_stream_text(&value) {
                      yield Ok(Chunk::Content {
                        delta: ContentDelta { text }
                      });
                    }
                    if Self::is_stream_done(&value) {
                      yield Ok(Chunk::MessageStop);
                    }
                  }
                  Err(err) => {
                    yield Err(ModelError::StreamError(format!("invalid Gemini stream chunk: {err}")));
                  }
                }
              }
            }
          }
          Err(err) => {
            yield Err(ModelError::StreamError(err.to_string()));
          }
        }
      }
    };

    Ok(Box::pin(stream))
  }

  async fn list_models(&self) -> Result<ListModelsResponse> {
    Ok(ListModelsResponse {
      object_type: "list".to_string(),
      data: GOOGLE_MODELS
        .iter()
        .map(|id| ModelInfo {
          id: (*id).to_string(),
          object_type: "model".to_string(),
          created: 0,
          owned_by: Some("google".to_string()),
        })
        .collect(),
    })
  }

  async fn validate_auth(&self) -> Result<()> {
    let url = format!(
      "{}/models?key={}",
      self.base_url.trim_end_matches('/'),
      self.api_key
    );
    let response = self
      .client
      .get(&url)
      .send()
      .await
      .map_err(ModelError::NetworkError)?;
    if response.status().is_success() {
      Ok(())
    } else {
      Err(ModelError::AuthError("Invalid GOOGLE_API_KEY".to_string()))
    }
  }

  fn client(&self) -> &Client {
    &self.client
  }

  fn config(&self) -> &ProviderConfig {
    &self.config
  }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
  contents: Vec<GeminiContent>,
  #[serde(skip_serializing_if = "Option::is_none")]
  generation_config: Option<GeminiGenerationConfig>,
  #[serde(skip_serializing_if = "Option::is_none")]
  safety_settings: Option<Vec<serde_json::Value>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  system_instruction: Option<GeminiContent>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiContent {
  role: String,
  parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiPart {
  #[serde(skip_serializing_if = "Option::is_none")]
  text: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
  #[serde(skip_serializing_if = "Option::is_none")]
  temperature: Option<f32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  max_output_tokens: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  top_p: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
  candidates: Vec<GeminiCandidate>,
  #[serde(default)]
  usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
  content: GeminiContent,
  #[serde(default)]
  finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
  #[serde(default)]
  prompt_token_count: Option<u32>,
  #[serde(default)]
  candidates_token_count: Option<u32>,
  #[serde(default)]
  total_token_count: Option<u32>,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_google_models_list() {
    assert!(GOOGLE_MODELS.contains(&"gemini-1.5-pro"));
    assert!(GOOGLE_MODELS.contains(&"gemini-2.0-flash-exp"));
  }

  #[test]
  fn test_parse_gemini_response() {
    let provider = GoogleProvider::new(
      "test-key".to_string(),
      ProviderConfig {
        provider_id: "google".to_string(),
        ..Default::default()
      },
    );

    let json = r#"{
      "candidates": [{
        "content": {
          "role": "model",
          "parts": [{"text": "hello from gemini"}]
        },
        "finishReason": "STOP"
      }],
      "usageMetadata": {
        "promptTokenCount": 10,
        "candidatesTokenCount": 5,
        "totalTokenCount": 15
      }
    }"#;

    let parsed = provider.parse_gemini_response(json, "gemini-1.5-pro");
    assert!(parsed.is_ok());
    let response = parsed.expect("response");
    assert_eq!(
      response.choices[0].message.content.as_deref(),
      Some("hello from gemini")
    );
    assert_eq!(response.usage.total_tokens, 15);
  }
}
