//! 1:1 codex: Sandbox transformation layer.
//!
//! Converts a `CommandSpec` + `SandboxPolicy` into an `ExecRequest` that
//! the exec layer can run. This is the bridge between "what the tool wants
//! to run" and "how it actually gets spawned" under the active sandbox policy.
//!
//! Current scope (Spec 2): Linux/WSL only, no seatbelt/windows restricted token.

use std::collections::HashMap;
use std::path::PathBuf;

use cokra_protocol::SandboxPolicy;

use crate::exec::ExecExpiration;
use crate::exec::ExecParams;
use crate::exec::SandboxPermissions;
use crate::exec::WindowsSandboxLevel;

// ---------------------------------------------------------------------------
// SandboxKind — what type of sandboxing is in effect for this execution
// ---------------------------------------------------------------------------

/// The resolved sandbox type for a single execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedSandboxKind {
  /// No sandbox — run directly on host.
  None,
  /// Policy-based sandbox is active (future: bwrap/landlock/seatbelt).
  /// For now, this still runs on host but with policy metadata attached.
  Policy,
}

// ---------------------------------------------------------------------------
// CommandSpec — what the tool wants to run (before sandbox transform)
// ---------------------------------------------------------------------------

/// Pre-transform command specification from a tool handler.
#[derive(Debug, Clone)]
pub struct CommandSpec {
  /// Full argv.
  pub command: Vec<String>,
  /// Desired working directory.
  pub cwd: PathBuf,
  /// Extra environment variables.
  pub env: HashMap<String, String>,
  /// Expiration strategy.
  pub expiration: ExecExpiration,
  /// Sandbox permissions requested by the tool.
  pub sandbox_permissions: SandboxPermissions,
  /// Windows sandbox level.
  pub windows_sandbox_level: WindowsSandboxLevel,
  /// Network proxy placeholder.
  pub network: Option<()>,
  /// Network attempt id.
  pub network_attempt_id: Option<String>,
  /// Justification.
  pub justification: Option<String>,
  /// Override argv[0].
  pub arg0: Option<String>,
}

impl CommandSpec {
  /// Convert into `ExecParams` (identity transform — used when no sandbox
  /// transform is applied).
  pub fn into_exec_params(self) -> ExecParams {
    ExecParams {
      command: self.command,
      cwd: self.cwd,
      expiration: self.expiration,
      env: self.env,
      network: self.network,
      network_attempt_id: self.network_attempt_id,
      sandbox_permissions: self.sandbox_permissions,
      windows_sandbox_level: self.windows_sandbox_level,
      justification: self.justification,
      arg0: self.arg0,
    }
  }
}

// ---------------------------------------------------------------------------
// SandboxTransformRequest
// ---------------------------------------------------------------------------

/// Input to `SandboxManager::transform()`.
#[derive(Debug)]
pub struct SandboxTransformRequest {
  pub command_spec: CommandSpec,
  pub policy: SandboxPolicy,
}

// ---------------------------------------------------------------------------
// SandboxTransformResult
// ---------------------------------------------------------------------------

/// Output of `SandboxManager::transform()`.
#[derive(Debug)]
pub struct SandboxTransformResult {
  pub exec_params: ExecParams,
  pub sandbox_kind: ResolvedSandboxKind,
}

// ---------------------------------------------------------------------------
// SandboxTransformError
// ---------------------------------------------------------------------------

/// Errors during sandbox transformation.
#[derive(Debug, Clone)]
pub enum SandboxTransformError {
  /// The Linux sandbox executable (e.g. bwrap) is not available.
  MissingLinuxSandboxExecutable(String),
  /// General transformation error.
  Other(String),
}

impl std::fmt::Display for SandboxTransformError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      SandboxTransformError::MissingLinuxSandboxExecutable(msg) => {
        write!(f, "missing sandbox executable: {msg}")
      }
      SandboxTransformError::Other(msg) => write!(f, "{msg}"),
    }
  }
}

impl std::error::Error for SandboxTransformError {}

// ---------------------------------------------------------------------------
// SandboxManager
// ---------------------------------------------------------------------------

/// Transforms a `CommandSpec` into an `ExecParams` respecting the active
/// `SandboxPolicy`.
///
/// ## Current implementation (Spec 2 — minimal viable)
///
/// - `DangerFullAccess` → no sandbox, identity transform
/// - `ExternalSandbox` → no sandbox, identity transform (external manages it)
/// - `WorkspaceWrite` / `ReadOnly` → currently falls back to host execution
///   (no bwrap/landlock yet), but guarantees the shell binary is visible
///   because we run on the host rootfs.
///
/// ## Future (post-Spec 2)
///
/// - `WorkspaceWrite` → bwrap/landlock with bind-mounts for /bin, /usr,
///   /lib, /lib64, /etc + writable workspace roots.
/// - `ReadOnly` → bwrap with read-only overlay.
pub struct SandboxManager;

impl SandboxManager {
  /// Transform a `CommandSpec` + `SandboxPolicy` into executable `ExecParams`.
  ///
  /// ## Spec 2.2 — WorkspaceWrite guarantees
  ///
  /// In WorkspaceWrite/ReadOnly mode, shell binaries and their dependencies
  /// MUST be executable. Currently we achieve this by running on the host
  /// rootfs (no isolation). When bwrap/landlock is implemented, we must
  /// bind-mount /bin, /usr, /lib, /lib64 at minimum.
  ///
  /// ## Spec 2.3 — Fallback strategy
  ///
  /// If sandbox transform cannot guarantee executability (e.g. bwrap not
  /// installed), we fall back to `ResolvedSandboxKind::None` (host execution)
  /// for `DangerFullAccess` and `ExternalSandbox`. For `WorkspaceWrite` and
  /// `ReadOnly`, we currently always fall back to host execution with a
  /// warning logged.
  pub fn transform(
    request: SandboxTransformRequest,
  ) -> Result<SandboxTransformResult, SandboxTransformError> {
    let SandboxTransformRequest {
      command_spec,
      policy,
    } = request;

    match &policy {
      SandboxPolicy::DangerFullAccess => {
        // No sandbox at all — identity transform.
        Ok(SandboxTransformResult {
          exec_params: command_spec.into_exec_params(),
          sandbox_kind: ResolvedSandboxKind::None,
        })
      }
      SandboxPolicy::ExternalSandbox { .. } => {
        // External sandbox manages isolation — identity transform.
        Ok(SandboxTransformResult {
          exec_params: command_spec.into_exec_params(),
          sandbox_kind: ResolvedSandboxKind::None,
        })
      }
      SandboxPolicy::WorkspaceWrite { .. } | SandboxPolicy::ReadOnly { .. } => {
        // Spec 2.2: For now, run on host rootfs to guarantee shell
        // binary visibility. Future: bwrap/landlock with bind-mounts.
        //
        // Spec 2.3: This IS the fallback — host execution.
        // When bwrap is implemented, this branch will attempt sandbox
        // transform first, and only fall back to host if bwrap is
        // missing.
        tracing::debug!(
          "sandbox policy {:?} — falling back to host execution (no bwrap/landlock yet)",
          policy_kind_str(&policy)
        );

        Ok(SandboxTransformResult {
          exec_params: command_spec.into_exec_params(),
          sandbox_kind: ResolvedSandboxKind::Policy,
        })
      }
    }
  }
}

fn policy_kind_str(policy: &SandboxPolicy) -> &'static str {
  match policy {
    SandboxPolicy::DangerFullAccess => "DangerFullAccess",
    SandboxPolicy::ExternalSandbox { .. } => "ExternalSandbox",
    SandboxPolicy::WorkspaceWrite { .. } => "WorkspaceWrite",
    SandboxPolicy::ReadOnly { .. } => "ReadOnly",
  }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;
  use cokra_protocol::ReadOnlyAccess;

  fn basic_command_spec() -> CommandSpec {
    CommandSpec {
      command: vec!["/bin/bash".to_string(), "-c".to_string(), "pwd".to_string()],
      cwd: PathBuf::from("/tmp"),
      env: HashMap::new(),
      expiration: ExecExpiration::DefaultTimeout,
      sandbox_permissions: SandboxPermissions::Default,
      windows_sandbox_level: WindowsSandboxLevel::Disabled,
      network: None,
      network_attempt_id: None,
      justification: None,
      arg0: None,
    }
  }

  #[test]
  fn danger_full_access_is_identity_transform() {
    let result = SandboxManager::transform(SandboxTransformRequest {
      command_spec: basic_command_spec(),
      policy: SandboxPolicy::DangerFullAccess,
    })
    .unwrap();

    assert_eq!(result.sandbox_kind, ResolvedSandboxKind::None);
    assert_eq!(result.exec_params.command[0], "/bin/bash");
  }

  #[test]
  fn workspace_write_falls_back_to_host() {
    let result = SandboxManager::transform(SandboxTransformRequest {
      command_spec: basic_command_spec(),
      policy: SandboxPolicy::WorkspaceWrite {
        writable_roots: vec!["/tmp".to_string()],
        read_only_access: ReadOnlyAccess::FullAccess,
        network_access: false,
        exclude_tmpdir_env_var: false,
        exclude_slash_tmp: false,
      },
    })
    .unwrap();

    assert_eq!(result.sandbox_kind, ResolvedSandboxKind::Policy);
    assert_eq!(result.exec_params.command[0], "/bin/bash");
  }

  #[test]
  fn read_only_falls_back_to_host() {
    let result = SandboxManager::transform(SandboxTransformRequest {
      command_spec: basic_command_spec(),
      policy: SandboxPolicy::ReadOnly {
        access: ReadOnlyAccess::FullAccess,
      },
    })
    .unwrap();

    assert_eq!(result.sandbox_kind, ResolvedSandboxKind::Policy);
  }

  #[test]
  fn external_sandbox_is_identity() {
    let result = SandboxManager::transform(SandboxTransformRequest {
      command_spec: basic_command_spec(),
      policy: SandboxPolicy::ExternalSandbox {
        network_access: cokra_protocol::NetworkAccess::None,
      },
    })
    .unwrap();

    assert_eq!(result.sandbox_kind, ResolvedSandboxKind::None);
  }
}
