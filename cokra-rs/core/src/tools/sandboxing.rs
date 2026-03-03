//! Shared approvals and sandbox contracts used by cokra tool runtimes.

use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::session::Session;
use crate::tools::network_approval::NetworkApprovalSpec;
use cokra_protocol::AskForApproval;
use cokra_protocol::EventMsg;
use cokra_protocol::ReviewDecision;
use cokra_protocol::SandboxPolicy;

/// Approval store with per-key session caching.
#[derive(Clone, Default, Debug)]
pub struct ApprovalStore {
  map: HashMap<String, ReviewDecision>,
}

impl ApprovalStore {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn get<K>(&self, key: &K) -> Option<ReviewDecision>
  where
    K: Serialize,
  {
    let key_str = serde_json::to_string(key).ok()?;
    self.map.get(&key_str).cloned()
  }

  pub fn put<K>(&mut self, key: K, value: ReviewDecision)
  where
    K: Serialize,
  {
    if let Ok(key_str) = serde_json::to_string(&key) {
      self.map.insert(key_str, value);
    }
  }
}

/// Evaluate approval with per-key session cache semantics.
pub async fn with_cached_approval<K, F, Fut>(
  store: &mut ApprovalStore,
  keys: Vec<K>,
  fetch: F,
) -> ReviewDecision
where
  K: Serialize,
  F: FnOnce() -> Fut,
  Fut: std::future::Future<Output = ReviewDecision>,
{
  if keys.is_empty() {
    return fetch().await;
  }

  let all_always = keys
    .iter()
    .all(|key| matches!(store.get(key), Some(ReviewDecision::Always)));
  if all_always {
    return ReviewDecision::Always;
  }

  let decision = fetch().await;
  if matches!(decision, ReviewDecision::Always) {
    for key in keys {
      store.put(key, ReviewDecision::Always);
    }
  }
  decision
}

#[derive(Clone)]
pub struct ApprovalCtx<'a> {
  pub session: &'a Session,
  pub turn: &'a ToolTurnContext,
  pub call_id: &'a str,
  pub retry_reason: Option<String>,
  pub network_approval_reason: Option<String>,
}

/// Orchestrator-level approval requirement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecApprovalRequirement {
  /// No approval prompt required.
  Skip {
    /// First attempt can bypass sandbox directly.
    bypass_sandbox: bool,
  },
  /// Prompt for approval.
  NeedsApproval { reason: Option<String> },
  /// Never run this invocation.
  Forbidden { reason: String },
}

/// Default approval requirement derived from policy + sandbox policy.
pub fn default_exec_approval_requirement(
  policy: AskForApproval,
  sandbox_policy: &SandboxPolicy,
) -> ExecApprovalRequirement {
  let needs_approval = match policy {
    AskForApproval::Never | AskForApproval::OnFailure => false,
    AskForApproval::OnRequest => !matches!(
      sandbox_policy,
      SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. }
    ),
    AskForApproval::UnlessTrusted => true,
  };

  if needs_approval {
    ExecApprovalRequirement::NeedsApproval { reason: None }
  } else {
    ExecApprovalRequirement::Skip {
      bypass_sandbox: false,
    }
  }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxOverride {
  NoOverride,
  BypassSandboxFirstAttempt,
}

/// Per-turn context needed by orchestrator/runtime.
#[derive(Clone, Debug)]
pub struct ToolTurnContext {
  pub thread_id: String,
  pub turn_id: String,
  pub cwd: PathBuf,
  pub tx_event: Option<mpsc::Sender<EventMsg>>,
  pub approval_policy: AskForApproval,
  pub sandbox_policy: SandboxPolicy,
  pub has_managed_network_requirements: bool,
}

#[async_trait]
pub trait Approvable<Req> {
  type ApprovalKey: Hash + Eq + Clone + Debug + Serialize + Send + Sync;

  fn approval_keys(&self, req: &Req) -> Vec<Self::ApprovalKey>;

  fn sandbox_mode_for_first_attempt(&self, _req: &Req) -> SandboxOverride {
    SandboxOverride::NoOverride
  }

  fn should_bypass_approval(&self, policy: AskForApproval, already_approved: bool) -> bool {
    if already_approved {
      return true;
    }
    matches!(policy, AskForApproval::Never)
  }

  fn exec_approval_requirement(&self, _req: &Req) -> Option<ExecApprovalRequirement> {
    None
  }

  fn wants_no_sandbox_approval(&self, policy: AskForApproval) -> bool {
    !matches!(policy, AskForApproval::Never | AskForApproval::OnRequest)
  }

  async fn start_approval_async(&mut self, req: &Req, ctx: ApprovalCtx<'_>) -> ReviewDecision;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxablePreference {
  Auto,
  Require,
  Forbid,
}

pub trait Sandboxable {
  fn sandbox_preference(&self) -> SandboxablePreference;

  fn escalate_on_failure(&self) -> bool {
    true
  }
}

pub struct ToolCtx<'a> {
  pub session: &'a Session,
  pub turn: &'a ToolTurnContext,
  pub call_id: String,
  pub tool_name: String,
  pub network_attempt_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxKind {
  Policy,
  None,
}

pub struct SandboxAttempt<'a> {
  pub sandbox: SandboxKind,
  pub policy: &'a SandboxPolicy,
  pub enforce_managed_network: bool,
  pub sandbox_cwd: &'a Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
  Rejected(String),
  SandboxDenied {
    output: String,
    network_policy_reason: Option<String>,
  },
  Execution(String),
}

impl ToolError {
  pub fn sandbox_denied(output: impl Into<String>) -> Self {
    Self::SandboxDenied {
      output: output.into(),
      network_policy_reason: None,
    }
  }

  pub fn sandbox_denied_with_network_reason(
    output: impl Into<String>,
    reason: impl Into<String>,
  ) -> Self {
    Self::SandboxDenied {
      output: output.into(),
      network_policy_reason: Some(reason.into()),
    }
  }
}

impl std::fmt::Display for ToolError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      ToolError::Rejected(msg) => write!(f, "{msg}"),
      ToolError::SandboxDenied { output, .. } => write!(f, "{output}"),
      ToolError::Execution(msg) => write!(f, "{msg}"),
    }
  }
}

impl std::error::Error for ToolError {}

#[async_trait]
pub trait ToolRuntime<Req, Out>: Approvable<Req> + Sandboxable + Send {
  fn network_approval_spec(&self, _req: &Req, _ctx: &ToolCtx<'_>) -> Option<NetworkApprovalSpec> {
    None
  }

  async fn run(
    &mut self,
    req: &Req,
    attempt: &SandboxAttempt<'_>,
    ctx: &ToolCtx<'_>,
  ) -> Result<Out, ToolError>;
}

#[cfg(test)]
mod tests {
  use super::*;
  use cokra_protocol::ReadOnlyAccess;

  #[test]
  fn external_sandbox_skips_on_request_approval() {
    assert_eq!(
      default_exec_approval_requirement(
        AskForApproval::OnRequest,
        &SandboxPolicy::ExternalSandbox {
          network_access: cokra_protocol::NetworkAccess::None,
        },
      ),
      ExecApprovalRequirement::Skip {
        bypass_sandbox: false,
      }
    );
  }

  #[test]
  fn read_only_requires_on_request_approval() {
    assert_eq!(
      default_exec_approval_requirement(
        AskForApproval::OnRequest,
        &SandboxPolicy::ReadOnly {
          access: ReadOnlyAccess::FullAccess
        }
      ),
      ExecApprovalRequirement::NeedsApproval { reason: None }
    );
  }
}
