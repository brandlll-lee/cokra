use serde::Deserialize;
use serde::Serialize;

/// Responses API SSE event stream items.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResponseEvent {
  /// Response object is created.
  Created,
  /// Incremental assistant text chunk.
  ContentDelta(ContentDeltaEvent),
  /// New output item started streaming.
  OutputItemAdded(OutputItemEvent),
  /// Output item finalized.
  OutputItemDone(OutputItemEvent),
  /// Model-issued tool call.
  FunctionCall(FunctionCallEvent),
  /// Reasoning summary delta.
  ReasoningSummaryDelta { delta: String, summary_index: usize },
  /// Raw reasoning content delta.
  ReasoningContentDelta { delta: String, content_index: usize },
  /// Provider rate limit snapshot.
  RateLimits(ResponseRateLimitsSnapshot),
  /// Response completed with token usage.
  Completed {
    response_id: String,
    token_usage: Option<ResponseTokenUsage>,
  },
  /// Resolved server-side model id.
  ServerModel(String),
  /// Current model response turn is complete.
  EndTurn,
  /// Provider emitted an error event.
  Error(ResponseErrorEvent),
}

/// Output item lifecycle event payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputItemEvent {
  pub id: String,
  pub role: Option<String>,
  pub item_type: Option<String>,
}

/// Token usage snapshot in response completion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ResponseTokenUsage {
  pub input_tokens: i64,
  pub output_tokens: i64,
  pub total_tokens: i64,
}

/// Simplified rate limit snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ResponseRateLimitsSnapshot {
  pub requests_remaining: Option<i64>,
  pub tokens_remaining: Option<i64>,
  pub reset_seconds: Option<i64>,
}

/// Text delta emitted by model streaming.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContentDeltaEvent {
  pub text: String,
  pub index: usize,
}

/// Function call event emitted by model streaming.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionCallEvent {
  pub id: String,
  pub call_type: String,
  pub function: FunctionCall,
  /// Google Gemini 3 thought signature for preserving reasoning state
  /// Must be passed back in subsequent requests for multi-turn function calling
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub thought_signature: Option<String>,
}

/// Function call payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionCall {
  pub name: String,
  pub arguments: String,
}

/// Provider-side error event in responses stream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResponseErrorEvent {
  pub message: String,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn response_event_roundtrip_content_delta() {
    let event = ResponseEvent::ContentDelta(ContentDeltaEvent {
      text: "hello".to_string(),
      index: 1,
    });

    let json = serde_json::to_string(&event).expect("serialize response event");
    let parsed: ResponseEvent = serde_json::from_str(&json).expect("deserialize response event");
    assert_eq!(parsed, event);
  }

  #[test]
  fn response_event_roundtrip_function_call() {
    let event = ResponseEvent::FunctionCall(FunctionCallEvent {
      id: "call_1".to_string(),
      call_type: "function".to_string(),
      function: FunctionCall {
        name: "read_file".to_string(),
        arguments: r#"{"file_path":"demo.txt"}"#.to_string(),
      },
      thought_signature: None,
    });

    let json = serde_json::to_string(&event).expect("serialize response event");
    let parsed: ResponseEvent = serde_json::from_str(&json).expect("deserialize response event");
    assert_eq!(parsed, event);
  }

  #[test]
  fn response_event_roundtrip_completed() {
    let event = ResponseEvent::Completed {
      response_id: "resp_1".to_string(),
      token_usage: Some(ResponseTokenUsage {
        input_tokens: 10,
        output_tokens: 5,
        total_tokens: 15,
      }),
    };

    let json = serde_json::to_string(&event).expect("serialize response event");
    let parsed: ResponseEvent = serde_json::from_str(&json).expect("deserialize response event");
    assert_eq!(parsed, event);
  }
}
