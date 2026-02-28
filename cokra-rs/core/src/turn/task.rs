//! Session Task trait
//!
//! Defines the interface for different types of session tasks

use crate::turn::TurnContext;
use crate::turn::TurnError;
use async_trait::async_trait;
use cokra_protocol::AgentMessageEvent;

/// Result of running a task
pub type TaskResult = Result<Option<AgentMessageEvent>, TurnError>;

/// Session task trait
///
/// All task types must implement this trait.
#[async_trait]
pub trait SessionTask: Send {
  /// Run the task
  async fn run(&mut self, cx: TurnContext) -> TaskResult;

  /// Get the task kind
  fn task_kind(&self) -> TaskKind;

  /// Get the task ID
  fn task_id(&self) -> &str;

  /// Cancel the task
  async fn cancel(&mut self) -> Result<(), TurnError>;
}

/// Kinds of session tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
  /// Regular conversation task
  Regular,

  /// Code review task
  Review,

  /// Context compaction task
  Compact,

  /// Undo/rollback task
  GhostSnapshot,

  /// Custom task
  Custom,
}

/// Task metadata
#[derive(Debug, Clone)]
pub struct TaskMetadata {
  /// Unique task ID
  pub id: String,

  /// Task kind
  pub kind: TaskKind,

  /// Creation timestamp
  pub created_at: u64,

  /// Cancellation token
  pub cancellation_token: Option<CancellationToken>,
}

impl TaskMetadata {
  /// Create new task metadata
  pub fn new(id: impl Into<String>, kind: TaskKind) -> Self {
    Self {
      id: id.into(),
      kind,
      created_at: chrono::Utc::now().timestamp() as u64,
      cancellation_token: None,
    }
  }
}

/// Cancellation token for cancelling tasks
#[derive(Debug, Clone)]
pub struct CancellationToken {
  cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl CancellationToken {
  /// Create a new cancellation token
  pub fn new() -> Self {
    Self {
      cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    }
  }

  /// Check if cancelled
  pub fn is_cancelled(&self) -> bool {
    self.cancelled.load(std::sync::atomic::Ordering::Relaxed)
  }

  /// Cancel the task
  pub fn cancel(&self) {
    self
      .cancelled
      .store(true, std::sync::atomic::Ordering::Relaxed);
  }
}

impl Default for CancellationToken {
  fn default() -> Self {
    Self::new()
  }
}
