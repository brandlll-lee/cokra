use serde::{Deserialize, Serialize};

/// Responses API SSE event stream items.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResponseEvent {
  /// Incremental assistant text chunk.
  ContentDelta(ContentDeltaEvent),
  /// Model-issued tool call.
  FunctionCall(FunctionCallEvent),
  /// Current model response turn is complete.
  EndTurn,
  /// Provider emitted an error event.
  Error(ResponseErrorEvent),
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
    });

    let json = serde_json::to_string(&event).expect("serialize response event");
    let parsed: ResponseEvent = serde_json::from_str(&json).expect("deserialize response event");
    assert_eq!(parsed, event);
  }
}
