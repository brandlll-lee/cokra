use crate::model::Message as ModelMessage;
use cokra_protocol::FunctionCallEvent;

/// Unified response item abstraction aligned with codex-style turn data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseItem {
  Message {
    role: String,
    content: String,
  },
  FunctionCall {
    id: String,
    name: String,
    arguments: String,
  },
  FunctionCallOutput {
    call_id: String,
    output: String,
    is_error: bool,
  },
}

impl ResponseItem {
  pub fn from_model_message(msg: &ModelMessage) -> Option<Self> {
    match msg {
      ModelMessage::System(content) => Some(Self::Message {
        role: "system".to_string(),
        content: content.clone(),
      }),
      ModelMessage::User(content) => Some(Self::Message {
        role: "user".to_string(),
        content: content.clone(),
      }),
      ModelMessage::Assistant { content, .. } => Some(Self::Message {
        role: "assistant".to_string(),
        content: content.clone().unwrap_or_default(),
      }),
      ModelMessage::Tool {
        tool_call_id,
        content,
      } => Some(Self::FunctionCallOutput {
        call_id: tool_call_id.clone(),
        output: content.clone(),
        is_error: false,
      }),
    }
  }

  pub fn from_function_call_event(call: &FunctionCallEvent) -> Self {
    Self::FunctionCall {
      id: call.id.clone(),
      name: call.function.name.clone(),
      arguments: call.function.arguments.clone(),
    }
  }
}
