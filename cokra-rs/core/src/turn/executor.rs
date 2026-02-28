//! Turn Executor
//!
//! Executes a turn (one user interaction cycle) in a Cokra session.

use std::sync::Arc;

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::model::{ChatRequest, Message as ModelMessage, ModelClient};
use crate::session::Session;
use crate::tools::context::{ToolInvocation, ToolOutput};
use crate::tools::registry::ToolRegistry;
use cokra_protocol::{
  CompletionStatus, ErrorEvent, EventMsg, ItemCompletedEvent, ItemStartedEvent, ModeKind,
  TurnCompleteEvent, TurnStartedEvent,
};

type Event = cokra_protocol::EventMsg;

/// Turn executor errors
#[derive(Debug, thiserror::Error)]
pub enum TurnError {
  #[error("Model error: {0}")]
  ModelError(#[from] crate::model::ModelError),

  #[error("Tool error: {0}")]
  ToolError(String),

  #[error("Tool not found: {0}")]
  ToolNotFound(String),

  #[error("Session error: {0}")]
  SessionError(String),
}

/// Turn execution result
#[derive(Debug, Clone)]
pub struct TurnResult {
  /// Final assistant text.
  pub content: String,
  /// Token usage summary.
  pub usage: crate::model::Usage,
  /// Whether the run completed successfully.
  pub success: bool,
}

/// Turn configuration
#[derive(Debug, Clone)]
pub struct TurnConfig {
  pub model: String,
  pub temperature: Option<f32>,
  pub max_tokens: Option<u32>,
  pub system_prompt: Option<String>,
  pub enable_tools: bool,
}

impl Default for TurnConfig {
  fn default() -> Self {
    Self {
      model: "gpt-4o".to_string(),
      temperature: Some(0.2),
      max_tokens: Some(4096),
      system_prompt: None,
      enable_tools: true,
    }
  }
}

#[derive(Clone)]
pub struct TurnExecutor {
  model_client: Arc<ModelClient>,
  tool_registry: Arc<ToolRegistry>,
  session: Arc<Session>,
  tx_event: mpsc::Sender<Event>,
  config: TurnConfig,
}

impl TurnExecutor {
  pub fn new(
    model_client: Arc<ModelClient>,
    tool_registry: Arc<ToolRegistry>,
    session: Arc<Session>,
    tx_event: mpsc::Sender<Event>,
    config: TurnConfig,
  ) -> Self {
    Self {
      model_client,
      tool_registry,
      session,
      tx_event,
      config,
    }
  }

  pub async fn run_turn(&self, input: UserInput) -> Result<TurnResult, TurnError> {
    let thread_id = self
      .session
      .id()
      .unwrap_or_else(|| Uuid::new_v4().to_string());
    let turn_id = Uuid::new_v4().to_string();
    let item_id = Uuid::new_v4().to_string();

    self
      .send_event(EventMsg::TurnStarted(TurnStartedEvent {
        thread_id: thread_id.clone(),
        turn_id: turn_id.clone(),
        mode: ModeKind::Default,
        model: self.config.model.clone(),
        start_time: chrono::Utc::now().timestamp(),
      }))
      .await?;

    self
      .session
      .append_message(ModelMessage::User(input.content.clone()))
      .await;

    let mut messages = self.build_messages(input).await?;
    let mut final_content = String::new();
    let mut total_usage = crate::model::Usage::default();
    let mut success = false;

    let max_iterations = 10;
    let mut iteration = 0;

    loop {
      iteration += 1;
      if iteration > max_iterations {
        return Err(TurnError::SessionError(
          "too many tool call iterations".to_string(),
        ));
      }

      self
        .send_event(EventMsg::ItemStarted(ItemStartedEvent {
          thread_id: thread_id.clone(),
          turn_id: turn_id.clone(),
          item_id: item_id.clone(),
          item_type: "agent-message".to_string(),
        }))
        .await?;

      let request = ChatRequest {
        model: self.config.model.clone(),
        messages: messages.clone(),
        temperature: self.config.temperature,
        max_tokens: self.config.max_tokens,
        tools: if self.config.enable_tools {
          Some(self.tool_registry.model_tools())
        } else {
          None
        },
        ..Default::default()
      };

      let response = match self.model_client.chat(request).await {
        Ok(r) => r,
        Err(e) => {
          self
            .send_event(EventMsg::Error(ErrorEvent {
              thread_id: thread_id.clone(),
              turn_id: turn_id.clone(),
              error: e.to_string(),
              user_facing_message: format!("Model error: {e}"),
              details: format!("{e:?}"),
            }))
            .await?;
          return Err(TurnError::ModelError(e));
        }
      };

      let choice = response
        .choices
        .first()
        .cloned()
        .ok_or_else(|| TurnError::SessionError("model returned no choices".to_string()))?;

      if let Some(content) = choice.message.content.clone() {
        final_content.push_str(&content);
      }

      total_usage.input_tokens += response.usage.input_tokens;
      total_usage.output_tokens += response.usage.output_tokens;
      total_usage.total_tokens += response.usage.total_tokens;

      let assistant_message = ModelMessage::Assistant {
        content: choice.message.content.clone(),
        tool_calls: choice.message.tool_calls.clone(),
      };
      messages.push(assistant_message.clone());
      self.session.append_message(assistant_message).await;

      let tool_calls = choice.message.tool_calls.unwrap_or_default();
      if tool_calls.is_empty() {
        success = true;
        break;
      }

      for tool_call in tool_calls {
        let output = self.execute_tool(tool_call.clone()).await?;
        let tool_msg = ModelMessage::Tool {
          tool_call_id: output.id.clone(),
          content: output.content.clone(),
        };
        messages.push(tool_msg.clone());
        self.session.append_message(tool_msg).await;
      }
    }

    self
      .send_event(EventMsg::ItemCompleted(ItemCompletedEvent {
        thread_id: thread_id.clone(),
        turn_id: turn_id.clone(),
        item_id,
        result: final_content.clone(),
      }))
      .await?;

    self
      .send_event(EventMsg::TurnComplete(TurnCompleteEvent {
        thread_id,
        turn_id,
        status: CompletionStatus::Success,
        end_time: chrono::Utc::now().timestamp(),
      }))
      .await?;

    Ok(TurnResult {
      content: final_content,
      usage: total_usage,
      success,
    })
  }

  async fn execute_tool(&self, tool_call: crate::model::ToolCall) -> Result<ToolOutput, TurnError> {
    let handler = self
      .tool_registry
      .get_handler(&tool_call.function.name)
      .ok_or_else(|| TurnError::ToolNotFound(tool_call.function.name.clone()))?;

    let invocation = ToolInvocation {
      id: tool_call.id.clone(),
      name: tool_call.function.name.clone(),
      arguments: tool_call.function.arguments.clone(),
    };

    let mut output = handler
      .handle(invocation)
      .map_err(|e| TurnError::ToolError(e.to_string()))?;

    if output.id.is_empty() {
      output.id = tool_call.id;
    }

    Ok(output)
  }

  async fn build_messages(&self, input: UserInput) -> Result<Vec<ModelMessage>, TurnError> {
    let mut messages = Vec::new();

    if let Some(system) = &self.config.system_prompt {
      messages.push(ModelMessage::System(system.clone()));
    }

    let history = self.session.get_history(100).await;
    messages.extend(history);
    messages.push(ModelMessage::User(input.content));

    Ok(messages)
  }

  async fn send_event(&self, event: Event) -> Result<(), TurnError> {
    self.session.emit_event(event.clone());
    self
      .tx_event
      .send(event)
      .await
      .map_err(|e| TurnError::SessionError(format!("failed to send event: {e}")))
  }
}

#[derive(Debug, Clone)]
pub struct UserInput {
  pub content: String,
  pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone)]
pub struct Attachment {
  pub kind: AttachmentKind,
  pub data: Vec<u8>,
  pub mime_type: String,
}

#[derive(Debug, Clone)]
pub enum AttachmentKind {
  Image,
  File,
  PDF,
  Audio,
}
