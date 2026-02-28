//! Unified streaming helpers.

use serde_json::Value;

use super::types::{Chunk, ContentDelta, ToolCallDelta, Usage};

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

#[cfg(test)]
mod tests {
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
}
