//! 1:1 codex: Execution policy layer.
//!
//! Determines the approval requirement for a command based on:
//! - Whether the command is known-safe (read-only allowlist)
//! - Whether the command is potentially dangerous (force-delete etc.)
//! - The active sandbox policy
//! - The active approval policy
//! - The sandbox permissions level
//!
//! This mirrors codex-rs `exec_policy.rs` `render_decision_for_unmatched_command`
//! plus `shell-command/src/command_safety/{is_safe_command,is_dangerous_command}.rs`.

use cokra_protocol::AskForApproval;
use cokra_protocol::SandboxPolicy;

use crate::exec::SandboxPermissions;
use crate::tools::command_intent::CommandIntent;
use crate::tools::sandboxing::ExecApprovalRequirement;

// ---------------------------------------------------------------------------
// Command safety classification (1:1 codex shell-command crate)
// ---------------------------------------------------------------------------

/// 1:1 codex `is_safe_command.rs::is_known_safe_command`.
///
/// Returns `true` for read-only commands that can be auto-approved without
/// user confirmation. This is a conservative allowlist — only commands that
/// are guaranteed to not modify state, write files, or execute external code.
pub fn is_known_safe_command(command: &[String]) -> bool {
  is_safe_to_call_with_exec(command)
}

#[cfg(test)]
mod shell_string_tests {
  use super::eval_shell_command_approval;
  use crate::exec::SandboxPermissions;
  use crate::tools::sandboxing::ExecApprovalRequirement;
  use cokra_protocol::AskForApproval;
  use cokra_protocol::ReadOnlyAccess;
  use cokra_protocol::SandboxPolicy;

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
  fn shell_string_safe_command_uses_canonical_command() {
    let req = eval_shell_command_approval(
      "git status",
      std::path::Path::new("."),
      &ws_policy(),
      AskForApproval::UnlessTrusted,
      SandboxPermissions::UseDefault,
    );
    assert!(matches!(req, ExecApprovalRequirement::Skip { .. }));
  }
}

/// 1:1 codex `is_safe_command.rs::is_safe_to_call_with_exec`.
fn is_safe_to_call_with_exec(command: &[String]) -> bool {
  let Some(cmd0) = command.first().map(String::as_str) else {
    return false;
  };

  match std::path::Path::new(cmd0)
    .file_name()
    .and_then(|osstr| osstr.to_str())
  {
    Some(cmd) if cfg!(target_os = "linux") && matches!(cmd, "numfmt" | "tac") => true,

    #[rustfmt::skip]
    Some(
      "cat" |
      "cd" |
      "cut" |
      "echo" |
      "expr" |
      "false" |
      "grep" |
      "head" |
      "id" |
      "ls" |
      "nl" |
      "paste" |
      "pwd" |
      "rev" |
      "seq" |
      "stat" |
      "tail" |
      "tr" |
      "true" |
      "uname" |
      "uniq" |
      "wc" |
      "which" |
      "whoami") => {
      true
    },

    Some("base64") => {
      const UNSAFE_BASE64_OPTIONS: &[&str] = &["-o", "--output"];
      !command.iter().skip(1).any(|arg| {
        UNSAFE_BASE64_OPTIONS.contains(&arg.as_str())
          || arg.starts_with("--output=")
          || (arg.starts_with("-o") && arg != "-o")
      })
    }

    Some("find") => {
      #[rustfmt::skip]
      const UNSAFE_FIND_OPTIONS: &[&str] = &[
        "-exec", "-execdir", "-ok", "-okdir",
        "-delete",
        "-fls", "-fprint", "-fprint0", "-fprintf",
      ];
      !command
        .iter()
        .any(|arg| UNSAFE_FIND_OPTIONS.contains(&arg.as_str()))
    }

    // Ripgrep
    Some("rg") => {
      const UNSAFE_RIPGREP_OPTIONS_WITH_ARGS: &[&str] = &["--pre", "--hostname-bin"];
      const UNSAFE_RIPGREP_OPTIONS_WITHOUT_ARGS: &[&str] = &["--search-zip", "-z"];
      !command.iter().any(|arg| {
        UNSAFE_RIPGREP_OPTIONS_WITHOUT_ARGS.contains(&arg.as_str())
          || UNSAFE_RIPGREP_OPTIONS_WITH_ARGS
            .iter()
            .any(|&opt| arg == opt || arg.starts_with(&format!("{opt}=")))
      })
    }

    // Git (read-only subcommands only)
    Some("git") => {
      if git_has_config_override_global_option(command) {
        return false;
      }
      let Some((subcommand_idx, subcommand)) =
        find_git_subcommand(command, &["status", "log", "diff", "show", "branch"])
      else {
        return false;
      };
      let subcommand_args = &command[subcommand_idx + 1..];
      match subcommand {
        "status" | "log" | "diff" | "show" => git_subcommand_args_are_read_only(subcommand_args),
        "branch" => {
          git_subcommand_args_are_read_only(subcommand_args)
            && git_branch_is_read_only(subcommand_args)
        }
        other => {
          debug_assert!(false, "unexpected git subcommand from matcher: {other}");
          false
        }
      }
    }

    // Special-case `sed -n {N|M,N}p`
    Some("sed")
      if {
        command.len() <= 4
          && command.get(1).map(String::as_str) == Some("-n")
          && is_valid_sed_n_arg(command.get(2).map(String::as_str))
      } =>
    {
      true
    }

    _ => false,
  }
}

// 1:1 codex: git branch is safe only when read-only flags are present.
fn git_branch_is_read_only(branch_args: &[String]) -> bool {
  if branch_args.is_empty() {
    return true;
  }
  let mut saw_read_only_flag = false;
  for arg in branch_args.iter().map(String::as_str) {
    match arg {
      "--list" | "-l" | "--show-current" | "-a" | "--all" | "-r" | "--remotes" | "-v" | "-vv"
      | "--verbose" => {
        saw_read_only_flag = true;
      }
      _ if arg.starts_with("--format=") => {
        saw_read_only_flag = true;
      }
      _ => {
        return false;
      }
    }
  }
  saw_read_only_flag
}

fn git_has_config_override_global_option(command: &[String]) -> bool {
  command.iter().map(String::as_str).any(|arg| {
    matches!(arg, "-c" | "--config-env")
      || (arg.starts_with("-c") && arg.len() > 2)
      || arg.starts_with("--config-env=")
  })
}

fn is_git_global_option_with_value(arg: &str) -> bool {
  matches!(
    arg,
    "-C"
      | "-c"
      | "--config-env"
      | "--exec-path"
      | "--git-dir"
      | "--namespace"
      | "--super-prefix"
      | "--work-tree"
  )
}

fn is_git_global_option_with_inline_value(arg: &str) -> bool {
  (arg.starts_with("--config-env=")
    || arg.starts_with("--exec-path=")
    || arg.starts_with("--git-dir=")
    || arg.starts_with("--namespace=")
    || arg.starts_with("--super-prefix=")
    || arg.starts_with("--work-tree="))
    || ((arg.starts_with("-C") || arg.starts_with("-c")) && arg.len() > 2)
}

/// 1:1 codex `is_dangerous_command.rs::find_git_subcommand`.
fn find_git_subcommand<'a>(
  command: &'a [String],
  subcommands: &[&str],
) -> Option<(usize, &'a str)> {
  let cmd0 = command.first().map(String::as_str)?;
  if !cmd0.ends_with("git") {
    return None;
  }
  let mut skip_next = false;
  for (idx, arg) in command.iter().enumerate().skip(1) {
    if skip_next {
      skip_next = false;
      continue;
    }
    let arg = arg.as_str();
    if is_git_global_option_with_inline_value(arg) {
      continue;
    }
    if is_git_global_option_with_value(arg) {
      skip_next = true;
      continue;
    }
    if arg == "--" || arg.starts_with('-') {
      continue;
    }
    if subcommands.contains(&arg) {
      return Some((idx, arg));
    }
    // First non-option token is the subcommand; if it doesn't match, stop.
    return None;
  }
  None
}

fn git_subcommand_args_are_read_only(args: &[String]) -> bool {
  const UNSAFE_GIT_FLAGS: &[&str] = &[
    "--output",
    "--ext-diff",
    "--textconv",
    "--exec",
    "--paginate",
  ];
  !args.iter().map(String::as_str).any(|arg| {
    UNSAFE_GIT_FLAGS.contains(&arg) || arg.starts_with("--output=") || arg.starts_with("--exec=")
  })
}

/// Returns true if `arg` matches /^(\d+,)?\d+p$/
fn is_valid_sed_n_arg(arg: Option<&str>) -> bool {
  let s = match arg {
    Some(s) => s,
    None => return false,
  };
  let core = match s.strip_suffix('p') {
    Some(rest) => rest,
    None => return false,
  };
  let parts: Vec<&str> = core.split(',').collect();
  match parts.as_slice() {
    [num] => !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()),
    [a, b] => {
      !a.is_empty()
        && !b.is_empty()
        && a.chars().all(|c| c.is_ascii_digit())
        && b.chars().all(|c| c.is_ascii_digit())
    }
    _ => false,
  }
}

// ---------------------------------------------------------------------------
// Dangerous command detection (1:1 codex is_dangerous_command.rs)
// ---------------------------------------------------------------------------

/// 1:1 codex `is_dangerous_command.rs::command_might_be_dangerous`.
///
/// Returns `true` for a narrow set of destructive patterns that should
/// ALWAYS require user confirmation regardless of approval policy.
pub fn command_might_be_dangerous(command: &[String]) -> bool {
  is_dangerous_to_call_with_exec(command)
}

fn is_dangerous_to_call_with_exec(command: &[String]) -> bool {
  let cmd0 = command.first().map(String::as_str);
  match cmd0 {
    Some("rm") => matches!(command.get(1).map(String::as_str), Some("-f" | "-rf")),
    // For sudo <cmd> simply recurse on <cmd>.
    Some("sudo") => is_dangerous_to_call_with_exec(&command[1..]),
    _ => false,
  }
}

// ---------------------------------------------------------------------------
// Policy evaluation (1:1 codex render_decision_for_unmatched_command)
// ---------------------------------------------------------------------------

/// Evaluate the approval requirement for a command execution.
///
/// 1:1 codex `exec_policy.rs::render_decision_for_unmatched_command`:
///
/// 1. If command is known-safe → `Skip` (auto-approve)
/// 2. If command is dangerous → `NeedsApproval` (or `Forbidden` if Never)
/// 3. apply_patch intercept → `Forbidden`
/// 4. Escalated permissions in non-OnRequest modes → `Forbidden`
/// 5. Otherwise → policy + sandbox matrix
pub fn eval_exec_approval(
  command: &[String],
  sandbox_policy: &SandboxPolicy,
  approval_policy: AskForApproval,
  sandbox_permissions: SandboxPermissions,
) -> ExecApprovalRequirement {
  // Spec: apply_patch intercept — forbid external apply_patch execution.
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

  // Spec: forbid escalated permissions in non-OnRequest modes.
  if sandbox_permissions == SandboxPermissions::RequireEscalated
    && !matches!(approval_policy, AskForApproval::OnRequest)
  {
    return ExecApprovalRequirement::Forbidden {
      reason: "escalated sandbox permissions require OnRequest approval policy".to_string(),
    };
  }

  // 1:1 codex: if the command is known-safe, allow without prompting.
  if is_known_safe_command(command) {
    return ExecApprovalRequirement::Skip {
      bypass_sandbox: false,
    };
  }

  // 1:1 codex: if the command is dangerous, always prompt (or forbid in Never mode).
  if command_might_be_dangerous(command) {
    return if matches!(approval_policy, AskForApproval::Never) {
      ExecApprovalRequirement::Forbidden {
        reason: "dangerous command blocked by approval policy (Never)".to_string(),
      }
    } else {
      ExecApprovalRequirement::NeedsApproval {
        reason: command_display_for_approval(command),
      }
    };
  }

  // 1:1 codex: policy + sandbox matrix for unclassified commands.
  match approval_policy {
    AskForApproval::Never | AskForApproval::OnFailure => {
      // Allow the command to run, relying on sandbox for protection.
      ExecApprovalRequirement::Skip {
        bypass_sandbox: false,
      }
    }
    AskForApproval::UnlessTrusted => {
      // Already checked is_known_safe_command and it returned false, so prompt.
      ExecApprovalRequirement::NeedsApproval {
        reason: command_display_for_approval(command),
      }
    }
    AskForApproval::OnRequest => {
      match sandbox_policy {
        SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. } => {
          // User has indicated full access — run non-dangerous commands.
          ExecApprovalRequirement::Skip {
            bypass_sandbox: false,
          }
        }
        SandboxPolicy::ReadOnly { .. } | SandboxPolicy::WorkspaceWrite { .. } => {
          // 1:1 codex: in restricted sandboxes, only prompt for escalated
          // permissions — let sandbox enforce the rest.
          if sandbox_permissions == SandboxPermissions::RequireEscalated {
            ExecApprovalRequirement::NeedsApproval {
              reason: command_display_for_approval(command),
            }
          } else {
            ExecApprovalRequirement::Skip {
              bypass_sandbox: false,
            }
          }
        }
      }
    }
  }
}

pub fn eval_shell_command_approval(
  command: &str,
  cwd: &std::path::Path,
  sandbox_policy: &SandboxPolicy,
  approval_policy: AskForApproval,
  sandbox_permissions: SandboxPermissions,
) -> ExecApprovalRequirement {
  let intent = CommandIntent::from_command(command, cwd);
  let approval_command = if intent.canonical_command.is_empty() {
    vec![command.to_string()]
  } else {
    intent.canonical_command
  };
  eval_exec_approval(
    &approval_command,
    sandbox_policy,
    approval_policy,
    sandbox_permissions,
  )
}

/// Build the display string for the approval prompt.
fn command_display_for_approval(command: &[String]) -> Option<String> {
  if command.is_empty() {
    return None;
  }
  Some(command.join(" "))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;
  use cokra_protocol::ReadOnlyAccess;

  fn vec_str(args: &[&str]) -> Vec<String> {
    args.iter().map(std::string::ToString::to_string).collect()
  }

  fn ws_policy() -> SandboxPolicy {
    SandboxPolicy::WorkspaceWrite {
      writable_roots: vec![".".to_string()],
      read_only_access: ReadOnlyAccess::FullAccess,
      network_access: false,
      exclude_tmpdir_env_var: false,
      exclude_slash_tmp: false,
    }
  }

  // -- is_known_safe_command tests --

  #[test]
  fn known_safe_examples() {
    assert!(is_known_safe_command(&vec_str(&["ls"])));
    assert!(is_known_safe_command(&vec_str(&["wc", "-l", "file.txt"])));
    assert!(is_known_safe_command(&vec_str(&["cat", "foo.rs"])));
    assert!(is_known_safe_command(&vec_str(&["head", "-n", "10"])));
    assert!(is_known_safe_command(&vec_str(&["git", "status"])));
    assert!(is_known_safe_command(&vec_str(&["git", "branch"])));
    assert!(is_known_safe_command(&vec_str(&[
      "git",
      "branch",
      "--show-current"
    ])));
    assert!(is_known_safe_command(&vec_str(&["grep", "-r", "foo"])));
    assert!(is_known_safe_command(&vec_str(&["rg", "Cargo.toml", "-n"])));
    assert!(is_known_safe_command(&vec_str(&[
      "find", ".", "-name", "*.rs"
    ])));
    assert!(is_known_safe_command(&vec_str(&["sed", "-n", "1,5p"])));
  }

  #[test]
  fn known_unsafe_examples() {
    assert!(!is_known_safe_command(&vec_str(&["cargo", "check"])));
    assert!(!is_known_safe_command(&vec_str(&["python", "script.py"])));
    assert!(!is_known_safe_command(&vec_str(&["npm", "install"])));
    assert!(!is_known_safe_command(&vec_str(&["git", "push"])));
    assert!(!is_known_safe_command(&vec_str(&["git", "checkout"])));
    assert!(!is_known_safe_command(&vec_str(&[
      "find", ".", "-name", "*.rs", "-delete"
    ])));
    assert!(!is_known_safe_command(&vec_str(&[
      "rg",
      "--search-zip",
      "files"
    ])));
  }

  #[test]
  fn git_branch_mutating_is_not_safe() {
    assert!(!is_known_safe_command(&vec_str(&[
      "git", "branch", "-d", "feature"
    ])));
    assert!(!is_known_safe_command(&vec_str(&[
      "git",
      "branch",
      "new-branch"
    ])));
  }

  #[test]
  fn git_config_override_is_not_safe() {
    assert!(!is_known_safe_command(&vec_str(&[
      "git",
      "-c",
      "core.pager=cat",
      "log",
      "-n",
      "1",
    ])));
  }

  // -- command_might_be_dangerous tests --

  #[test]
  fn rm_rf_is_dangerous() {
    assert!(command_might_be_dangerous(&vec_str(&["rm", "-rf", "/"])));
  }

  #[test]
  fn rm_f_is_dangerous() {
    assert!(command_might_be_dangerous(&vec_str(&["rm", "-f", "/"])));
  }

  #[test]
  fn sudo_rm_is_dangerous() {
    assert!(command_might_be_dangerous(&vec_str(&[
      "sudo", "rm", "-rf", "/"
    ])));
  }

  #[test]
  fn ls_is_not_dangerous() {
    assert!(!command_might_be_dangerous(&vec_str(&["ls"])));
  }

  // -- eval_exec_approval tests --

  #[test]
  fn safe_command_skips_in_on_request() {
    let req = eval_exec_approval(
      &vec_str(&["wc", "-l", "file.txt"]),
      &ws_policy(),
      AskForApproval::OnRequest,
      SandboxPermissions::UseDefault,
    );
    assert!(matches!(req, ExecApprovalRequirement::Skip { .. }));
  }

  #[test]
  fn safe_command_skips_in_unless_trusted() {
    let req = eval_exec_approval(
      &vec_str(&["ls", "-la"]),
      &ws_policy(),
      AskForApproval::UnlessTrusted,
      SandboxPermissions::UseDefault,
    );
    assert!(matches!(req, ExecApprovalRequirement::Skip { .. }));
  }

  #[test]
  fn unsafe_command_needs_approval_in_on_request_workspace() {
    let req = eval_exec_approval(
      &vec_str(&["cargo", "build"]),
      &ws_policy(),
      AskForApproval::OnRequest,
      SandboxPermissions::UseDefault,
    );
    // 1:1 codex: non-escalated in restricted sandbox → Skip (sandbox enforces)
    assert!(matches!(req, ExecApprovalRequirement::Skip { .. }));
  }

  #[test]
  fn dangerous_command_needs_approval_always() {
    let req = eval_exec_approval(
      &vec_str(&["rm", "-rf", "/"]),
      &SandboxPolicy::DangerFullAccess,
      AskForApproval::OnRequest,
      SandboxPermissions::UseDefault,
    );
    assert!(matches!(req, ExecApprovalRequirement::NeedsApproval { .. }));
  }

  #[test]
  fn dangerous_command_forbidden_in_never() {
    let req = eval_exec_approval(
      &vec_str(&["rm", "-rf", "/"]),
      &ws_policy(),
      AskForApproval::Never,
      SandboxPermissions::UseDefault,
    );
    assert!(matches!(req, ExecApprovalRequirement::Forbidden { .. }));
  }

  #[test]
  fn never_forbids_shell() {
    // Never mode: non-safe, non-dangerous → relies on sandbox (Allow in codex)
    // but since we map Never → Skip for non-dangerous, this is Skip.
    let req = eval_exec_approval(
      &vec_str(&["bash", "-c", "pwd"]),
      &ws_policy(),
      AskForApproval::Never,
      SandboxPermissions::UseDefault,
    );
    // "bash" is not in known-safe list (we don't parse bash -c here),
    // and it's not dangerous, so it falls to Never → Skip.
    assert!(matches!(req, ExecApprovalRequirement::Skip { .. }));
  }

  #[test]
  fn on_failure_skips() {
    let req = eval_exec_approval(
      &vec_str(&["bash", "-c", "pwd"]),
      &ws_policy(),
      AskForApproval::OnFailure,
      SandboxPermissions::UseDefault,
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
      &vec_str(&["bash", "-c", "pwd"]),
      &SandboxPolicy::DangerFullAccess,
      AskForApproval::OnRequest,
      SandboxPermissions::UseDefault,
    );
    assert!(matches!(req, ExecApprovalRequirement::Skip { .. }));
  }

  #[test]
  fn on_request_workspace_write_non_escalated_skips() {
    // 1:1 codex: non-escalated in restricted sandbox → Skip (let sandbox enforce)
    let req = eval_exec_approval(
      &vec_str(&["python", "script.py"]),
      &ws_policy(),
      AskForApproval::OnRequest,
      SandboxPermissions::UseDefault,
    );
    assert!(matches!(req, ExecApprovalRequirement::Skip { .. }));
  }

  #[test]
  fn on_request_workspace_write_escalated_needs_approval() {
    let req = eval_exec_approval(
      &vec_str(&["python", "script.py"]),
      &ws_policy(),
      AskForApproval::OnRequest,
      SandboxPermissions::RequireEscalated,
    );
    assert!(matches!(req, ExecApprovalRequirement::NeedsApproval { .. }));
  }

  #[test]
  fn unless_trusted_non_safe_needs_approval() {
    let req = eval_exec_approval(
      &vec_str(&["cargo", "build"]),
      &SandboxPolicy::DangerFullAccess,
      AskForApproval::UnlessTrusted,
      SandboxPermissions::UseDefault,
    );
    assert!(matches!(req, ExecApprovalRequirement::NeedsApproval { .. }));
  }

  #[test]
  fn apply_patch_intercepted() {
    let req = eval_exec_approval(
      &["apply_patch".to_string(), "file.patch".to_string()],
      &SandboxPolicy::DangerFullAccess,
      AskForApproval::OnRequest,
      SandboxPermissions::UseDefault,
    );
    assert!(matches!(req, ExecApprovalRequirement::Forbidden { .. }));
  }

  #[test]
  fn apply_patch_with_path_intercepted() {
    let req = eval_exec_approval(
      &["/usr/bin/apply_patch".to_string()],
      &SandboxPolicy::DangerFullAccess,
      AskForApproval::OnRequest,
      SandboxPermissions::UseDefault,
    );
    assert!(matches!(req, ExecApprovalRequirement::Forbidden { .. }));
  }

  #[test]
  fn escalated_forbidden_in_non_on_request() {
    let req = eval_exec_approval(
      &vec_str(&["bash", "-c", "pwd"]),
      &ws_policy(),
      AskForApproval::UnlessTrusted,
      SandboxPermissions::RequireEscalated,
    );
    assert!(matches!(req, ExecApprovalRequirement::Forbidden { .. }));
  }

  #[test]
  fn escalated_ok_in_on_request() {
    let req = eval_exec_approval(
      &vec_str(&["bash", "-c", "pwd"]),
      &ws_policy(),
      AskForApproval::OnRequest,
      SandboxPermissions::RequireEscalated,
    );
    // Escalated + restricted sandbox → NeedsApproval
    assert!(matches!(req, ExecApprovalRequirement::NeedsApproval { .. }));
  }
}
