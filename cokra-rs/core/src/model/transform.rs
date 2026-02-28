//! Unified provider message transformation layer.
//!
//! This module mirrors the provider normalization approach used by Opencode:
//! - Request normalization from common message format to provider-specific payloads
//! - Response normalization back to the common [`ChatResponse`] type
//! - Streaming chunk normalization across SSE formats

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::error::{ModelError, Result};
use super::types::{
  ChatRequest, ChatResponse, Choice, ChoiceMessage, Message, ToolCall, ToolCallFunction, Usage,
};

/// Streaming chunk normalized by the transform layer.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamChunk {
  /// Optional text delta produced by the model.
  pub text: Option<String>,
  /// Optional tool call id.
  pub tool_call_id: Option<String>,
  /// Optional tool name.
  pub tool_name: Option<String>,
  /// Optional tool arguments delta.
  pub tool_arguments: Option<String>,
  /// Optional usage update found in the chunk.
  pub usage: Option<Usage>,
  /// True when this chunk marks stream completion.
  pub done: bool,
}

/// Message transform contract for providers.
pub trait MessageTransform: Send + Sync {
  /// Convert common [`ChatRequest`] payload to a provider-specific JSON payload.
  fn transform_request(&self, request: &ChatRequest) -> Result<Value>;

  /// Convert provider-specific JSON response to common [`ChatResponse`].
  fn transform_response(&self, response: &Value) -> Result<ChatResponse>;

  /// Convert one raw stream chunk to a normalized [`StreamChunk`].
  fn transform_chunk(&self, chunk: &str) -> Option<StreamChunk>;
}

/// Tool call id conversion mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallIdFormat {
  /// Keep original id.
  Default,
  /// Mistral-compatible 9-char alphanumeric id.
  Alphanumeric9,
  /// Keep only `[a-zA-Z0-9_-]`.
  Sanitize,
}

/// Empty-content policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyContentHandling {
  /// Remove empty messages/parts.
  Filter,
  /// Replace empty content with provided static fallback.
  Replace(&'static str),
  /// Return error when empty content is found.
  Reject,
}

/// Transform behavior controls.
#[derive(Debug, Clone)]
pub struct TransformConfig {
  pub supports_system_cache: bool,
  pub supports_tool_caching: bool,
  pub tool_call_id_format: ToolCallIdFormat,
  pub empty_content_handling: EmptyContentHandling,
}

impl Default for TransformConfig {
  fn default() -> Self {
    Self {
      supports_system_cache: false,
      supports_tool_caching: false,
      tool_call_id_format: ToolCallIdFormat::Default,
      empty_content_handling: EmptyContentHandling::Filter,
    }
  }
}

/// OpenAI-compatible pass-through transform.
#[derive(Debug, Default)]
pub struct OpenAICompatibleTransform;

impl MessageTransform for OpenAICompatibleTransform {
  fn transform_request(&self, request: &ChatRequest) -> Result<Value> {
    Ok(json!({
      "model": request.model,
      "messages": request.messages,
      "temperature": request.temperature,
      "max_tokens": request.max_tokens,
      "stream": request.stream,
      "tools": request.tools,
      "stop": request.stop,
      "presence_penalty": request.presence_penalty,
      "frequency_penalty": request.frequency_penalty,
      "top_p": request.top_p,
      "user": request.user,
    }))
  }

  fn transform_response(&self, response: &Value) -> Result<ChatResponse> {
    serde_json::from_value(response.clone())
      .map_err(|e| ModelError::InvalidResponse(format!("failed to parse OpenAI response: {e}")))
  }

  fn transform_chunk(&self, chunk: &str) -> Option<StreamChunk> {
    parse_sse_line(chunk).and_then(parse_openai_compatible_chunk)
  }
}

/// Anthropic-specific request/response transform.
#[derive(Debug, Clone)]
pub struct AnthropicTransform {
  config: TransformConfig,
}

impl AnthropicTransform {
  pub fn new() -> Self {
    Self {
      config: TransformConfig {
        supports_system_cache: true,
        supports_tool_caching: true,
        tool_call_id_format: ToolCallIdFormat::Sanitize,
        empty_content_handling: EmptyContentHandling::Filter,
      },
    }
  }

  pub fn with_config(config: TransformConfig) -> Self {
    Self { config }
  }

  pub fn config(&self) -> &TransformConfig {
    &self.config
  }

  fn filter_empty_content(&self, messages: &mut Vec<Message>) -> Result<()> {
    match self.config.empty_content_handling {
      EmptyContentHandling::Filter => {
        messages.retain(|m| !is_empty_message(m));
        Ok(())
      }
      EmptyContentHandling::Replace(replacement) => {
        for msg in &mut *messages {
          replace_empty_message(msg, replacement);
        }
        Ok(())
      }
      EmptyContentHandling::Reject => {
        if messages.iter().any(is_empty_message) {
          return Err(ModelError::InvalidRequest(
            "anthropic transform rejected empty message content".to_string(),
          ));
        }
        Ok(())
      }
    }
  }

  fn normalize_tool_call_id(&self, id: &str) -> String {
    normalize_tool_call_id(id, self.config.tool_call_id_format)
  }

  fn to_anthropic_message(&self, msg: &Message) -> Option<Value> {
    match msg {
      Message::System(content) => Some(json!({
        "role": "user",
        "content": [{
          "type": "text",
          "text": format!("<system_prompt>{content}</system_prompt>")
        }]
      })),
      Message::User(content) => Some(json!({
        "role": "user",
        "content": [{
          "type": "text",
          "text": content
        }]
      })),
      Message::Assistant {
        content,
        tool_calls,
      } => {
        let mut parts = Vec::<Value>::new();

        if let Some(text) = content {
          if !text.is_empty() {
            parts.push(json!({
              "type": "text",
              "text": text
            }));
          }
        }

        if let Some(calls) = tool_calls {
          for call in calls {
            let input = match serde_json::from_str::<Value>(&call.function.arguments) {
              Ok(v) => v,
              Err(_) => json!({ "raw": call.function.arguments }),
            };
            parts.push(json!({
              "type": "tool_use",
              "id": self.normalize_tool_call_id(&call.id),
              "name": call.function.name,
              "input": input
            }));
          }
        }

        if parts.is_empty() {
          return None;
        }

        Some(json!({
          "role": "assistant",
          "content": parts
        }))
      }
      Message::Tool {
        tool_call_id,
        content,
      } => Some(json!({
        "role": "user",
        "content": [{
          "type": "tool_result",
          "tool_use_id": self.normalize_tool_call_id(tool_call_id),
          "content": content
        }]
      })),
    }
  }
}

impl Default for AnthropicTransform {
  fn default() -> Self {
    Self::new()
  }
}

impl MessageTransform for AnthropicTransform {
  fn transform_request(&self, request: &ChatRequest) -> Result<Value> {
    let mut messages = request.messages.clone();
    self.filter_empty_content(&mut messages)?;

    let system = messages.iter().find_map(|m| match m {
      Message::System(content) => Some(content.clone()),
      _ => None,
    });

    let mut provider_messages = Vec::new();
    for msg in &messages {
      if matches!(msg, Message::System(_)) {
        continue;
      }
      if let Some(converted) = self.to_anthropic_message(msg) {
        provider_messages.push(converted);
      }
    }

    Ok(json!({
      "model": request.model,
      "messages": provider_messages,
      "max_tokens": request.max_tokens.unwrap_or(4096),
      "temperature": request.temperature,
      "top_p": request.top_p,
      "system": system,
      "tools": request.tools.as_ref().map(|tools| {
        tools.iter()
          .filter_map(|tool| tool.function.as_ref())
          .map(|function| json!({
            "name": function.name,
            "description": function.description,
            "input_schema": function.parameters
          }))
          .collect::<Vec<_>>()
      }),
      "stream": request.stream,
    }))
  }

  fn transform_response(&self, response: &Value) -> Result<ChatResponse> {
    if response.get("choices").is_some() {
      return serde_json::from_value(response.clone())
        .map_err(|e| ModelError::InvalidResponse(format!("failed to parse response: {e}")));
    }

    let id = response
      .get("id")
      .and_then(Value::as_str)
      .unwrap_or("anthropic-response")
      .to_string();
    let model = response
      .get("model")
      .and_then(Value::as_str)
      .unwrap_or("anthropic/unknown")
      .to_string();
    let stop_reason = response
      .get("stop_reason")
      .and_then(Value::as_str)
      .map(ToString::to_string);

    let usage = response
      .get("usage")
      .and_then(parse_usage_from_value)
      .unwrap_or_default();

    let mut text_parts = Vec::<String>::new();
    let mut tool_calls = Vec::<ToolCall>::new();

    if let Some(content) = response.get("content").and_then(Value::as_array) {
      for item in content {
        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
          "text" => {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
              text_parts.push(text.to_string());
            }
          }
          "tool_use" => {
            let id = item
              .get("id")
              .and_then(Value::as_str)
              .unwrap_or("tool_call_0")
              .to_string();
            let name = item
              .get("name")
              .and_then(Value::as_str)
              .unwrap_or("tool")
              .to_string();
            let arguments = match item.get("input") {
              Some(Value::String(s)) => s.clone(),
              Some(v) => serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()),
              None => "{}".to_string(),
            };
            tool_calls.push(ToolCall {
              id,
              call_type: "function".to_string(),
              function: ToolCallFunction { name, arguments },
            });
          }
          _ => {}
        }
      }
    }

    Ok(ChatResponse {
      id,
      object_type: "chat.completion".to_string(),
      created: Utc::now().timestamp() as u64,
      model,
      choices: vec![Choice {
        index: 0,
        message: ChoiceMessage {
          role: "assistant".to_string(),
          content: if text_parts.is_empty() {
            None
          } else {
            Some(text_parts.join(""))
          },
          tool_calls: if tool_calls.is_empty() {
            None
          } else {
            Some(tool_calls)
          },
        },
        finish_reason: stop_reason,
      }],
      usage,
      extra: Default::default(),
    })
  }

  fn transform_chunk(&self, chunk: &str) -> Option<StreamChunk> {
    parse_sse_line(chunk).and_then(|value| {
      let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();

      if event_type == "message_stop" {
        return Some(StreamChunk {
          done: true,
          usage: value.get("usage").and_then(parse_usage_from_value),
          ..Default::default()
        });
      }

      if event_type == "content_block_delta" {
        let text = value
          .get("delta")
          .and_then(|delta| delta.get("text"))
          .and_then(Value::as_str)
          .map(ToString::to_string);
        return Some(StreamChunk {
          text,
          usage: value.get("usage").and_then(parse_usage_from_value),
          ..Default::default()
        });
      }

      parse_openai_compatible_chunk(value)
    })
  }
}

/// Selects an appropriate transform implementation for a provider id.
pub fn transform_for_provider(provider_id: &str) -> Box<dyn MessageTransform> {
  match provider_id {
    "anthropic" => Box::new(AnthropicTransform::new()),
    _ => Box::new(OpenAICompatibleTransform),
  }
}

/// Mistral requires alphanumeric tool call IDs with exactly 9 chars.
pub fn normalize_tool_call_id_for_mistral(id: &str) -> String {
  normalize_tool_call_id(id, ToolCallIdFormat::Alphanumeric9)
}

fn normalize_tool_call_id(id: &str, format: ToolCallIdFormat) -> String {
  match format {
    ToolCallIdFormat::Default => id.to_string(),
    ToolCallIdFormat::Sanitize => id
      .chars()
      .map(|c| {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
          c
        } else {
          '_'
        }
      })
      .collect(),
    ToolCallIdFormat::Alphanumeric9 => {
      let mut normalized: String = id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(9)
        .collect();
      while normalized.len() < 9 {
        normalized.push('0');
      }
      normalized
    }
  }
}

fn parse_sse_line(chunk: &str) -> Option<Value> {
  for line in chunk.lines() {
    if !line.starts_with("data: ") {
      continue;
    }
    let data = line.trim_start_matches("data: ").trim();
    if data == "[DONE]" {
      return Some(json!({ "done": true }));
    }
    if let Ok(value) = serde_json::from_str::<Value>(data) {
      return Some(value);
    }
  }
  None
}

fn parse_openai_compatible_chunk(value: Value) -> Option<StreamChunk> {
  if value.get("done").is_some() {
    return Some(StreamChunk {
      done: true,
      ..Default::default()
    });
  }

  let usage = value.get("usage").and_then(parse_usage_from_value);

  let choice = value
    .get("choices")
    .and_then(Value::as_array)
    .and_then(|choices| choices.first())?;

  let delta = choice.get("delta").unwrap_or(&Value::Null);
  let text = delta
    .get("content")
    .and_then(Value::as_str)
    .map(ToString::to_string);

  let mut tool_call_id = None;
  let mut tool_name = None;
  let mut tool_arguments = None;

  if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
    if let Some(first) = tool_calls.first() {
      tool_call_id = first
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string);
      tool_name = first
        .get("function")
        .and_then(|f| f.get("name"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
      tool_arguments = first
        .get("function")
        .and_then(|f| f.get("arguments"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    }
  }

  let done = choice
    .get("finish_reason")
    .and_then(Value::as_str)
    .is_some();

  Some(StreamChunk {
    text,
    tool_call_id,
    tool_name,
    tool_arguments,
    usage,
    done,
  })
}

fn parse_usage_from_value(value: &Value) -> Option<Usage> {
  if let Some(usage) = value.get("usage") {
    return parse_usage_from_value(usage);
  }

  if value.is_object() {
    let input_tokens = value
      .get("prompt_tokens")
      .or_else(|| value.get("input_tokens"))
      .or_else(|| value.get("promptTokenCount"))
      .and_then(Value::as_u64)
      .unwrap_or(0) as u32;
    let output_tokens = value
      .get("completion_tokens")
      .or_else(|| value.get("output_tokens"))
      .or_else(|| value.get("candidatesTokenCount"))
      .and_then(Value::as_u64)
      .unwrap_or(0) as u32;
    let total_tokens = value
      .get("total_tokens")
      .or_else(|| value.get("totalTokenCount"))
      .and_then(Value::as_u64)
      .unwrap_or((input_tokens + output_tokens) as u64) as u32;

    if input_tokens == 0 && output_tokens == 0 && total_tokens == 0 {
      return None;
    }

    return Some(Usage {
      input_tokens,
      output_tokens,
      total_tokens,
    });
  }

  None
}

fn is_empty_message(message: &Message) -> bool {
  match message {
    Message::System(content) | Message::User(content) => content.trim().is_empty(),
    Message::Assistant {
      content,
      tool_calls,
    } => {
      let no_content = content
        .as_ref()
        .map(|c| c.trim().is_empty())
        .unwrap_or(true);
      let no_tool_calls = tool_calls.as_ref().map(Vec::is_empty).unwrap_or(true);
      no_content && no_tool_calls
    }
    Message::Tool {
      tool_call_id,
      content,
    } => tool_call_id.trim().is_empty() || content.trim().is_empty(),
  }
}

fn replace_empty_message(message: &mut Message, replacement: &str) {
  match message {
    Message::System(content) | Message::User(content) => {
      if content.trim().is_empty() {
        *content = replacement.to_string();
      }
    }
    Message::Assistant {
      content,
      tool_calls,
    } => {
      if content
        .as_ref()
        .map(|c| c.trim().is_empty())
        .unwrap_or(true)
        && tool_calls.as_ref().map(Vec::is_empty).unwrap_or(true)
      {
        *content = Some(replacement.to_string());
      }
    }
    Message::Tool {
      tool_call_id,
      content,
    } => {
      if tool_call_id.trim().is_empty() {
        *tool_call_id = "tool_call_0".to_string();
      }
      if content.trim().is_empty() {
        *content = replacement.to_string();
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::model::types::Message;

  #[test]
  fn test_normalize_tool_call_id_for_mistral() {
    let normalized = normalize_tool_call_id_for_mistral("ab-12_cd*xy!zz");
    assert_eq!(normalized, "ab12cdxyz");
  }

  #[test]
  fn test_normalize_tool_call_id_for_mistral_pad() {
    let normalized = normalize_tool_call_id_for_mistral("x");
    assert_eq!(normalized, "x00000000");
  }

  #[test]
  fn test_anthropic_filters_empty_content() {
    let transform = AnthropicTransform::new();
    let request = ChatRequest {
      model: "claude-3-5-sonnet-20241022".to_string(),
      messages: vec![
        Message::System("".to_string()),
        Message::User("".to_string()),
        Message::User("hello".to_string()),
      ],
      ..Default::default()
    };

    let payload = transform.transform_request(&request).expect("payload");
    let messages = payload
      .get("messages")
      .and_then(Value::as_array)
      .expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(
      messages[0]
        .get("role")
        .and_then(Value::as_str)
        .expect("role"),
      "user"
    );
  }

  #[test]
  fn test_anthropic_sanitizes_tool_call_ids() {
    let transform = AnthropicTransform::new();
    let request = ChatRequest {
      model: "claude-sonnet-4-20250514".to_string(),
      messages: vec![Message::Assistant {
        content: Some("calling tool".to_string()),
        tool_calls: Some(vec![ToolCall {
          id: "tool*id#1".to_string(),
          call_type: "function".to_string(),
          function: ToolCallFunction {
            name: "read_file".to_string(),
            arguments: "{}".to_string(),
          },
        }]),
      }],
      ..Default::default()
    };

    let payload = transform.transform_request(&request).expect("payload");
    let id = payload["messages"][0]["content"][1]["id"]
      .as_str()
      .expect("id");
    assert_eq!(id, "tool_id_1");
  }

  #[test]
  fn test_parse_openai_chunk() {
    let chunk = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}";
    let parsed = OpenAICompatibleTransform
      .transform_chunk(chunk)
      .expect("parsed");
    assert_eq!(parsed.text, Some("hi".to_string()));
    assert!(!parsed.done);
  }
}
