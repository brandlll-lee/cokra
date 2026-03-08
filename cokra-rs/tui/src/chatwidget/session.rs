use std::path::PathBuf;

#[derive(Debug, Default, Clone, Copy)]
pub struct TokenUsage {
  pub input_tokens: i64,
  pub output_tokens: i64,
  pub total_tokens: i64,
}

impl TokenUsage {
  pub fn is_zero(&self) -> bool {
    self.input_tokens == 0 && self.output_tokens == 0 && self.total_tokens == 0
  }
}

#[derive(Debug, Default)]
pub(super) struct SessionState {
  pub(super) token_usage: TokenUsage,
  pub(super) model_name: String,
  pub(super) cwd: Option<PathBuf>,
  pub(super) agent_turn_running: bool,
  pub(super) has_seen_session_configured: bool,
  pub(super) last_separator_elapsed_secs: Option<u64>,
  pub(super) last_wait_end_fingerprint: Option<u64>,
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
}

#[cfg(test)]
mod tests {
  use super::SessionState;

  #[test]
  fn worked_elapsed_uses_last_separator_as_baseline() {
    let mut state = SessionState::default();

    assert_eq!(state.worked_elapsed_from(42), 42);
    assert_eq!(state.worked_elapsed_from(105), 63);

    // Guard against timer resets or resumed sessions reporting a smaller elapsed.
    assert_eq!(state.worked_elapsed_from(3), 3);
  }
}
