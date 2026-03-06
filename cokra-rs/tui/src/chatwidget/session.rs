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
}
