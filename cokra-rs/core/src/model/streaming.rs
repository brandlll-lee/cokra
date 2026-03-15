//! Unified streaming helpers.

use std::collections::HashMap;
use std::pin::Pin;

use futures::Stream;
use futures::StreamExt;
use serde_json::Value;

use cokra_protocol::ContentDeltaEvent as ResponseContentDeltaEvent;
use cokra_protocol::FunctionCall as ResponseFunctionCall;
use cokra_protocol::FunctionCallEvent as ResponseFunctionCallEvent;
use cokra_protocol::OutputItemEvent;
use cokra_protocol::ResponseErrorEvent;
use cokra_protocol::ResponseEvent;
use cokra_protocol::ResponseRateLimitsSnapshot;
use cokra_protocol::ResponseTokenUsage;

use super::types::Chunk;
use super::types::ContentDelta;
use super::types::ToolCallDelta;
use super::types::Usage;
use crate::model::error::ModelError;
use crate::model::error::Result;

/// Optional binary decoder for stream payloads.
pub trait BinaryDecoder: Send + Sync {
  fn decode(&self, bytes: &[u8]) -> std::result::Result<String, String>;
}

/// Usage parser contract.
pub trait UsageParser: Send + Sync {
  fn parse(&mut self, chunk: &str);
  fn retrieve(&self) -> Option<Usage>;
}

/// Streaming processor configuration.
pub struct StreamingConfig {
  pub separator: &'static str,
  pub usage_parser: Box<dyn UsageParser>,
  pub binary_decoder: Option<Box<dyn BinaryDecoder>>,
}

/// Parsed stream event.
#[derive(Debug, Clone)]
pub struct ParsedStreamEvent {
  pub chunk: Option<Chunk>,
  pub usage: Option<Usage>,
  pub done: bool,
}

/// OpenAI-compatible usage parser.
#[derive(Default)]
pub struct OpenAIUsageParser {
  usage: Option<Usage>,
}

impl UsageParser for OpenAIUsageParser {
  fn parse(&mut self, chunk: &str) {
    let Some(value) = parse_data_line_json(chunk) else {
      return;
    };

    let usage = value.get("usage").and_then(parse_usage);
    if usage.is_some() {
      self.usage = usage;
    }
  }

  fn retrieve(&self) -> Option<Usage> {
    self.usage.clone()
  }
}

/// Anthropic usage parser.
#[derive(Default)]
pub struct AnthropicUsageParser {
  usage: Option<Usage>,
}

impl UsageParser for AnthropicUsageParser {
  fn parse(&mut self, chunk: &str) {
    let Some(value) = parse_data_line_json(chunk) else {
      return;
    };

    let top = value.get("usage").and_then(parse_usage);
    if top.is_some() {
      self.usage = top;
      return;
    }

    let message = value
      .get("message")
      .and_then(|msg| msg.get("usage"))
      .and_then(parse_usage);
    if message.is_some() {
      self.usage = message;
    }
  }

  fn retrieve(&self) -> Option<Usage> {
    self.usage.clone()
  }
}

/// Stateful SSE streaming parser.
pub struct StreamingProcessor {
  config: StreamingConfig,
  buffer: String,
}

impl StreamingProcessor {
  pub fn new(config: StreamingConfig) -> Self {
    Self {
      config,
      buffer: String::new(),
    }
  }

  /// Feeds one text segment and returns normalized events.
  pub fn push_text(&mut self, text: &str) -> Vec<ParsedStreamEvent> {
    self.buffer.push_str(text);
    self.drain_events()
  }

  /// Feeds bytes (optionally decoded with binary decoder) and returns events.
  pub fn push_bytes(&mut self, bytes: &[u8]) -> Vec<ParsedStreamEvent> {
    let decoded = match &self.config.binary_decoder {
      Some(decoder) => decoder.decode(bytes).unwrap_or_default(),
      None => String::from_utf8_lossy(bytes).to_string(),
    };
    self.push_text(&decoded)
  }

  /// Flushes the remaining buffer.
  pub fn finish(&mut self) -> Vec<ParsedStreamEvent> {
    if self.buffer.is_empty() {
      return Vec::new();
    }
    let remaining = std::mem::take(&mut self.buffer);
    vec![parse_event(&remaining, &mut *self.config.usage_parser)]
  }

  fn drain_events(&mut self) -> Vec<ParsedStreamEvent> {
    let mut events = Vec::new();
    while let Some(idx) = self.buffer.find(self.config.separator) {
      let event = self.buffer[..idx].to_string();
      self.buffer.drain(..idx + self.config.separator.len());
      events.push(parse_event(&event, &mut *self.config.usage_parser));
    }
    events
  }
}

fn parse_event(raw: &str, parser: &mut dyn UsageParser) -> ParsedStreamEvent {
  let mut event = ParsedStreamEvent {
    chunk: None,
    usage: None,
    done: false,
  };

  for line in raw.lines() {
    if !line.starts_with("data: ") {
      continue;
    }
    let payload = line.trim_start_matches("data: ").trim();
    parser.parse(line);
    event.usage = parser.retrieve();

    if payload == "[DONE]" {
      event.done = true;
      event.chunk = Some(Chunk::MessageStop);
      continue;
    }

    let Ok(value) = serde_json::from_str::<Value>(payload) else {
      continue;
    };

    event.chunk = parse_chunk_value(&value);
    if matches!(event.chunk, Some(Chunk::MessageStop)) {
      event.done = true;
    }
  }

  event
}

fn parse_chunk_value(value: &Value) -> Option<Chunk> {
  // Anthropic style
  if let Some(event_type) = value.get("type").and_then(Value::as_str) {
    match event_type {
      "content_block_delta" => {
        let text = value
          .get("delta")
          .and_then(|delta| delta.get("text"))
          .and_then(Value::as_str)
          .unwrap_or_default()
          .to_string();
        return Some(Chunk::Content {
          delta: ContentDelta { text },
        });
      }
      "tool_call_delta" => {
        let delta = value
          .get("delta")
          .cloned()
          .unwrap_or_else(|| Value::Object(Default::default()));
        let id = delta
          .get("id")
          .and_then(Value::as_str)
          .map(ToString::to_string);
        let name = delta
          .get("name")
          .and_then(Value::as_str)
          .map(ToString::to_string);
        let arguments = delta
          .get("arguments")
          .and_then(Value::as_str)
          .map(ToString::to_string);
        return Some(Chunk::ToolCall {
          delta: ToolCallDelta {
            id,
            name,
            arguments,
            thought_signature: None,
          },
        });
      }
      "message_stop" => return Some(Chunk::MessageStop),
      _ => {}
    }
  }

  // OpenAI-compatible style
  let choice = value
    .get("choices")
    .and_then(Value::as_array)
    .and_then(|choices| choices.first())?;

  if choice
    .get("finish_reason")
    .and_then(Value::as_str)
    .is_some()
  {
    return Some(Chunk::MessageStop);
  }

  let delta = choice.get("delta").unwrap_or(&Value::Null);
  if let Some(text) = delta.get("content").and_then(Value::as_str) {
    return Some(Chunk::Content {
      delta: ContentDelta {
        text: text.to_string(),
      },
    });
  }

  if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
    let first = tool_calls.first()?;
    return Some(Chunk::ToolCall {
      delta: ToolCallDelta {
        id: first
          .get("id")
          .and_then(Value::as_str)
          .map(ToString::to_string),
        name: first
          .get("function")
          .and_then(|f| f.get("name"))
          .and_then(Value::as_str)
          .map(ToString::to_string),
        arguments: first
          .get("function")
          .and_then(|f| f.get("arguments"))
          .and_then(Value::as_str)
          .map(ToString::to_string),
        thought_signature: None,
      },
    });
  }

  None
}

fn parse_data_line_json(chunk: &str) -> Option<Value> {
  for line in chunk.lines() {
    if !line.starts_with("data: ") {
      continue;
    }
    let payload = line.trim_start_matches("data: ").trim();
    if payload == "[DONE]" {
      return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(payload) {
      return Some(value);
    }
  }
  None
}

fn parse_usage(value: &Value) -> Option<Usage> {
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

  Some(Usage {
    input_tokens,
    output_tokens,
    total_tokens,
  })
}

#[derive(Debug, Clone, Default)]
struct FunctionCallBuffer {
  call_id: String,
  name: String,
  arguments: String,
}

pub fn create_openai_responses_event_stream(
  response: reqwest::Response,
) -> Pin<Box<dyn Stream<Item = Result<ResponseEvent>> + Send>> {
  Box::pin(async_stream::stream! {
    let status = response.status();
    let headers = response.headers().clone();
    let mut stream = response.bytes_stream();

    if !status.is_success() {
      let mut body = String::new();
      while let Some(item) = stream.next().await {
        match item {
          Ok(bytes) => body.push_str(&String::from_utf8_lossy(&bytes)),
          Err(err) => {
            yield Err(ModelError::StreamError(err.to_string()));
            return;
          }
        }
      }
      yield Err(ModelError::ApiError(format!("HTTP {}: {}", status, body)));
      return;
    }

    if let Some(snapshot) = response_rate_limits_from_headers(&headers) {
      yield Ok(ResponseEvent::RateLimits(snapshot));
    }

    let mut buffer = String::new();
    let mut text_index = 0usize;
    let mut response_id = String::new();
    let mut function_calls = HashMap::<String, FunctionCallBuffer>::new();

    while let Some(item) = stream.next().await {
      let bytes = match item {
        Ok(bytes) => bytes,
        Err(err) => {
          yield Err(ModelError::StreamError(err.to_string()));
          return;
        }
      };

      buffer.push_str(&String::from_utf8_lossy(&bytes).replace("\r\n", "\n"));
      while let Some(idx) = buffer.find("\n\n") {
        let raw = buffer[..idx].to_string();
        buffer.drain(..idx + 2);
        match parse_openai_responses_event(
          &raw,
          &mut response_id,
          &mut text_index,
          &mut function_calls,
        ) {
          Ok(events) => {
            for event in events {
              yield Ok(event);
            }
          }
          Err(err) => {
            yield Err(err);
            return;
          }
        }
      }
    }

    if !buffer.trim().is_empty() {
      match parse_openai_responses_event(
        &buffer,
        &mut response_id,
        &mut text_index,
        &mut function_calls,
      ) {
        Ok(events) => {
          for event in events {
            yield Ok(event);
          }
        }
        Err(err) => {
          yield Err(err);
          return;
        }
      }
    }
  })
}

pub fn response_event_stream_to_chunk_stream(
  mut stream: Pin<Box<dyn Stream<Item = Result<ResponseEvent>> + Send>>,
) -> Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>> {
  Box::pin(async_stream::stream! {
    while let Some(item) = stream.next().await {
      let event = match item {
        Ok(event) => event,
        Err(err) => {
          yield Err(err);
          return;
        }
      };

      match event {
        ResponseEvent::ContentDelta(delta) => {
          if !delta.text.is_empty() {
            yield Ok(Chunk::Content {
              delta: ContentDelta { text: delta.text },
            });
          }
        }
        ResponseEvent::FunctionCall(call) => {
          yield Ok(Chunk::ToolCall {
            delta: ToolCallDelta {
              id: Some(call.id),
              name: Some(call.function.name),
              arguments: Some(call.function.arguments),
              thought_signature: None,
            },
          });
        }
        ResponseEvent::Completed { .. } | ResponseEvent::EndTurn => {
          yield Ok(Chunk::MessageStop);
        }
        ResponseEvent::Error(err) => {
          yield Err(ModelError::StreamError(err.message));
          return;
        }
        _ => {}
      }
    }
  })
}

fn parse_openai_responses_event(
  raw: &str,
  response_id: &mut String,
  text_index: &mut usize,
  function_calls: &mut HashMap<String, FunctionCallBuffer>,
) -> Result<Vec<ResponseEvent>> {
  let mut events = Vec::new();

  for line in raw.lines() {
    let trimmed = line.trim();
    if !trimmed.starts_with("data: ") {
      continue;
    }

    let payload = trimmed.trim_start_matches("data: ").trim();
    if payload == "[DONE]" || payload.is_empty() {
      continue;
    }

    let value: Value = serde_json::from_str(payload)
      .map_err(|err| ModelError::StreamError(format!("invalid responses event: {err}")))?;
    let event_type = value
      .get("type")
      .and_then(Value::as_str)
      .unwrap_or_default();

    match event_type {
      "response.created" => {
        if let Some(id) = value
          .get("response")
          .and_then(|response| response.get("id"))
          .and_then(Value::as_str)
        {
          *response_id = id.to_string();
        }
        events.push(ResponseEvent::Created);
        if let Some(model) = value
          .get("response")
          .and_then(|response| response.get("model"))
          .and_then(Value::as_str)
        {
          events.push(ResponseEvent::ServerModel(model.to_string()));
        }
      }
      "response.output_item.added" => {
        if let Some(item) = value.get("item") {
          match item.get("type").and_then(Value::as_str) {
            Some("message") => {
              let id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("assistant_item")
                .to_string();
              events.push(ResponseEvent::OutputItemAdded(OutputItemEvent {
                id,
                role: Some("assistant".to_string()),
                item_type: Some("message".to_string()),
              }));
            }
            Some("function_call") => {
              let item_id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
              let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or(&item_id)
                .to_string();
              let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
              let arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
              function_calls.insert(
                item_id,
                FunctionCallBuffer {
                  call_id,
                  name,
                  arguments,
                },
              );
            }
            _ => {}
          }
        }
      }
      "response.output_item.done" => {
        if let Some(item) = value.get("item") {
          match item.get("type").and_then(Value::as_str) {
            Some("message") => {
              let id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("assistant_item")
                .to_string();
              events.push(ResponseEvent::OutputItemDone(OutputItemEvent {
                id,
                role: Some("assistant".to_string()),
                item_type: Some("message".to_string()),
              }));
            }
            Some("function_call") => {
              let item_id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
              let mut call = function_calls.remove(&item_id).unwrap_or_default();
              if call.call_id.is_empty() {
                call.call_id = item
                  .get("call_id")
                  .and_then(Value::as_str)
                  .unwrap_or(&item_id)
                  .to_string();
              }
              if call.name.is_empty() {
                call.name = item
                  .get("name")
                  .and_then(Value::as_str)
                  .unwrap_or_default()
                  .to_string();
              }
              if call.arguments.is_empty() {
                call.arguments = item
                  .get("arguments")
                  .and_then(Value::as_str)
                  .unwrap_or_default()
                  .to_string();
              }
              if !call.name.is_empty() {
                events.push(ResponseEvent::FunctionCall(ResponseFunctionCallEvent {
                  id: call.call_id,
                  call_type: "function".to_string(),
                  function: ResponseFunctionCall {
                    name: call.name,
                    arguments: call.arguments,
                  },
                  thought_signature: None,
                }));
              }
            }
            _ => {}
          }
        }
      }
      "response.output_text.delta" => {
        let delta = value
          .get("delta")
          .and_then(Value::as_str)
          .unwrap_or_default()
          .to_string();
        if !delta.is_empty() {
          events.push(ResponseEvent::ContentDelta(ResponseContentDeltaEvent {
            text: delta,
            index: *text_index,
          }));
          *text_index += 1;
        }
      }
      "response.reasoning_summary_text.delta" => {
        let delta = value
          .get("delta")
          .and_then(Value::as_str)
          .unwrap_or_default()
          .to_string();
        let summary_index = value
          .get("summary_index")
          .and_then(Value::as_u64)
          .unwrap_or(0) as usize;
        if !delta.is_empty() {
          events.push(ResponseEvent::ReasoningSummaryDelta {
            delta,
            summary_index,
          });
        }
      }
      "response.function_call_arguments.delta" => {
        let item_id = value
          .get("item_id")
          .and_then(Value::as_str)
          .unwrap_or_default()
          .to_string();
        let delta = value
          .get("delta")
          .and_then(Value::as_str)
          .unwrap_or_default();
        if let Some(call) = function_calls.get_mut(&item_id) {
          call.arguments.push_str(delta);
        }
      }
      "response.completed" | "response.incomplete" => {
        let usage = value
          .get("response")
          .and_then(|response| response.get("usage"))
          .and_then(parse_response_usage);
        events.push(ResponseEvent::Completed {
          response_id: response_id.clone(),
          token_usage: usage,
        });
        events.push(ResponseEvent::EndTurn);
      }
      "error" => {
        let message = value
          .get("message")
          .and_then(Value::as_str)
          .unwrap_or("Unknown provider error")
          .to_string();
        events.push(ResponseEvent::Error(ResponseErrorEvent { message }));
      }
      _ => {}
    }
  }

  Ok(events)
}

fn response_rate_limits_from_headers(
  headers: &reqwest::header::HeaderMap,
) -> Option<ResponseRateLimitsSnapshot> {
  let requests_remaining = header_i64(headers, "x-ratelimit-remaining-requests");
  let tokens_remaining = header_i64(headers, "x-ratelimit-remaining-tokens");
  let reset_seconds = header_i64(headers, "x-ratelimit-reset-requests");

  if requests_remaining.is_none() && tokens_remaining.is_none() && reset_seconds.is_none() {
    return None;
  }

  Some(ResponseRateLimitsSnapshot {
    requests_remaining,
    tokens_remaining,
    reset_seconds,
  })
}

fn header_i64(headers: &reqwest::header::HeaderMap, key: &str) -> Option<i64> {
  headers
    .get(key)
    .and_then(|value| value.to_str().ok())
    .and_then(|value| value.parse::<i64>().ok())
}

fn parse_response_usage(value: &Value) -> Option<ResponseTokenUsage> {
  // Normalize provider-native usage details once, so downstream consumers can
  // render a canonical input/cache/reasoning breakdown without re-parsing.
  let prompt_tokens = value
    .get("input_tokens")
    .or_else(|| value.get("prompt_tokens"))
    .or_else(|| value.get("promptTokenCount"))
    .and_then(Value::as_i64)
    .unwrap_or(0);
  let cached_input_tokens = value
    .get("cached_input_tokens")
    .or_else(|| value.get("cached_tokens"))
    .or_else(|| {
      value
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
    })
    .or_else(|| {
      value
        .get("prompt_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
    })
    .or_else(|| value.get("cachedContentTokenCount"))
    .and_then(Value::as_i64)
    .unwrap_or(0);
  let input_tokens = prompt_tokens.saturating_sub(cached_input_tokens);
  let output_tokens = value
    .get("output_tokens")
    .or_else(|| value.get("completion_tokens"))
    .or_else(|| value.get("candidatesTokenCount"))
    .and_then(Value::as_i64)
    .unwrap_or(0);
  let reasoning_output_tokens = value
    .get("reasoning_output_tokens")
    .or_else(|| value.get("reasoning_tokens"))
    .or_else(|| {
      value
        .get("output_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
    })
    .or_else(|| {
      value
        .get("completion_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
    })
    .or_else(|| value.get("thoughtsTokenCount"))
    .and_then(Value::as_i64)
    .unwrap_or(0);
  let total_tokens = value
    .get("total_tokens")
    .or_else(|| value.get("totalTokenCount"))
    .and_then(Value::as_i64)
    .unwrap_or(prompt_tokens + output_tokens + reasoning_output_tokens);

  if input_tokens == 0
    && cached_input_tokens == 0
    && output_tokens == 0
    && reasoning_output_tokens == 0
    && total_tokens == 0
  {
    return None;
  }

  Some(ResponseTokenUsage {
    input_tokens,
    cached_input_tokens,
    output_tokens,
    reasoning_output_tokens,
    total_tokens,
  })
}

#[cfg(test)]
mod tests {
  use pretty_assertions::assert_eq;

  use super::*;

  #[test]
  fn test_openai_usage_parser() {
    let mut parser = OpenAIUsageParser::default();
    parser.parse(r#"data: {"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#);
    let usage = parser.retrieve();
    assert!(usage.is_some());
    let usage = usage.expect("usage");
    assert_eq!(usage.total_tokens, 15);
  }

  #[test]
  fn test_anthropic_usage_parser_message_usage() {
    let mut parser = AnthropicUsageParser::default();
    parser.parse(r#"data: {"message":{"usage":{"input_tokens":12,"output_tokens":8}}}"#);
    let usage = parser.retrieve();
    assert!(usage.is_some());
    let usage = usage.expect("usage");
    assert_eq!(usage.input_tokens, 12);
    assert_eq!(usage.output_tokens, 8);
  }

  #[test]
  fn test_streaming_processor_openai_event() {
    let config = StreamingConfig {
      separator: "\n\n",
      usage_parser: Box::new(OpenAIUsageParser::default()),
      binary_decoder: None,
    };
    let mut processor = StreamingProcessor::new(config);
    let events = processor.push_text(
      "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n",
    );
    assert_eq!(events.len(), 1);
    let chunk = events[0].chunk.clone();
    assert!(matches!(chunk, Some(Chunk::Content { .. })));
  }

  #[test]
  fn test_parse_openai_responses_event_text_and_completed() {
    let raw = concat!(
      "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-5.2-codex\"}}\n",
      "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\",\"id\":\"msg_1\"}}\n",
      "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n",
      "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"id\":\"msg_1\"}}\n",
      "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":22,\"input_tokens_details\":{\"cached_tokens\":3},\"output_tokens_details\":{\"reasoning_tokens\":7}}}}\n",
    );

    let mut response_id = String::new();
    let mut text_index = 0usize;
    let mut function_calls = HashMap::new();
    let events =
      parse_openai_responses_event(raw, &mut response_id, &mut text_index, &mut function_calls)
        .expect("parse responses event");

    assert!(matches!(events[0], ResponseEvent::Created));
    assert!(matches!(events[1], ResponseEvent::ServerModel(_)));
    assert!(matches!(events[2], ResponseEvent::OutputItemAdded(_)));
    assert!(matches!(events[3], ResponseEvent::ContentDelta(_)));
    assert!(matches!(events[4], ResponseEvent::OutputItemDone(_)));
    assert!(matches!(events[5], ResponseEvent::Completed { .. }));
    assert!(matches!(events[6], ResponseEvent::EndTurn));

    let usage = match &events[5] {
      ResponseEvent::Completed {
        token_usage: Some(usage),
        ..
      } => usage,
      _ => panic!("expected completed usage"),
    };
    assert_eq!(
      usage,
      &ResponseTokenUsage {
        input_tokens: 7,
        cached_input_tokens: 3,
        output_tokens: 5,
        reasoning_output_tokens: 7,
        total_tokens: 22,
      }
    );
  }

  #[test]
  fn test_parse_openai_responses_event_function_call() {
    let raw = concat!(
      "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"read_file\",\"arguments\":\"{\"}}\n",
      "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"\\\"path\\\":\\\"Cargo.toml\\\"}\"}\n",
      "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"Cargo.toml\\\"}\",\"status\":\"completed\"}}\n",
    );

    let mut response_id = String::new();
    let mut text_index = 0usize;
    let mut function_calls = HashMap::new();
    let events =
      parse_openai_responses_event(raw, &mut response_id, &mut text_index, &mut function_calls)
        .expect("parse function call event");

    let function_call = events
      .into_iter()
      .find_map(|event| match event {
        ResponseEvent::FunctionCall(call) => Some(call),
        _ => None,
      })
      .expect("function call");
    assert_eq!(function_call.id, "call_1");
    assert_eq!(function_call.function.name, "read_file");
    assert_eq!(
      function_call.function.arguments,
      "{\"path\":\"Cargo.toml\"}"
    );
  }
}
