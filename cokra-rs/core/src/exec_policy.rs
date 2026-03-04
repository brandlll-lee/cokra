//! 1:1 codex: Execution policy layer.
//!
//! Determines the approval requirement for a command based on:
//! - The command argv
//! - The active sandbox policy
//! - The active approval policy
//! - The sandbox permissions level
//!
//! This replaces the coarse-grained `spec.permissions.requires_approval`
//! with fine-grained per-command decisions.

use cokra_protocol::AskForApproval;
use cokra_protocol::SandboxPolicy;

use crate::exec::SandboxPermissions;
use crate::tools::sandboxing::ExecApprovalRequirement;

// ---------------------------------------------------------------------------
// Policy evaluation
// ---------------------------------------------------------------------------

/// Evaluate the approval requirement for a command execution.
///
/// ## Spec 4 rules (1:1 codex):
///
/// 1. `AskForApproval::Never` → `Forbidden` for commands requiring approval
///    (shell commands always require approval in this mode)
/// 2. `AskForApproval::OnFailure` → `Skip` (run first, ask on failure)
/// 3. `AskForApproval::OnRequest` + `DangerFullAccess`/`ExternalSandbox` → `Skip`
/// 4. `AskForApproval::OnRequest` + other policies → `NeedsApproval`
/// 5. `AskForApproval::UnlessTrusted` → always `NeedsApproval`
/// 6. `RequireEscalated` permissions are forbidden in non-OnRequest modes
/// 7. `apply_patch` in argv[0] is intercepted and forbidden
///    (codex intercepts apply_patch to prevent external execution)
pub fn eval_exec_approval(
  command: &[String],
  sandbox_policy: &SandboxPolicy,
  approval_policy: AskForApproval,
  sandbox_permissions: SandboxPermissions,
) -> ExecApprovalRequirement {
  // Spec 4: apply_patch intercept — forbid external apply_patch execution
  if let Some(program) = command.first() {
    let basename = program
      .rsplit('/')
      .next()
      .unwrap_or(program)
      .rsplit('\\')
      .next()
      .unwrap_or(program);
    if basename == "apply_patch" || basename == "apply-patch" {
      return ExecApprovalRequirement::Forbidden {
        reason: "apply_patch must be handled internally, not via external execution".to_string(),
      };
    }
  }

  // Spec 4: forbid escalated permissions in non-OnRequest modes
  if sandbox_permissions == SandboxPermissions::RequireEscalated
    && !matches!(approval_policy, AskForApproval::OnRequest)
  {
    return ExecApprovalRequirement::Forbidden {
      reason: "escalated sandbox permissions require OnRequest approval policy".to_string(),
    };
  }

  match approval_policy {
    AskForApproval::Never => {
      // Shell execution is always potentially dangerous — forbid in Never mode
      ExecApprovalRequirement::Forbidden {
        reason: "shell execution is blocked by approval policy (Never)".to_string(),
      }
    }
    AskForApproval::OnFailure => {
      // Run first, ask on failure
      ExecApprovalRequirement::Skip {
        bypass_sandbox: false,
      }
    }
    AskForApproval::OnRequest => {
      // Check sandbox policy to decide if approval is needed
      match sandbox_policy {
        SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. } => {
          ExecApprovalRequirement::Skip {
            bypass_sandbox: false,
          }
        }
        _ => ExecApprovalRequirement::NeedsApproval {
          reason: command_display_for_approval(command),
        },
      }
    }
    AskForApproval::UnlessTrusted => {
      // Always need approval
      ExecApprovalRequirement::NeedsApproval {
        reason: command_display_for_approval(command),
      }
    }
  }
}

/// Build the display string for the approval prompt.
fn command_display_for_approval(command: &[String]) -> Option<String> {
  if command.is_empty() {
    return None;
  }
  // Show the command as the user would type it
  Some(command.join(" "))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;
  use cokra_protocol::ReadOnlyAccess;

  fn ws_policy() -> SandboxPolicy {
    SandboxPolicy::WorkspaceWrite {
      writable_roots: vec![".".to_string()],
      read_only_access: ReadOnlyAccess::FullAccess,
      network_access: false,
      exclude_tmpdir_env_var: false,
      exclude_slash_tmp: false,
    }
  }

  #[test]
  fn never_forbids_shell() {
    let req = eval_exec_approval(
      &["bash".to_string(), "-c".to_string(), "pwd".to_string()],
      &ws_policy(),
      AskForApproval::Never,
      SandboxPermissions::Default,
    );
    assert!(matches!(req, ExecApprovalRequirement::Forbidden { .. }));
  }

  #[test]
  fn on_failure_skips() {
    let req = eval_exec_approval(
      &["bash".to_string(), "-c".to_string(), "pwd".to_string()],
      &ws_policy(),
      AskForApproval::OnFailure,
      SandboxPermissions::Default,
    );
    assert!(matches!(
      req,
      ExecApprovalRequirement::Skip {
        bypass_sandbox: false
      }
    ));
  }

  #[test]
  fn on_request_danger_full_access_skips() {
    let req = eval_exec_approval(
      &["bash".to_string(), "-c".to_string(), "pwd".to_string()],
      &SandboxPolicy::DangerFullAccess,
      AskForApproval::OnRequest,
      SandboxPermissions::Default,
    );
    assert!(matches!(req, ExecApprovalRequirement::Skip { .. }));
  }

  #[test]
  fn on_request_workspace_write_needs_approval() {
    let req = eval_exec_approval(
      &["bash".to_string(), "-c".to_string(), "pwd".to_string()],
      &ws_policy(),
      AskForApproval::OnRequest,
      SandboxPermissions::Default,
    );
    assert!(matches!(req, ExecApprovalRequirement::NeedsApproval { .. }));
  }

  #[test]
  fn unless_trusted_always_needs_approval() {
    let req = eval_exec_approval(
      &["bash".to_string(), "-c".to_string(), "pwd".to_string()],
      &SandboxPolicy::DangerFullAccess,
      AskForApproval::UnlessTrusted,
      SandboxPermissions::Default,
    );
    assert!(matches!(req, ExecApprovalRequirement::NeedsApproval { .. }));
  }

  #[test]
  fn apply_patch_intercepted() {
    let req = eval_exec_approval(
      &["apply_patch".to_string(), "file.patch".to_string()],
      &SandboxPolicy::DangerFullAccess,
      AskForApproval::OnRequest,
      SandboxPermissions::Default,
    );
    assert!(matches!(req, ExecApprovalRequirement::Forbidden { .. }));
  }

  #[test]
  fn apply_patch_with_path_intercepted() {
    let req = eval_exec_approval(
      &["/usr/bin/apply_patch".to_string()],
      &SandboxPolicy::DangerFullAccess,
      AskForApproval::OnRequest,
      SandboxPermissions::Default,
    );
    assert!(matches!(req, ExecApprovalRequirement::Forbidden { .. }));
  }

  #[test]
  fn escalated_forbidden_in_non_on_request() {
    let req = eval_exec_approval(
      &["bash".to_string(), "-c".to_string(), "pwd".to_string()],
      &ws_policy(),
      AskForApproval::UnlessTrusted,
      SandboxPermissions::RequireEscalated,
    );
    assert!(matches!(req, ExecApprovalRequirement::Forbidden { .. }));
  }

  #[test]
  fn escalated_ok_in_on_request() {
    let req = eval_exec_approval(
      &["bash".to_string(), "-c".to_string(), "pwd".to_string()],
      &ws_policy(),
      AskForApproval::OnRequest,
      SandboxPermissions::RequireEscalated,
    );
    // Should still need approval (WorkspaceWrite + OnRequest)
    assert!(matches!(req, ExecApprovalRequirement::NeedsApproval { .. }));
  }
}
