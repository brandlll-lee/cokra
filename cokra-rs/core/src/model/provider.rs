//! Model Provider trait and implementations
//!
//! This module defines the [ModelProvider] trait that all LLM providers must implement.

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::pin::Pin;

use cokra_protocol::{
  ContentDeltaEvent as ResponseContentDeltaEvent, FunctionCall as ResponseFunctionCall,
  FunctionCallEvent as ResponseFunctionCallEvent, ResponseEvent,
};

use super::error::{ModelError, Result};
use super::types::{ChatRequest, ChatResponse, Chunk, ListModelsResponse, ProviderConfig};

pub type ResponseEventStream = Pin<Box<dyn Stream<Item = Result<ResponseEvent>> + Send>>;

/// Model Provider trait
///
/// All LLM providers must implement this trait to be used with Cokra.
/// It provides a unified interface for chat completions, streaming, and authentication.
#[async_trait]
pub trait ModelProvider: Send + Sync {
  /// Returns the unique identifier for this provider
  fn provider_id(&self) -> &'static str;

  /// Returns the display name for this provider
  fn provider_name(&self) -> &'static str;

  /// Returns the list of environment variables required by this provider
  fn required_env_vars(&self) -> Vec<&'static str> {
    Vec::new()
  }

  /// Returns the default models for this provider
  fn default_models(&self) -> Vec<&'static str> {
    Vec::new()
  }

  /// Creates a chat completion
  async fn chat_completion(&self, request: ChatRequest) -> Result<ChatResponse>;

  /// Creates a streaming chat completion
  async fn chat_completion_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>>;

  /// Creates a Responses-API compatible event stream.
  async fn responses_stream(&self, request: ChatRequest) -> Result<ResponseEventStream> {
    let chunk_stream = self.chat_completion_stream(request).await?;
    Ok(chunk_stream_to_response_events(chunk_stream))
  }

  /// Lists available models for this provider
  async fn list_models(&self) -> Result<ListModelsResponse>;

  /// Validates that authentication is working
  async fn validate_auth(&self) -> Result<()>;

  /// Returns the HTTP client for this provider
  fn client(&self) -> &Client;

  /// Returns the configuration for this provider
  fn config(&self) -> &ProviderConfig;
}

#[derive(Debug, Clone, Default)]
struct FunctionCallBuffer {
  id: String,
  name: String,
  arguments: String,
}

/// Convert provider chunk stream into codex-style response events.
pub fn chunk_stream_to_response_events(
  mut chunk_stream: Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>,
) -> ResponseEventStream {
  Box::pin(async_stream::stream! {
    let mut text_index: usize = 0;
    let mut function_calls: BTreeMap<String, FunctionCallBuffer> = BTreeMap::new();
    let mut active_call_id: Option<String> = None;
    let mut emitted_end_turn = false;

    while let Some(chunk) = chunk_stream.next().await {
      let chunk = match chunk {
        Ok(chunk) => chunk,
        Err(err) => {
          yield Err(err);
          return;
        }
      };

      match chunk {
        Chunk::Content { delta } => {
          if delta.text.is_empty() {
            continue;
          }
          let text = delta.text;
          yield Ok(ResponseEvent::ContentDelta(ResponseContentDeltaEvent {
            text,
            index: text_index,
          }));
          text_index += 1;
        }
        Chunk::ToolCall { delta } => {
          let call_id = delta
            .id
            .clone()
            .or_else(|| active_call_id.clone())
            .unwrap_or_else(|| format!("tool_call_{}", function_calls.len() + 1));

          active_call_id = Some(call_id.clone());

          let entry = function_calls
            .entry(call_id.clone())
            .or_insert_with(|| FunctionCallBuffer {
              id: call_id.clone(),
              ..Default::default()
            });

          if let Some(name) = delta.name {
            entry.name = name;
          }
          if let Some(arguments) = delta.arguments {
            entry.arguments.push_str(&arguments);
          }
        }
        Chunk::MessageStop => {
          for call in function_calls.values() {
            if call.name.is_empty() {
              continue;
            }
            yield Ok(ResponseEvent::FunctionCall(ResponseFunctionCallEvent {
              id: call.id.clone(),
              call_type: "function".to_string(),
              function: ResponseFunctionCall {
                name: call.name.clone(),
                arguments: call.arguments.clone(),
              },
            }));
          }
          function_calls.clear();
          active_call_id = None;
          emitted_end_turn = true;
          yield Ok(ResponseEvent::EndTurn);
        }
        Chunk::MessageStart { .. } | Chunk::MessageDelta { .. } | Chunk::Unknown => {}
      }
    }

    if !function_calls.is_empty() {
      for call in function_calls.values() {
        if call.name.is_empty() {
          continue;
        }
        yield Ok(ResponseEvent::FunctionCall(ResponseFunctionCallEvent {
          id: call.id.clone(),
          call_type: "function".to_string(),
          function: ResponseFunctionCall {
            name: call.name.clone(),
            arguments: call.arguments.clone(),
          },
        }));
      }
    }

    if !emitted_end_turn {
      yield Ok(ResponseEvent::EndTurn);
    }
  })
}

/// Information about a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
  /// Unique identifier
  pub id: String,

  /// Display name
  pub name: String,

  /// Required environment variables
  pub env_vars: Vec<String>,

  /// Whether the provider is authenticated
  pub authenticated: bool,

  /// Available models
  pub models: Vec<String>,

  /// Provider-specific options
  #[serde(default)]
  pub options: serde_json::Value,
}

impl ProviderInfo {
  /// Create a new ProviderInfo
  pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
    Self {
      id: id.into(),
      name: name.into(),
      env_vars: Vec::new(),
      authenticated: false,
      models: Vec::new(),
      options: serde_json::json!({}),
    }
  }

  /// Set authenticated status
  pub fn authenticated(mut self, authenticated: bool) -> Self {
    self.authenticated = authenticated;
    self
  }

  /// Set environment variables
  pub fn env_vars(mut self, env_vars: Vec<String>) -> Self {
    self.env_vars = env_vars;
    self
  }

  /// Set available models
  pub fn models(mut self, models: Vec<String>) -> Self {
    self.models = models;
    self
  }
}

/// Builder for creating providers
pub struct ProviderBuilder<P: ModelProvider> {
  provider: P,
}

impl<P: ModelProvider + Default> ProviderBuilder<P> {
  /// Create a new builder with default configuration
  pub fn new() -> Self {
    Self {
      provider: P::default(),
    }
  }
}

impl<P: ModelProvider> ProviderBuilder<P> {
  /// Set the API key
  pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
    // This would need to be implemented by each provider
    self
  }

  /// Set the base URL
  pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
    self
  }

  /// Build the provider
  pub fn build(self) -> P {
    self.provider
  }
}

/// Stream wrapper for async streams
pub struct ModelStream {
  // Internal stream type
}

impl ModelStream {
  /// Create a new model stream
  pub fn new() -> Self {
    Self {}
  }
}

/// Trait for converting responses to chunks
pub trait ToChunk {
  /// Convert a response to a chunk
  fn to_chunk(&self) -> Chunk;
}

// =============================================================================
// Helper functions for provider implementations
// =============================================================================

/// Standard error handling for HTTP responses
pub async fn handle_response(response: reqwest::Response) -> Result<String> {
  if response.status().is_success() {
    Ok(response.text().await?)
  } else {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(ModelError::ApiError(format!("HTTP {}: {}", status, body)))
  }
}

/// Parse JSON from response
pub async fn parse_response<T: serde::de::DeserializeOwned>(
  response: reqwest::Response,
) -> Result<T> {
  let body = handle_response(response).await?;
  Ok(serde_json::from_str(&body)?)
}

/// Build headers for API requests
pub fn build_headers(
  api_key: &str,
  extra: &std::collections::HashMap<String, String>,
) -> reqwest::header::HeaderMap {
  use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};

  let mut headers = HeaderMap::new();

  // Authorization header
  if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", api_key)) {
    headers.insert(AUTHORIZATION, value);
  }

  // Extra headers
  for (key, value) in extra {
    if let (Ok(name), Ok(val)) = (
      HeaderName::from_bytes(key.as_bytes()),
      HeaderValue::from_str(value),
    ) {
      headers.insert(name, val);
    }
  }

  headers
}

#[cfg(test)]
mod tests {
  use super::*;
  use futures::StreamExt;

  #[tokio::test]
  async fn chunk_stream_converts_text_delta_and_end_turn() {
    let source = futures::stream::iter(vec![
      Ok(Chunk::Content {
        delta: super::super::types::ContentDelta {
          text: "Hello".to_string(),
        },
      }),
      Ok(Chunk::Content {
        delta: super::super::types::ContentDelta {
          text: " World".to_string(),
        },
      }),
      Ok(Chunk::MessageStop),
    ]);

    let mut stream = chunk_stream_to_response_events(Box::pin(source));
    let mut seen = Vec::new();
    while let Some(event) = stream.next().await {
      seen.push(event.expect("response event"));
    }

    assert_eq!(
      seen,
      vec![
        ResponseEvent::ContentDelta(ResponseContentDeltaEvent {
          text: "Hello".to_string(),
          index: 0,
        }),
        ResponseEvent::ContentDelta(ResponseContentDeltaEvent {
          text: " World".to_string(),
          index: 1,
        }),
        ResponseEvent::EndTurn,
      ]
    );
  }

  #[tokio::test]
  async fn chunk_stream_converts_tool_call_before_end_turn() {
    let source = futures::stream::iter(vec![
      Ok(Chunk::ToolCall {
        delta: super::super::types::ToolCallDelta {
          id: Some("call_1".to_string()),
          name: Some("read_file".to_string()),
          arguments: Some("{\"file_path\":\"a.txt\"}".to_string()),
        },
      }),
      Ok(Chunk::MessageStop),
    ]);

    let mut stream = chunk_stream_to_response_events(Box::pin(source));
    let mut seen = Vec::new();
    while let Some(event) = stream.next().await {
      seen.push(event.expect("response event"));
    }

    assert_eq!(
      seen,
      vec![
        ResponseEvent::FunctionCall(ResponseFunctionCallEvent {
          id: "call_1".to_string(),
          call_type: "function".to_string(),
          function: ResponseFunctionCall {
            name: "read_file".to_string(),
            arguments: "{\"file_path\":\"a.txt\"}".to_string(),
          },
        }),
        ResponseEvent::EndTurn,
      ]
    );
  }
}
