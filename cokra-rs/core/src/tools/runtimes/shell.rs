//! 1:1 codex: Shell runtime — delegates execution to the unified exec layer.
//!
//! This module implements `ToolRuntime` for shell commands, replacing the old
//! `ShellHandler` that directly spawned processes.
//!
//! Two request types (1:1 codex):
//! - `ShellCommandRequest`: a command string that needs `derive_exec_args()`
//! - `ShellRequest`: a pre-built argv `Vec<String>` (already split)

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;

use crate::exec::ExecError;
use crate::exec::ExecExpiration;
use crate::exec::ExecParams;
use crate::exec::ExecToolCallOutput;
use crate::exec::PermissionProfile;
use crate::exec::SandboxPermissions;
use crate::exec::WindowsSandboxLevel;
use crate::exec::execute_command;
use crate::exec_policy::eval_exec_approval;
use crate::sandbox_manager::CommandSpec;
use crate::sandbox_manager::SandboxManager;
use crate::sandbox_manager::SandboxTransformRequest;
use crate::shell::Shell;
use crate::tools::sandboxing::Approvable;
use crate::tools::sandboxing::ApprovalCtx;
use crate::tools::sandboxing::ExecApprovalRequirement;
use crate::tools::sandboxing::SandboxAttempt;
use crate::tools::sandboxing::SandboxKind;
use crate::tools::sandboxing::Sandboxable;
use crate::tools::sandboxing::SandboxablePreference;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::sandboxing::ToolError;
use crate::tools::sandboxing::ToolRuntime;
use cokra_protocol::AskForApproval;
use cokra_protocol::ReviewDecision;

// ---------------------------------------------------------------------------
// ShellCommandRequest (Spec 3: cmd:String + login, needs derive_exec_args)
// ---------------------------------------------------------------------------

/// A shell command request where the command is a string that needs to be
/// passed through `Shell::derive_exec_args()` to produce the actual argv.
#[derive(Debug, Clone, Serialize)]
pub struct ShellCommandRequest {
  /// The command string (e.g. "pwd", "ls -la").
  pub command: String,
  /// Working directory override.
  pub cwd: PathBuf,
  /// Timeout in milliseconds.
  pub timeout_ms: Option<u64>,
  /// Extra environment variables.
  pub env: HashMap<String, String>,
  /// Justification for the command.
  pub justification: Option<String>,
  /// Suggested reusable escalation prefix.
  pub prefix_rule: Option<Vec<String>>,
  /// Sandbox permission mode.
  pub sandbox_permissions: SandboxPermissions,
  /// Additional sandbox permissions.
  pub additional_permissions: Option<PermissionProfile>,
}

// ---------------------------------------------------------------------------
// ShellRequest (Spec 3: pre-built argv Vec<String>)
// ---------------------------------------------------------------------------

/// A shell request where the argv is already constructed.
#[derive(Debug, Clone, Serialize)]
pub struct ShellRequest {
  /// Full argv (program + arguments).
  pub command: Vec<String>,
  /// Working directory.
  pub cwd: PathBuf,
  /// Timeout in milliseconds.
  pub timeout_ms: Option<u64>,
  /// Extra environment variables.
  pub env: HashMap<String, String>,
  /// Justification for the command.
  pub justification: Option<String>,
  /// Suggested reusable escalation prefix.
  pub prefix_rule: Option<Vec<String>>,
  /// Sandbox permission mode.
  pub sandbox_permissions: SandboxPermissions,
  /// Additional sandbox permissions.
  pub additional_permissions: Option<PermissionProfile>,
}

// ---------------------------------------------------------------------------
// ShellRuntime (implements ToolRuntime for ShellCommandRequest)
// ---------------------------------------------------------------------------

/// Runtime that executes shell commands through the unified exec layer.
///
/// This replaces the old `ShellHandler` that directly spawned processes.
pub struct ShellRuntime {
  shell: Shell,
  approval_policy: AskForApproval,
}

impl ShellRuntime {
  pub fn new(shell: Shell, approval_policy: AskForApproval) -> Self {
    Self {
      shell,
      approval_policy,
    }
  }

  /// Build `ExecParams` from a `ShellCommandRequest`.
  fn build_exec_params(&self, req: &ShellCommandRequest) -> ExecParams {
    let argv = self.shell.derive_exec_args(&req.command, true);
    self.build_exec_params_for_argv(
      argv,
      req.cwd.clone(),
      req.timeout_ms,
      req.env.clone(),
      req.justification.clone(),
      req.prefix_rule.clone(),
      req.sandbox_permissions,
      req.additional_permissions.clone(),
    )
  }

  fn build_exec_params_for_argv(
    &self,
    command: Vec<String>,
    cwd: PathBuf,
    timeout_ms: Option<u64>,
    env: HashMap<String, String>,
    justification: Option<String>,
    prefix_rule: Option<Vec<String>>,
    sandbox_permissions: SandboxPermissions,
    additional_permissions: Option<PermissionProfile>,
  ) -> ExecParams {
    let timeout = timeout_ms
      .map(|ms| ExecExpiration::Timeout(Duration::from_millis(ms)))
      .unwrap_or(ExecExpiration::DefaultTimeout);

    ExecParams {
      command,
      cwd,
      expiration: timeout,
      env,
      network: None,
      network_attempt_id: None,
      sandbox_permissions,
      additional_permissions,
      windows_sandbox_level: WindowsSandboxLevel::Disabled,
      justification,
      prefix_rule,
      arg0: None,
    }
  }
}

#[async_trait]
impl Approvable<ShellRequest> for ShellRuntime {
  type ApprovalKey = String;

  fn approval_keys(&self, req: &ShellRequest) -> Vec<Self::ApprovalKey> {
    vec![format!("shell:{}", req.command.join(" "))]
  }

  fn exec_approval_requirement(
    &self,
    req: &ShellRequest,
  ) -> Option<ExecApprovalRequirement> {
    Some(eval_exec_approval(
      &req.command,
      &cokra_protocol::SandboxPolicy::DangerFullAccess,
      self.approval_policy.clone(),
      req.sandbox_permissions,
    ))
  }

  async fn start_approval_async(
    &mut self,
    req: &ShellRequest,
    ctx: ApprovalCtx<'_>,
  ) -> ReviewDecision {
    ctx
      .session
      .request_exec_approval(
        ctx.turn.thread_id.clone(),
        ctx.turn.turn_id.clone(),
        ctx.call_id.to_string(),
        "shell".to_string(),
        req.command.join(" "),
        ctx.turn.cwd.clone(),
        ctx.turn.tx_event.clone(),
      )
      .await
  }
}

#[async_trait]
impl Approvable<ShellCommandRequest> for ShellRuntime {
  type ApprovalKey = String;

  fn approval_keys(&self, req: &ShellCommandRequest) -> Vec<Self::ApprovalKey> {
    vec![format!("shell:{}", req.command)]
  }

  fn exec_approval_requirement(
    &self,
    req: &ShellCommandRequest,
  ) -> Option<ExecApprovalRequirement> {
    let argv = self.shell.derive_exec_args(&req.command, true);
    Some(eval_exec_approval(
      &argv,
      &cokra_protocol::SandboxPolicy::DangerFullAccess,
      self.approval_policy.clone(),
      req.sandbox_permissions,
    ))
  }

  async fn start_approval_async(
    &mut self,
    req: &ShellCommandRequest,
    ctx: ApprovalCtx<'_>,
  ) -> ReviewDecision {
    // 1:1 codex: always block until the user responds.
    ctx
      .session
      .request_exec_approval(
        ctx.turn.thread_id.clone(),
        ctx.turn.turn_id.clone(),
        ctx.call_id.to_string(),
        "shell".to_string(),
        req.command.clone(),
        ctx.turn.cwd.clone(),
        ctx.turn.tx_event.clone(),
      )
      .await
  }
}

impl Sandboxable for ShellRuntime {
  fn sandbox_preference(&self) -> SandboxablePreference {
    SandboxablePreference::Auto
  }

  fn escalate_on_failure(&self) -> bool {
    true
  }
}

#[async_trait]
impl ToolRuntime<ShellCommandRequest, ExecToolCallOutput> for ShellRuntime {
  async fn run(
    &mut self,
    req: &ShellCommandRequest,
    attempt: &SandboxAttempt<'_>,
    _ctx: &ToolCtx<'_>,
  ) -> Result<ExecToolCallOutput, ToolError> {
    let exec_params = self.build_exec_params(req);

    // Spec 2: sandbox transform
    let transform_result = SandboxManager::transform(SandboxTransformRequest {
      command_spec: CommandSpec {
        command: exec_params.command.clone(),
        cwd: exec_params.cwd.clone(),
        env: exec_params.env.clone(),
        expiration: exec_params.expiration.clone(),
        sandbox_permissions: exec_params.sandbox_permissions,
        additional_permissions: exec_params.additional_permissions.clone(),
        windows_sandbox_level: exec_params.windows_sandbox_level,
        network: exec_params.network,
        network_attempt_id: exec_params.network_attempt_id.clone(),
        justification: exec_params.justification.clone(),
        prefix_rule: exec_params.prefix_rule.clone(),
        arg0: exec_params.arg0.clone(),
      },
      policy: attempt.policy.clone(),
    })
    .map_err(|e| ToolError::Execution(e.to_string()))?;

    // Spec 1: execute through unified exec layer
    let output = execute_command(&transform_result.exec_params)
      .await
      .map_err(|e| match e {
        ExecError::SpawnFailed { message, .. } => {
          // message already contains os error from std::io::Error Display.
          if attempt.sandbox != SandboxKind::None && looks_like_sandbox_denial(&message) {
            ToolError::SandboxDenied {
              output: message,
              network_policy_reason: None,
            }
          } else {
            ToolError::Execution(message)
          }
        }
        ExecError::SandboxDenied { output } => ToolError::SandboxDenied {
          output,
          network_policy_reason: None,
        },
        ExecError::Other(msg) => ToolError::Execution(msg),
      })?;

    Ok(output)
  }
}

#[async_trait]
impl ToolRuntime<ShellRequest, ExecToolCallOutput> for ShellRuntime {
  async fn run(
    &mut self,
    req: &ShellRequest,
    attempt: &SandboxAttempt<'_>,
    _ctx: &ToolCtx<'_>,
  ) -> Result<ExecToolCallOutput, ToolError> {
    let exec_params = self.build_exec_params_for_argv(
      req.command.clone(),
      req.cwd.clone(),
      req.timeout_ms,
      req.env.clone(),
      req.justification.clone(),
      req.prefix_rule.clone(),
      req.sandbox_permissions,
      req.additional_permissions.clone(),
    );

    let transform_result = SandboxManager::transform(SandboxTransformRequest {
      command_spec: CommandSpec {
        command: exec_params.command.clone(),
        cwd: exec_params.cwd.clone(),
        env: exec_params.env.clone(),
        expiration: exec_params.expiration.clone(),
        sandbox_permissions: exec_params.sandbox_permissions,
        additional_permissions: exec_params.additional_permissions.clone(),
        windows_sandbox_level: exec_params.windows_sandbox_level,
        network: exec_params.network,
        network_attempt_id: exec_params.network_attempt_id.clone(),
        justification: exec_params.justification.clone(),
        prefix_rule: exec_params.prefix_rule.clone(),
        arg0: exec_params.arg0.clone(),
      },
      policy: attempt.policy.clone(),
    })
    .map_err(|e| ToolError::Execution(e.to_string()))?;

    execute_command(&transform_result.exec_params)
      .await
      .map_err(|e| match e {
        ExecError::SpawnFailed { message, .. } => {
          if attempt.sandbox != SandboxKind::None && looks_like_sandbox_denial(&message) {
            ToolError::SandboxDenied {
              output: message,
              network_policy_reason: None,
            }
          } else {
            ToolError::Execution(message)
          }
        }
        ExecError::SandboxDenied { output } => ToolError::SandboxDenied {
          output,
          network_policy_reason: None,
        },
        ExecError::Other(msg) => ToolError::Execution(msg),
      })
  }
}

/// Check if an error message looks like a sandbox denial.
fn looks_like_sandbox_denial(message: &str) -> bool {
  let lower = message.to_lowercase();
  lower.contains("sandbox denied")
    || lower.contains("permission denied")
    || lower.contains("operation not permitted")
}

// ---------------------------------------------------------------------------
// process_shell_command — convenience function for the handler
// ---------------------------------------------------------------------------

/// High-level function: execute a shell command through the full pipeline.
///
/// This is the function that `ShellHandler` calls instead of directly
/// spawning a process.
///
/// Pipeline: ShellCommandRequest → ExecParams → SandboxManager::transform()
///           → execute_command() → ExecToolCallOutput → formatted string
pub async fn process_shell_command(
  shell: &Shell,
  command: &str,
  cwd: PathBuf,
  timeout_ms: Option<u64>,
  env: HashMap<String, String>,
  sandbox_policy: &cokra_protocol::SandboxPolicy,
) -> Result<ExecToolCallOutput, ExecError> {
  let argv = shell.derive_exec_args(command, true);
  let timeout = timeout_ms
    .map(|ms| ExecExpiration::Timeout(Duration::from_millis(ms)))
    .unwrap_or(ExecExpiration::DefaultTimeout);

  let command_spec = CommandSpec {
    command: argv,
    cwd,
    env,
    expiration: timeout,
    sandbox_permissions: SandboxPermissions::UseDefault,
    additional_permissions: None,
    windows_sandbox_level: WindowsSandboxLevel::Disabled,
    network: None,
    network_attempt_id: None,
    justification: None,
    prefix_rule: None,
    arg0: None,
  };

  let transform_result = SandboxManager::transform(SandboxTransformRequest {
    command_spec,
    policy: sandbox_policy.clone(),
  })
  .map_err(|e| ExecError::Other(e.to_string()))?;

  execute_command(&transform_result.exec_params).await
}
