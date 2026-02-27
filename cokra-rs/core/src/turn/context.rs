// Turn Context
use std::sync::Arc;

use cokra_config::Config;

use crate::session::SessionSource;

/// Turn context - per-turn configuration and state
pub(crate) struct TurnContext {
    /// Submission ID
    pub(crate) sub_id: String,
    /// Configuration
    pub(crate) config: Arc<Config>,
    /// Session source
    pub(crate) session_source: SessionSource,
    /// Current working directory
    pub(crate) cwd: std::path::PathBuf,
    /// Model to use
    pub(crate) model: String,
    /// Approval policy
    pub(crate) approval_policy: String,
    /// Sandbox mode
    pub(crate) sandbox_mode: String,
}

impl TurnContext {
    /// Create a new turn context
    pub(crate) fn new(
        sub_id: String,
        config: Arc<Config>,
        session_source: SessionSource,
    ) -> Self {
        Self {
            sub_id,
            config,
            session_source,
            cwd: std::env::current_dir().unwrap_or_default(),
            model: "gpt-4".to_string(),
            approval_policy: "ask".to_string(),
            sandbox_mode: "permissive".to_string(),
        }
    }

    /// Create with custom settings
    pub(crate) fn with_model(mut self, model: String) -> Self {
        self.model = model;
        self
    }

    /// Create with custom cwd
    pub(crate) fn with_cwd(mut self, cwd: std::path::PathBuf) -> Self {
        self.cwd = cwd;
        self
    }

    /// Get the model context window
    pub fn model_context_window(&self) -> i64 {
        128000 // Default context window
    }
}
