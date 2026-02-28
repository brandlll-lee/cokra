//! Regular Task
//!
//! Standard conversation flow task

use crate::turn::{
  CancellationToken, SessionTask, TaskKind, TaskMetadata, TurnConfig, TurnContext, TurnError,
  TurnExecutor, UserInput,
};
use async_trait::async_trait;
use cokra_protocol::AgentMessageEvent;
use tokio::sync::mpsc;

/// Regular conversation task
///
/// Handles the standard user-assistant interaction flow.
pub struct RegularTask {
  metadata: TaskMetadata,
  user_input: UserInput,
  cancellation_token: CancellationToken,
  executor: Option<TurnExecutor>,
}

impl RegularTask {
  /// Create a new regular task
  pub fn new(user_input: UserInput) -> Self {
    let id = uuid::Uuid::new_v4().to_string();
    let cancellation_token = CancellationToken::new();

    let mut metadata = TaskMetadata::new(&id, TaskKind::Regular);
    metadata.cancellation_token = Some(cancellation_token.clone());

    Self {
      metadata,
      user_input,
      cancellation_token,
      executor: None,
    }
  }

  /// Create with executor
  pub fn with_executor(mut self, executor: TurnExecutor) -> Self {
    self.executor = Some(executor);
    self
  }

  /// Set the executor
  pub fn set_executor(&mut self, executor: TurnExecutor) {
    self.executor = Some(executor);
  }

  /// Get the user input
  pub fn user_input(&self) -> &UserInput {
    &self.user_input
  }

  /// Check if cancelled
  pub fn is_cancelled(&self) -> bool {
    self.cancellation_token.is_cancelled()
  }

  /// Cancel the task
  pub fn cancel(&self) {
    self.cancellation_token.cancel();
  }
}

#[async_trait]
impl SessionTask for RegularTask {
  async fn run(&mut self, cx: TurnContext) -> Result<Option<AgentMessageEvent>, TurnError> {
    // Check for cancellation
    if self.is_cancelled() {
      return Ok(None);
    }

    // Create executor if not set
    let executor = if let Some(exec) = &self.executor {
      exec.clone()
    } else {
      let config = TurnConfig {
        model: TurnConfig::default().model,
        temperature: cx.temperature,
        max_tokens: cx.max_tokens,
        system_prompt: None,
        enable_tools: cx.enable_tools,
      };

      let (tx_event, _rx_event) = mpsc::channel(256);

      TurnExecutor::new(
        cx.model_client.clone(),
        cx.tool_registry.clone(),
        cx.session.clone(),
        tx_event,
        config,
      )
    };

    // Execute the turn
    let result = executor.run_turn(self.user_input.clone()).await?;

    // Convert to AgentMessageEvent
    let message = AgentMessageEvent {
      thread_id: uuid::Uuid::new_v4().to_string(),
      turn_id: uuid::Uuid::new_v4().to_string(),
      item_id: uuid::Uuid::new_v4().to_string(),
      content: vec![cokra_protocol::AgentMessageContent::Text {
        text: result.content,
      }],
    };

    Ok(Some(message))
  }

  fn task_kind(&self) -> TaskKind {
    TaskKind::Regular
  }

  fn task_id(&self) -> &str {
    &self.metadata.id
  }

  async fn cancel(&mut self) -> Result<(), TurnError> {
    self.cancellation_token.cancel();
    Ok(())
  }
}

impl From<UserInput> for RegularTask {
  fn from(input: UserInput) -> Self {
    Self::new(input)
  }
}
