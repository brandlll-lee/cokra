use serde_json::Value;

use cokra_config::ApprovalPolicy;
use cokra_config::SandboxConfig;

use crate::exec::PermissionProfile;
use crate::exec::SandboxPermissions;
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
  #[allow(dead_code)]
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
      validate_exec_request(&call.args, false, &self.approval_policy)?;
    }

    if matches!(
      call.tool_name.as_str(),
      "unified_exec" | "local_shell" | "container.exec"
    ) {
      let command = call
        .args
        .get("command")
        .and_then(Value::as_array)
        .ok_or_else(|| ValidationError::InvalidArguments("missing command".to_string()))?;
      if command.is_empty() || command.iter().any(|item| item.as_str().is_none()) {
        return Err(ValidationError::InvalidArguments(
          "command must be a non-empty string array".to_string(),
        ));
      }
      validate_exec_request(&call.args, true, &self.approval_policy)?;
    }

    Ok(ValidationResult {
      valid: true,
      reason: None,
    })
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

fn validate_exec_request(
  args: &Value,
  expects_argv: bool,
  approval_policy: &ApprovalPolicy,
) -> Result<(), ValidationError> {
  let sandbox_permissions = args
    .get("sandbox_permissions")
    .map(|value| serde_json::from_value::<SandboxPermissions>(value.clone()))
    .transpose()
    .map_err(|err| {
      ValidationError::InvalidArguments(format!("invalid sandbox_permissions: {err}"))
    })?
    .unwrap_or(SandboxPermissions::UseDefault);

  let additional_permissions = args
    .get("additional_permissions")
    .map(|value| serde_json::from_value::<PermissionProfile>(value.clone()))
    .transpose()
    .map_err(|err| {
      ValidationError::InvalidArguments(format!("invalid additional_permissions: {err}"))
    })?;

  if expects_argv {
    let _ = args
      .get("command")
      .and_then(Value::as_array)
      .ok_or_else(|| ValidationError::InvalidArguments("missing command".to_string()))?;
  }

  if let Some(prefix_rule) = args.get("prefix_rule") {
    let prefix_rule = prefix_rule.as_array().ok_or_else(|| {
      ValidationError::InvalidArguments("prefix_rule must be an array of strings".to_string())
    })?;
    if prefix_rule.is_empty() || prefix_rule.iter().any(|item| item.as_str().is_none()) {
      return Err(ValidationError::InvalidArguments(
        "prefix_rule must be a non-empty string array".to_string(),
      ));
    }
  }

  if let Some(justification) = args.get("justification")
    && justification
      .as_str()
      .is_none_or(|value| value.trim().is_empty())
  {
    return Err(ValidationError::InvalidArguments(
      "justification must be a non-empty string".to_string(),
    ));
  }

  if sandbox_permissions.requires_additional_permissions() {
    if !matches!(approval_policy.policy, cokra_config::ApprovalMode::Ask) {
      return Err(ValidationError::PermissionDenied(
        "with_additional_permissions requires interactive approval mode".to_string(),
      ));
    }
    let Some(profile) = additional_permissions.as_ref() else {
      return Err(ValidationError::InvalidArguments(
        "additional_permissions is required when sandbox_permissions is with_additional_permissions"
          .to_string(),
      ));
    };
    if profile.is_empty() {
      return Err(ValidationError::InvalidArguments(
        "additional_permissions must request at least one permission".to_string(),
      ));
    }
  } else if additional_permissions.is_some() {
    return Err(ValidationError::InvalidArguments(
      "additional_permissions requires sandbox_permissions=with_additional_permissions".to_string(),
    ));
  }

  if sandbox_permissions.requires_escalated_permissions()
    && !matches!(approval_policy.policy, cokra_config::ApprovalMode::Ask)
  {
    return Err(ValidationError::PermissionDenied(
      "require_escalated requires interactive approval mode".to_string(),
    ));
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::ToolCall;
  use super::ToolValidator;
  use cokra_config::ApprovalMode;
  use cokra_config::ApprovalPolicy;
  use cokra_config::PatchApproval;
  use cokra_config::SandboxConfig;
  use cokra_config::SandboxMode;
  use cokra_config::ShellApproval;

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
  fn approval_config_does_not_block_static_validation() {
    let validator = ToolValidator::new(
      SandboxConfig {
        mode: SandboxMode::Permissive,
        network_access: false,
      },
      policy(ApprovalMode::Never),
    );

    let call = ToolCall {
      tool_name: "read_file".to_string(),
      args: serde_json::json!({ "file_path": "demo.txt" }),
    };

    let result = validator.validate_tool_call(&call).expect("validation");
    assert!(result.valid);
  }
}
