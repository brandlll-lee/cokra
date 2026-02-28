// Turn Items
// Item types for turn content

use serde::{Deserialize, Serialize};

/// Agent status enum
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentStatus {
  /// Waiting for initialization
  PendingInit,
  /// Currently executing
  Running,
  /// Done with final message
  Completed(Option<String>),
  /// Encountered error
  Errored(String),
  /// Shut down
  Shutdown,
  /// Agent not found
  NotFound,
}

/// Token usage tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
  pub input_tokens: i64,
  pub cached_input_tokens: i64,
  pub output_tokens: i64,
  pub reasoning_output_tokens: i64,
  pub total_tokens: i64,
}

impl TokenUsage {
  pub fn new() -> Self {
    Self {
      input_tokens: 0,
      cached_input_tokens: 0,
      output_tokens: 0,
      reasoning_output_tokens: 0,
      total_tokens: 0,
    }
  }

  pub fn blended_total(&self) -> i64 {
    self.input_tokens - self.cached_input_tokens + self.output_tokens
  }
}

impl Default for TokenUsage {
  fn default() -> Self {
    Self::new()
  }
}
