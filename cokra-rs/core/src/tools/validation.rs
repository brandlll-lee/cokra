use serde_json::Value;

use cokra_config::{ApprovalMode, ApprovalPolicy, SandboxConfig};

use crate::tools::context::FunctionCallError;

#[derive(Debug, Clone)]
pub struct ToolCall {
  pub tool_name: String,
  pub args: Value,
}

#[derive(Debug, Clone)]
pub struct ValidationResult {
  pub valid: bool,
  pub reason: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
  #[error("dangerous command")]
  DangerousCommand,
  #[error("path traversal detected")]
  PathTraversal,
  #[error("permission denied: {0}")]
  PermissionDenied(String),
  #[error("invalid arguments: {0}")]
  InvalidArguments(String),
}

pub struct ToolValidator {
  sandbox_config: SandboxConfig,
  approval_policy: ApprovalPolicy,
}

impl ToolValidator {
  pub fn new(sandbox_config: SandboxConfig, approval_policy: ApprovalPolicy) -> Self {
    Self {
      sandbox_config,
      approval_policy,
    }
  }

  pub fn validate_tool_call(&self, call: &ToolCall) -> Result<ValidationResult, ValidationError> {
    if has_path_traversal(&call.args) {
      return Err(ValidationError::PathTraversal);
    }

    if call.tool_name == "shell" {
      let cmd = call
        .args
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| ValidationError::InvalidArguments("missing command".to_string()))?;
      self.validate_shell_command(cmd)?;
    }

    match self
      .approval_policy
      .check_tool_use(&call.tool_name, &call.args)
    {
      ApprovalResult::Approved => Ok(ValidationResult {
        valid: true,
        reason: None,
      }),
      ApprovalResult::Denied(reason) => Err(ValidationError::PermissionDenied(reason)),
      ApprovalResult::RequiresUserInput(prompt) => Ok(ValidationResult {
        valid: false,
        reason: Some(prompt),
      }),
    }
  }

  pub fn validate_shell_command(&self, cmd: &str) -> Result<ValidationResult, ValidationError> {
    if contains_dangerous_patterns(cmd) {
      return Err(ValidationError::DangerousCommand);
    }

    Ok(ValidationResult {
      valid: true,
      reason: None,
    })
  }

  pub fn sandbox_config(&self) -> &SandboxConfig {
    &self.sandbox_config
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalResult {
  Approved,
  Denied(String),
  RequiresUserInput(String),
}

pub trait ApprovalPolicyExt {
  fn check_tool_use(&self, tool: &str, args: &Value) -> ApprovalResult;
}

impl ApprovalPolicyExt for ApprovalPolicy {
  fn check_tool_use(&self, tool: &str, _args: &Value) -> ApprovalResult {
    match self.policy {
      ApprovalMode::Auto => ApprovalResult::Approved,
      ApprovalMode::Ask => ApprovalResult::RequiresUserInput(format!("Execute {tool}?")),
      ApprovalMode::Never => ApprovalResult::Denied("Tool use disabled".to_string()),
    }
  }
}

fn contains_dangerous_patterns(cmd: &str) -> bool {
  let patterns = [
    "rm -rf /",
    "mkfs",
    "dd if=",
    "shutdown",
    "reboot",
    ":(){:|:&};:",
  ];
  patterns.iter().any(|pattern| cmd.contains(pattern))
}

fn has_path_traversal(value: &Value) -> bool {
  match value {
    Value::String(s) => s.contains("../") || s.contains("..\\"),
    Value::Array(items) => items.iter().any(has_path_traversal),
    Value::Object(map) => map.values().any(has_path_traversal),
    _ => false,
  }
}

impl From<ValidationError> for FunctionCallError {
  fn from(value: ValidationError) -> Self {
    FunctionCallError::Validation(value.to_string())
  }
}

#[cfg(test)]
mod tests {
  use super::{ApprovalPolicyExt, ToolCall, ToolValidator};
  use cokra_config::{
    ApprovalMode, ApprovalPolicy, PatchApproval, SandboxConfig, SandboxMode, ShellApproval,
  };

  fn policy(mode: ApprovalMode) -> ApprovalPolicy {
    ApprovalPolicy {
      policy: mode,
      shell: ShellApproval::OnFailure,
      patch: PatchApproval::OnRequest,
    }
  }

  #[test]
  fn shell_danger_is_detected() {
    let validator = ToolValidator::new(
      SandboxConfig {
        mode: SandboxMode::Permissive,
        network_access: false,
      },
      policy(ApprovalMode::Auto),
    );

    let call = ToolCall {
      tool_name: "shell".to_string(),
      args: serde_json::json!({ "command": "rm -rf /" }),
    };

    assert!(validator.validate_tool_call(&call).is_err());
  }

  #[test]
  fn approval_policy_modes_work() {
    let args = serde_json::json!({});
    assert!(matches!(
      policy(ApprovalMode::Auto).check_tool_use("read_file", &args),
      super::ApprovalResult::Approved
    ));
    assert!(matches!(
      policy(ApprovalMode::Ask).check_tool_use("read_file", &args),
      super::ApprovalResult::RequiresUserInput(_)
    ));
    assert!(matches!(
      policy(ApprovalMode::Never).check_tool_use("read_file", &args),
      super::ApprovalResult::Denied(_)
    ));
  }
}
