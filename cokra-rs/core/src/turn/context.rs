//! Turn Context
//!
//! Provides context and services for turn execution.

use std::path::PathBuf;
use std::sync::Arc;

use cokra_config::{ApprovalPolicy, Config, SandboxMode};
use cokra_protocol::{ReadOnlyAccess, SandboxPolicy};

use crate::model::ModelClient;
use crate::session::Session;
use crate::tools::registry::ToolRegistry;

pub struct TurnContext {
  pub session: Arc<Session>,
  pub model_client: Arc<ModelClient>,
  pub tool_registry: Arc<ToolRegistry>,
  pub cwd: PathBuf,
  pub approval_policy: ApprovalPolicy,
  pub sandbox_policy: SandboxPolicy,
  pub enable_tools: bool,
  pub max_tokens: Option<u32>,
  pub temperature: Option<f32>,
}

impl TurnContext {
  pub fn new(
    session: Arc<Session>,
    model_client: Arc<ModelClient>,
    tool_registry: Arc<ToolRegistry>,
    config: &Config,
  ) -> Self {
    Self {
      session,
      model_client,
      tool_registry,
      cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
      approval_policy: config.approval.clone(),
      sandbox_policy: map_sandbox_policy(config),
      enable_tools: true,
      max_tokens: None,
      temperature: None,
    }
  }

  pub fn with_cwd(mut self, cwd: PathBuf) -> Self {
    self.cwd = cwd;
    self
  }

  pub fn with_approval_policy(mut self, policy: ApprovalPolicy) -> Self {
    self.approval_policy = policy;
    self
  }

  pub fn with_sandbox_policy(mut self, policy: SandboxPolicy) -> Self {
    self.sandbox_policy = policy;
    self
  }

  pub fn with_temperature(mut self, temp: f32) -> Self {
    self.temperature = Some(temp);
    self
  }

  pub fn with_max_tokens(mut self, max: u32) -> Self {
    self.max_tokens = Some(max);
    self
  }

  pub fn without_tools(mut self) -> Self {
    self.enable_tools = false;
    self
  }
}

fn map_sandbox_policy(config: &Config) -> SandboxPolicy {
  match config.sandbox.mode {
    SandboxMode::Strict => SandboxPolicy::ReadOnly {
      access: ReadOnlyAccess::FullAccess,
    },
    SandboxMode::Permissive => SandboxPolicy::WorkspaceWrite {
      writable_roots: vec![
        std::env::current_dir()
          .unwrap_or_else(|_| PathBuf::from("."))
          .display()
          .to_string(),
      ],
      read_only_access: ReadOnlyAccess::FullAccess,
      network_access: config.sandbox.network_access,
      exclude_tmpdir_env_var: false,
      exclude_slash_tmp: false,
    },
    SandboxMode::DangerFullAccess => SandboxPolicy::DangerFullAccess,
  }
}
