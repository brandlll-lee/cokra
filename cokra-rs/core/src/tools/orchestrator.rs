use crate::tools::network_approval::DeferredNetworkApproval;
use crate::tools::network_approval::NetworkApprovalMode;
use crate::tools::network_approval::begin_network_approval;
use crate::tools::network_approval::finish_deferred_network_approval;
use crate::tools::network_approval::finish_immediate_network_approval;
use crate::tools::sandboxing::ApprovalCtx;
use crate::tools::sandboxing::ApprovalStore;
use crate::tools::sandboxing::ExecApprovalRequirement;
use crate::tools::sandboxing::SandboxAttempt;
use crate::tools::sandboxing::SandboxKind;
use crate::tools::sandboxing::SandboxOverride;
use crate::tools::sandboxing::SandboxablePreference;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::sandboxing::ToolError;
use crate::tools::sandboxing::ToolRuntime;
use crate::tools::sandboxing::default_exec_approval_requirement;
use cokra_protocol::AskForApproval;
use cokra_protocol::ReviewDecision;
use cokra_protocol::SandboxPolicy;
use tokio::sync::Mutex;

/// Central place for approvals + sandbox selection + retry semantics.
pub struct ToolOrchestrator {
  approval_store: Mutex<ApprovalStore>,
}

pub struct OrchestratorRunResult<Out> {
  pub output: Out,
  pub deferred_network_approval: Option<DeferredNetworkApproval>,
}

impl ToolOrchestrator {
  pub fn new() -> Self {
    Self {
      approval_store: Mutex::new(ApprovalStore::new()),
    }
  }

  async fn has_cached_always<K>(&self, keys: &[K]) -> bool
  where
    K: serde::Serialize,
  {
    let store = self.approval_store.lock().await;
    keys
      .iter()
      .all(|key| matches!(store.get(key), Some(ReviewDecision::Always)))
  }

  async fn cache_always<K>(&self, keys: &[K])
  where
    K: Clone + serde::Serialize,
  {
    let mut store = self.approval_store.lock().await;
    for key in keys.iter().cloned() {
      store.put(key, ReviewDecision::Always);
    }
  }

  async fn run_attempt<Rq, Out, T>(
    tool: &mut T,
    req: &Rq,
    tool_ctx: &ToolCtx<'_>,
    attempt: &SandboxAttempt<'_>,
  ) -> (Result<Out, ToolError>, Option<DeferredNetworkApproval>)
  where
    T: ToolRuntime<Rq, Out>,
  {
    let network_approval = begin_network_approval(
      tool_ctx.session,
      &tool_ctx.turn.turn_id,
      &tool_ctx.call_id,
      tool_ctx.turn.has_managed_network_requirements,
      tool.network_approval_spec(req, tool_ctx),
    )
    .await;

    let attempt_tool_ctx = ToolCtx {
      session: tool_ctx.session,
      turn: tool_ctx.turn,
      call_id: tool_ctx.call_id.clone(),
      tool_name: tool_ctx.tool_name.clone(),
      network_attempt_id: network_approval
        .as_ref()
        .and_then(|approval| approval.attempt_id().map(ToString::to_string)),
    };
    let run_result = tool.run(req, attempt, &attempt_tool_ctx).await;

    let Some(network_approval) = network_approval else {
      return (run_result, None);
    };

    match network_approval.mode() {
      NetworkApprovalMode::Immediate => {
        let finalize = finish_immediate_network_approval(tool_ctx.session, network_approval).await;
        if let Err(err) = finalize {
          return (Err(err), None);
        }
        (run_result, None)
      }
      NetworkApprovalMode::Deferred => {
        let deferred = network_approval.into_deferred();
        if run_result.is_err() {
          finish_deferred_network_approval(tool_ctx.session, deferred).await;
          return (run_result, None);
        }
        (run_result, deferred)
      }
    }
  }

  pub async fn run<Rq, Out, T>(
    &self,
    tool: &mut T,
    req: &Rq,
    tool_ctx: &ToolCtx<'_>,
  ) -> Result<OrchestratorRunResult<Out>, ToolError>
  where
    T: ToolRuntime<Rq, Out>,
  {
    // 1) Approval phase.
    let mut already_approved = false;
    let requirement = tool.exec_approval_requirement(req).unwrap_or_else(|| {
      default_exec_approval_requirement(
        tool_ctx.turn.approval_policy.clone(),
        &tool_ctx.turn.sandbox_policy,
      )
    });

    match requirement {
      ExecApprovalRequirement::Skip { .. } => {}
      ExecApprovalRequirement::Forbidden { reason } => return Err(ToolError::Rejected(reason)),
      ExecApprovalRequirement::NeedsApproval { reason } => {
        let approval_ctx = ApprovalCtx {
          session: tool_ctx.session,
          turn: tool_ctx.turn,
          call_id: &tool_ctx.call_id,
          retry_reason: reason,
          network_approval_reason: None,
        };
        let approval_keys = tool.approval_keys(req);
        let decision = if approval_keys.is_empty() {
          tool.start_approval_async(req, approval_ctx).await
        } else if self.has_cached_always(&approval_keys).await {
          ReviewDecision::Always
        } else {
          let decision = tool.start_approval_async(req, approval_ctx).await;
          if matches!(decision, ReviewDecision::Always) {
            self.cache_always(&approval_keys).await;
          }
          decision
        };

        match decision {
          ReviewDecision::Denied => {
            return Err(ToolError::Rejected("rejected by user".to_string()));
          }
          ReviewDecision::Approved | ReviewDecision::Always => {}
        }
        already_approved = true;
      }
    }

    // 2) First attempt under selected sandbox.
    let initial_sandbox = match tool.sandbox_mode_for_first_attempt(req) {
      SandboxOverride::BypassSandboxFirstAttempt => SandboxKind::None,
      SandboxOverride::NoOverride => {
        select_initial_sandbox(&tool_ctx.turn.sandbox_policy, tool.sandbox_preference())
      }
    };
    let initial_attempt = SandboxAttempt {
      sandbox: initial_sandbox,
      policy: &tool_ctx.turn.sandbox_policy,
      enforce_managed_network: tool_ctx.turn.has_managed_network_requirements,
      sandbox_cwd: &tool_ctx.turn.cwd,
    };
    let (first_result, first_deferred_network_approval) =
      Self::run_attempt(tool, req, tool_ctx, &initial_attempt).await;

    match first_result {
      Ok(output) => Ok(OrchestratorRunResult {
        output,
        deferred_network_approval: first_deferred_network_approval,
      }),
      Err(ToolError::SandboxDenied {
        output,
        network_policy_reason,
      }) => {
        if !tool.escalate_on_failure() {
          return Err(ToolError::SandboxDenied {
            output,
            network_policy_reason,
          });
        }

        if !tool.wants_no_sandbox_approval(tool_ctx.turn.approval_policy.clone()) {
          let allow_on_request_network_prompt =
            matches!(tool_ctx.turn.approval_policy, AskForApproval::OnRequest)
              && network_policy_reason.is_some()
              && matches!(
                default_exec_approval_requirement(
                  tool_ctx.turn.approval_policy.clone(),
                  &tool_ctx.turn.sandbox_policy
                ),
                ExecApprovalRequirement::NeedsApproval { .. }
              );
          if !allow_on_request_network_prompt {
            return Err(ToolError::SandboxDenied {
              output,
              network_policy_reason,
            });
          }
        }

        let retry_reason = if let Some(network_reason) = network_policy_reason.clone() {
          network_reason
        } else {
          build_denial_reason_from_output(&output)
        };

        let bypass_retry_approval = tool
          .should_bypass_approval(tool_ctx.turn.approval_policy.clone(), already_approved)
          && network_policy_reason.is_none();
        if !bypass_retry_approval {
          let approval_ctx = ApprovalCtx {
            session: tool_ctx.session,
            turn: tool_ctx.turn,
            call_id: &tool_ctx.call_id,
            retry_reason: Some(retry_reason),
            network_approval_reason: network_policy_reason.clone(),
          };
          let decision = tool.start_approval_async(req, approval_ctx).await;
          if matches!(decision, ReviewDecision::Denied) {
            return Err(ToolError::Rejected("rejected by user".to_string()));
          }
        }

        // 4) Retry without sandbox.
        let escalated_attempt = SandboxAttempt {
          sandbox: SandboxKind::None,
          policy: &tool_ctx.turn.sandbox_policy,
          enforce_managed_network: tool_ctx.turn.has_managed_network_requirements,
          sandbox_cwd: &tool_ctx.turn.cwd,
        };

        let (retry_result, retry_deferred_network_approval) =
          Self::run_attempt(tool, req, tool_ctx, &escalated_attempt).await;
        retry_result.map(|output| OrchestratorRunResult {
          output,
          deferred_network_approval: retry_deferred_network_approval,
        })
      }
      Err(err) => Err(err),
    }
  }
}

fn select_initial_sandbox(
  policy: &SandboxPolicy,
  preference: SandboxablePreference,
) -> SandboxKind {
  match preference {
    SandboxablePreference::Forbid => SandboxKind::None,
    SandboxablePreference::Require => SandboxKind::Policy,
    SandboxablePreference::Auto => match policy {
      SandboxPolicy::DangerFullAccess => SandboxKind::None,
      _ => SandboxKind::Policy,
    },
  }
}

fn build_denial_reason_from_output(_output: &str) -> String {
  "command failed; retry without sandbox?".to_string()
}

impl Default for ToolOrchestrator {
  fn default() -> Self {
    Self::new()
  }
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use async_trait::async_trait;

  use super::*;
  use crate::session::Session;
  use crate::tools::network_approval::NetworkApprovalMode;
  use crate::tools::network_approval::NetworkApprovalOutcome;
  use crate::tools::network_approval::NetworkApprovalSpec;
  use crate::tools::network_approval::is_network_approval_attempt_active;
  use crate::tools::network_approval::record_network_approval_outcome;
  use crate::tools::sandboxing::ToolTurnContext;

  #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
  struct MockReq {
    command: String,
  }

  struct MockRuntime {
    requirement: ExecApprovalRequirement,
    decision: ReviewDecision,
    first_attempt_denied: bool,
    escalate: bool,
    network_spec: Option<NetworkApprovalSpec>,
    network_outcome: Option<NetworkApprovalOutcome>,
    fail_with_execution_error: bool,
    last_attempt_id: Option<String>,
    run_count: usize,
  }

  impl MockRuntime {
    fn new(requirement: ExecApprovalRequirement) -> Self {
      Self {
        requirement,
        decision: ReviewDecision::Approved,
        first_attempt_denied: false,
        escalate: true,
        network_spec: None,
        network_outcome: None,
        fail_with_execution_error: false,
        last_attempt_id: None,
        run_count: 0,
      }
    }
  }

  #[async_trait]
  impl crate::tools::sandboxing::Approvable<MockReq> for MockRuntime {
    type ApprovalKey = String;

    fn approval_keys(&self, _req: &MockReq) -> Vec<Self::ApprovalKey> {
      vec!["mock-key".to_string()]
    }

    fn exec_approval_requirement(&self, _req: &MockReq) -> Option<ExecApprovalRequirement> {
      Some(self.requirement.clone())
    }

    async fn start_approval_async(
      &mut self,
      _req: &MockReq,
      _ctx: ApprovalCtx<'_>,
    ) -> ReviewDecision {
      self.decision.clone()
    }
  }

  impl crate::tools::sandboxing::Sandboxable for MockRuntime {
    fn sandbox_preference(&self) -> SandboxablePreference {
      SandboxablePreference::Auto
    }

    fn escalate_on_failure(&self) -> bool {
      self.escalate
    }
  }

  #[async_trait]
  impl ToolRuntime<MockReq, String> for MockRuntime {
    fn network_approval_spec(
      &self,
      _req: &MockReq,
      _ctx: &ToolCtx<'_>,
    ) -> Option<NetworkApprovalSpec> {
      self.network_spec.clone()
    }

    async fn run(
      &mut self,
      _req: &MockReq,
      _attempt: &SandboxAttempt<'_>,
      ctx: &ToolCtx<'_>,
    ) -> Result<String, ToolError> {
      self.run_count += 1;
      if self.run_count == 1 && self.first_attempt_denied {
        return Err(ToolError::sandbox_denied("sandbox denied"));
      }

      self.last_attempt_id = ctx.network_attempt_id.clone();
      if self.fail_with_execution_error {
        return Err(ToolError::Execution("runtime failure".to_string()));
      }

      if let (Some(outcome), Some(attempt_id)) =
        (self.network_outcome.clone(), ctx.network_attempt_id.clone())
      {
        record_network_approval_outcome(&attempt_id, outcome).await;
      }

      Ok("ok".to_string())
    }
  }

  fn ctx() -> (Session, ToolTurnContext) {
    let session = Session::new();
    let turn = ToolTurnContext {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      cwd: PathBuf::from("."),
      tx_event: None,
      approval_policy: AskForApproval::OnRequest,
      sandbox_policy: SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![".".to_string()],
        read_only_access: cokra_protocol::ReadOnlyAccess::FullAccess,
        network_access: false,
        exclude_tmpdir_env_var: false,
        exclude_slash_tmp: false,
      },
      has_managed_network_requirements: false,
    };
    (session, turn)
  }

  #[tokio::test]
  async fn skip_runs_directly() {
    let (session, turn) = ctx();
    let orchestrator = ToolOrchestrator::new();
    let mut runtime = MockRuntime::new(ExecApprovalRequirement::Skip {
      bypass_sandbox: false,
    });
    let req = MockReq {
      command: "echo".to_string(),
    };
    let tool_ctx = ToolCtx {
      session: &session,
      turn: &turn,
      call_id: "call-1".to_string(),
      tool_name: "mock".to_string(),
      network_attempt_id: None,
    };

    let result = orchestrator.run(&mut runtime, &req, &tool_ctx).await;
    assert!(result.is_ok());
    assert_eq!(runtime.run_count, 1);
  }

  #[tokio::test]
  async fn forbidden_returns_rejected() {
    let (session, turn) = ctx();
    let orchestrator = ToolOrchestrator::new();
    let mut runtime = MockRuntime::new(ExecApprovalRequirement::Forbidden {
      reason: "forbidden".to_string(),
    });
    let req = MockReq {
      command: "echo".to_string(),
    };
    let tool_ctx = ToolCtx {
      session: &session,
      turn: &turn,
      call_id: "call-1".to_string(),
      tool_name: "mock".to_string(),
      network_attempt_id: None,
    };

    let result = orchestrator.run(&mut runtime, &req, &tool_ctx).await;
    assert!(matches!(result, Err(ToolError::Rejected(_))));
    assert_eq!(runtime.run_count, 0);
  }

  #[tokio::test]
  async fn needs_approval_denied_stops_execution() {
    let (session, turn) = ctx();
    let orchestrator = ToolOrchestrator::new();
    let mut runtime = MockRuntime::new(ExecApprovalRequirement::NeedsApproval { reason: None });
    runtime.decision = ReviewDecision::Denied;
    let req = MockReq {
      command: "echo".to_string(),
    };
    let tool_ctx = ToolCtx {
      session: &session,
      turn: &turn,
      call_id: "call-1".to_string(),
      tool_name: "mock".to_string(),
      network_attempt_id: None,
    };

    let result = orchestrator.run(&mut runtime, &req, &tool_ctx).await;
    assert!(matches!(result, Err(ToolError::Rejected(_))));
    assert_eq!(runtime.run_count, 0);
  }

  #[tokio::test]
  async fn denied_without_escalation_returns_denied() {
    let (session, turn) = ctx();
    let orchestrator = ToolOrchestrator::new();
    let mut runtime = MockRuntime::new(ExecApprovalRequirement::Skip {
      bypass_sandbox: false,
    });
    runtime.first_attempt_denied = true;
    runtime.escalate = false;
    let req = MockReq {
      command: "echo".to_string(),
    };
    let tool_ctx = ToolCtx {
      session: &session,
      turn: &turn,
      call_id: "call-1".to_string(),
      tool_name: "mock".to_string(),
      network_attempt_id: None,
    };

    let result = orchestrator.run(&mut runtime, &req, &tool_ctx).await;
    assert!(matches!(result, Err(ToolError::SandboxDenied { .. })));
    assert_eq!(runtime.run_count, 1);
  }

  #[tokio::test]
  async fn denied_with_escalation_retries() {
    let (session, mut turn) = ctx();
    turn.approval_policy = AskForApproval::UnlessTrusted;
    let orchestrator = ToolOrchestrator::new();
    let mut runtime = MockRuntime::new(ExecApprovalRequirement::Skip {
      bypass_sandbox: false,
    });
    runtime.first_attempt_denied = true;
    let req = MockReq {
      command: "echo".to_string(),
    };
    let tool_ctx = ToolCtx {
      session: &session,
      turn: &turn,
      call_id: "call-1".to_string(),
      tool_name: "mock".to_string(),
      network_attempt_id: None,
    };

    let result = orchestrator.run(&mut runtime, &req, &tool_ctx).await;
    assert!(result.is_ok());
    assert_eq!(runtime.run_count, 2);
  }

  #[tokio::test]
  async fn network_immediate_finalize_surfaces_denial() {
    let orchestrator = ToolOrchestrator::new();
    let mut runtime = MockRuntime::new(ExecApprovalRequirement::Skip {
      bypass_sandbox: false,
    });
    runtime.network_spec = Some(NetworkApprovalSpec {
      mode: NetworkApprovalMode::Immediate,
    });
    runtime.network_outcome = Some(NetworkApprovalOutcome::DeniedByPolicy(
      "network denied".to_string(),
    ));

    let session = Session::new();
    let turn = ToolTurnContext {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      cwd: PathBuf::from("."),
      tx_event: None,
      approval_policy: AskForApproval::OnRequest,
      sandbox_policy: SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![".".to_string()],
        read_only_access: cokra_protocol::ReadOnlyAccess::FullAccess,
        network_access: false,
        exclude_tmpdir_env_var: false,
        exclude_slash_tmp: false,
      },
      has_managed_network_requirements: true,
    };

    let req = MockReq {
      command: "echo".to_string(),
    };
    let tool_ctx = ToolCtx {
      session: &session,
      turn: &turn,
      call_id: "call-1".to_string(),
      tool_name: "mock".to_string(),
      network_attempt_id: None,
    };

    let result = orchestrator.run(&mut runtime, &req, &tool_ctx).await;
    assert!(matches!(result, Err(ToolError::Rejected(_))));
  }

  #[tokio::test]
  async fn deferred_network_failure_cleans_up_attempt() {
    let orchestrator = ToolOrchestrator::new();
    let mut runtime = MockRuntime::new(ExecApprovalRequirement::Skip {
      bypass_sandbox: false,
    });
    runtime.network_spec = Some(NetworkApprovalSpec {
      mode: NetworkApprovalMode::Deferred,
    });
    runtime.fail_with_execution_error = true;

    let session = Session::new();
    let turn = ToolTurnContext {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      cwd: PathBuf::from("."),
      tx_event: None,
      approval_policy: AskForApproval::OnRequest,
      sandbox_policy: SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![".".to_string()],
        read_only_access: cokra_protocol::ReadOnlyAccess::FullAccess,
        network_access: false,
        exclude_tmpdir_env_var: false,
        exclude_slash_tmp: false,
      },
      has_managed_network_requirements: true,
    };
    let req = MockReq {
      command: "echo".to_string(),
    };
    let tool_ctx = ToolCtx {
      session: &session,
      turn: &turn,
      call_id: "call-1".to_string(),
      tool_name: "mock".to_string(),
      network_attempt_id: None,
    };

    let result = orchestrator.run(&mut runtime, &req, &tool_ctx).await;
    assert!(matches!(result, Err(ToolError::Execution(_))));
    let attempt_id = runtime
      .last_attempt_id
      .clone()
      .expect("network attempt id should be captured");
    assert!(!is_network_approval_attempt_active(&attempt_id).await);
  }
}
