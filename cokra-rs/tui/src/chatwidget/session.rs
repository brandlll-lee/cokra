use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TokenUsage {
  pub input_tokens: i64,
  pub cached_input_tokens: i64,
  pub output_tokens: i64,
  pub reasoning_output_tokens: i64,
  pub total_tokens: i64,
}

impl TokenUsage {
  pub fn is_zero(&self) -> bool {
    self.input_tokens == 0
      && self.cached_input_tokens == 0
      && self.output_tokens == 0
      && self.reasoning_output_tokens == 0
      && self.total_tokens == 0
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StatusSnapshot {
  pub(super) header: String,
  pub(super) details: Option<String>,
  pub(super) inline_message: Option<String>,
}

impl StatusSnapshot {
  pub(super) fn new(
    header: impl Into<String>,
    details: Option<String>,
    inline_message: Option<String>,
  ) -> Self {
    Self {
      header: header.into(),
      details,
      inline_message,
    }
  }

  pub(super) fn working() -> Self {
    Self::new("Working", None, None)
  }
}

#[derive(Debug, Default)]
pub(super) struct SessionState {
  pub(super) token_usage: TokenUsage,
  pub(super) context_used_tokens: Option<i64>,
  pub(super) model_name: String,
  pub(super) cwd: Option<PathBuf>,
  pub(super) agent_turn_running: bool,
  pub(super) has_seen_session_configured: bool,
  pub(super) last_separator_elapsed_secs: Option<u64>,
  pub(super) last_wait_end_fingerprint: Option<u64>,
  pub(super) reasoning_buffer: String,
  pub(super) collab_wait_status: Option<StatusSnapshot>,
  pub(super) collab_compact_mode: bool,
  pub(super) active_status_override: Option<StatusSnapshot>,
  pub(super) mcp_starting_servers: BTreeSet<String>,
}

impl SessionState {
  pub(super) fn cwd(&self) -> Option<&PathBuf> {
    self.cwd.as_ref()
  }

  pub(super) fn model_name(&self) -> &str {
    &self.model_name
  }

  pub(super) fn set_model_name(&mut self, model_name: String) {
    self.model_name = model_name;
  }

  pub(super) fn token_usage(&self) -> TokenUsage {
    self.token_usage
  }

  pub(super) fn context_used_tokens(&self) -> Option<i64> {
    self.context_used_tokens
  }

  pub(super) fn worked_elapsed_from(&mut self, current_elapsed: u64) -> u64 {
    let baseline = match self.last_separator_elapsed_secs {
      Some(last) if current_elapsed < last => 0,
      Some(last) => last,
      None => 0,
    };
    let elapsed = current_elapsed.saturating_sub(baseline);
    self.last_separator_elapsed_secs = Some(current_elapsed);
    elapsed
  }

  pub(super) fn reset_turn_status(&mut self) {
    self.reasoning_buffer.clear();
    self.collab_wait_status = None;
    self.active_status_override = None;
    self.mcp_starting_servers.clear();
  }
}

#[cfg(test)]
mod tests {
  use std::collections::BTreeSet;

  use super::SessionState;
  use super::StatusSnapshot;

  #[test]
  fn worked_elapsed_uses_last_separator_as_baseline() {
    let mut state = SessionState::default();

    assert_eq!(state.worked_elapsed_from(42), 42);
    assert_eq!(state.worked_elapsed_from(105), 63);

    // Guard against timer resets or resumed sessions reporting a smaller elapsed.
    assert_eq!(state.worked_elapsed_from(3), 3);
  }

  #[test]
  fn reset_turn_status_clears_dynamic_status_state() {
    let mut state = SessionState {
      reasoning_buffer: "**Analyzing**".to_string(),
      collab_wait_status: Some(StatusSnapshot::new("Waiting for agents", None, None)),
      active_status_override: Some(StatusSnapshot::new("Searching the web", None, None)),
      mcp_starting_servers: BTreeSet::from(["filesystem".to_string()]),
      ..SessionState::default()
    };

    state.reset_turn_status();

    assert!(state.reasoning_buffer.is_empty());
    assert!(state.collab_wait_status.is_none());
    assert!(state.active_status_override.is_none());
    assert!(state.mcp_starting_servers.is_empty());
  }
}
