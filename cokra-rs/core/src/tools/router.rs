use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::session::Session;
use crate::tools::events::{ToolEmitter, ToolEventCtx, ToolEventStage};
use crate::tools::network_approval::NetworkApprovalSpec;
use crate::tools::orchestrator::ToolOrchestrator;
use crate::tools::registry::ToolRegistry;
use crate::tools::sandboxing::{
  Approvable, ApprovalCtx, ExecApprovalRequirement, SandboxAttempt, Sandboxable,
  SandboxablePreference, ToolCtx, ToolError, ToolRuntime, ToolTurnContext,
};
use crate::tools::spec::ToolSpec;
use crate::tools::validation::{ToolCall as ValidationToolCall, ToolValidator};
use crate::tools::{network_approval::NetworkApprovalMode, orchestrator::OrchestratorRunResult};
use cokra_protocol::{
  AskForApproval, EventMsg, ExecApprovalRequestEvent, ReviewDecision, SandboxPolicy,
};

use crate::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};

#[derive(Clone, Debug)]
pub struct ToolCall {
  pub tool_name: String,
  pub call_id: String,
  pub args: Value,
}

#[derive(Clone)]
pub struct ToolRunContext {
  pub session: Arc<Session>,
  pub tx_event: Option<mpsc::Sender<EventMsg>>,
  pub thread_id: String,
  pub turn_id: String,
  pub cwd: PathBuf,
  pub approval_policy: AskForApproval,
  pub sandbox_policy: SandboxPolicy,
  pub has_managed_network_requirements: bool,
  pub auto_approve_on_request: bool,
}

impl ToolRunContext {
  pub fn new(
    session: Arc<Session>,
    thread_id: String,
    turn_id: String,
    cwd: PathBuf,
    approval_policy: AskForApproval,
    sandbox_policy: SandboxPolicy,
  ) -> Self {
    Self {
      session,
      tx_event: None,
      thread_id,
      turn_id,
      cwd,
      approval_policy,
      sandbox_policy,
      has_managed_network_requirements: false,
      auto_approve_on_request: true,
    }
  }
}

pub struct ToolRouter {
  registry: Arc<ToolRegistry>,
  validator: Arc<ToolValidator>,
  orchestrator: Arc<Mutex<ToolOrchestrator>>,
}

impl ToolRouter {
  pub fn new(registry: Arc<ToolRegistry>, validator: Arc<ToolValidator>) -> Self {
    Self {
      registry,
      validator,
      orchestrator: Arc::new(Mutex::new(ToolOrchestrator::new())),
    }
  }

  pub async fn route_tool_call(
    &self,
    tool_name: &str,
    arguments: Value,
    ctx: ToolRunContext,
  ) -> Result<ToolOutput, FunctionCallError> {
    let call = ToolCall {
      tool_name: tool_name.to_string(),
      call_id: Uuid::new_v4().to_string(),
      args: arguments,
    };
    self.dispatch_tool_call(call, ctx).await
  }

  pub async fn dispatch_tool_call(
    &self,
    call: ToolCall,
    run_ctx: ToolRunContext,
  ) -> Result<ToolOutput, FunctionCallError> {
    self.validate_call(&call)?;

    let mut runtime = RegistryToolRuntime::new(
      Arc::clone(&self.registry),
      self.registry.get_spec(&call.tool_name).cloned(),
      run_ctx.approval_policy.clone(),
      run_ctx.auto_approve_on_request,
    );
    let turn_ctx = ToolTurnContext {
      thread_id: run_ctx.thread_id.clone(),
      turn_id: run_ctx.turn_id.clone(),
      cwd: run_ctx.cwd.clone(),
      tx_event: run_ctx.tx_event.clone(),
      approval_policy: run_ctx.approval_policy,
      sandbox_policy: run_ctx.sandbox_policy.clone(),
      has_managed_network_requirements: run_ctx.has_managed_network_requirements,
    };
    let tool_ctx = ToolCtx {
      session: run_ctx.session.as_ref(),
      turn: &turn_ctx,
      call_id: call.call_id.clone(),
      tool_name: call.tool_name.clone(),
      network_attempt_id: None,
    };

    let emitter = ToolEmitter::new(call.tool_name.clone());
    let event_ctx = ToolEventCtx {
      session: run_ctx.session.as_ref(),
      tx_event: run_ctx.tx_event.clone(),
      thread_id: &run_ctx.thread_id,
      turn_id: &run_ctx.turn_id,
      call_id: &call.call_id,
      tool_name: &call.tool_name,
      cwd: &run_ctx.cwd,
    };
    emitter.begin(event_ctx.clone()).await;

    let mut orchestrator = self.orchestrator.lock().await;
    let result = orchestrator.run(&mut runtime, &call, &tool_ctx).await;
    drop(orchestrator);

    match result {
      Ok(OrchestratorRunResult {
        output,
        deferred_network_approval,
      }) => {
        if deferred_network_approval.is_some() {
          crate::tools::network_approval::finish_deferred_network_approval(
            run_ctx.session.as_ref(),
            deferred_network_approval,
          )
          .await;
        }
        emitter
          .emit(event_ctx.clone(), ToolEventStage::Success(output.clone()))
          .await;
        Ok(output)
      }
      Err(err) => {
        let fc_err = map_tool_error(err);
        emitter
          .emit(event_ctx.clone(), ToolEventStage::Failure(fc_err.clone()))
          .await;
        Err(fc_err)
      }
    }
  }

  pub fn tool_supports_parallel(&self, call: &ToolCall) -> bool {
    let invocation = ToolInvocation {
      id: call.call_id.clone(),
      name: call.tool_name.clone(),
      arguments: call.args.to_string(),
    };
    match self.registry.is_mutating(&invocation) {
      Ok(is_mutating) => !is_mutating,
      Err(_) => false,
    }
  }

  pub fn list_available_tools(&self) -> Vec<ToolSpec> {
    self.registry.list_specs()
  }

  pub fn registry(&self) -> Arc<ToolRegistry> {
    self.registry.clone()
  }

  fn validate_call(&self, call: &ToolCall) -> Result<(), FunctionCallError> {
    let validation = ValidationToolCall {
      tool_name: call.tool_name.clone(),
      args: call.args.clone(),
    };
    self
      .validator
      .validate_tool_call(&validation)
      .map_err(FunctionCallError::from)?;
    Ok(())
  }
}

struct RegistryToolRuntime {
  registry: Arc<ToolRegistry>,
  spec: Option<ToolSpec>,
  approval_policy: AskForApproval,
  auto_approve_on_request: bool,
}

impl RegistryToolRuntime {
  fn new(
    registry: Arc<ToolRegistry>,
    spec: Option<ToolSpec>,
    approval_policy: AskForApproval,
    auto_approve_on_request: bool,
  ) -> Self {
    Self {
      registry,
      spec,
      approval_policy,
      auto_approve_on_request,
    }
  }
}

#[async_trait]
impl Approvable<ToolCall> for RegistryToolRuntime {
  type ApprovalKey = String;

  fn approval_keys(&self, req: &ToolCall) -> Vec<Self::ApprovalKey> {
    vec![format!("{}:{}", req.tool_name, req.args)]
  }

  fn exec_approval_requirement(&self, req: &ToolCall) -> Option<ExecApprovalRequirement> {
    let requires_approval = self
      .spec
      .as_ref()
      .map(|spec| spec.permissions.requires_approval)
      .unwrap_or(false);

    if !requires_approval {
      return Some(ExecApprovalRequirement::Skip {
        bypass_sandbox: false,
      });
    }

    match self.approval_policy {
      AskForApproval::Never => Some(ExecApprovalRequirement::Forbidden {
        reason: format!("tool {} is blocked by approval policy", req.tool_name),
      }),
      AskForApproval::OnFailure => Some(ExecApprovalRequirement::Skip {
        bypass_sandbox: false,
      }),
      AskForApproval::OnRequest | AskForApproval::UnlessTrusted => {
        Some(ExecApprovalRequirement::NeedsApproval {
          reason: Some(format!("Execute {}?", req.tool_name)),
        })
      }
    }
  }

  async fn start_approval_async(&mut self, req: &ToolCall, ctx: ApprovalCtx<'_>) -> ReviewDecision {
    ctx
      .session
      .emit_event(EventMsg::ExecApprovalRequest(ExecApprovalRequestEvent {
        thread_id: ctx.turn.thread_id.clone(),
        turn_id: ctx.turn.turn_id.clone(),
        id: ctx.call_id.to_string(),
        command: req.tool_name.clone(),
        cwd: ctx.turn.cwd.clone(),
      }));
    if let Some(tx_event) = &ctx.turn.tx_event {
      let _ = tx_event
        .send(EventMsg::ExecApprovalRequest(ExecApprovalRequestEvent {
          thread_id: ctx.turn.thread_id.clone(),
          turn_id: ctx.turn.turn_id.clone(),
          id: ctx.call_id.to_string(),
          command: req.tool_name.clone(),
          cwd: ctx.turn.cwd.clone(),
        }))
        .await;
    }

    if self.auto_approve_on_request {
      ReviewDecision::Approved
    } else {
      ReviewDecision::Denied
    }
  }
}

impl Sandboxable for RegistryToolRuntime {
  fn sandbox_preference(&self) -> SandboxablePreference {
    SandboxablePreference::Auto
  }
}

#[async_trait]
impl ToolRuntime<ToolCall, ToolOutput> for RegistryToolRuntime {
  fn network_approval_spec(
    &self,
    req: &ToolCall,
    _ctx: &ToolCtx<'_>,
  ) -> Option<NetworkApprovalSpec> {
    let mode = req
      .args
      .get("__network_approval_mode")
      .and_then(Value::as_str)
      .and_then(|mode| match mode {
        "immediate" => Some(NetworkApprovalMode::Immediate),
        "deferred" => Some(NetworkApprovalMode::Deferred),
        _ => None,
      })?;

    Some(NetworkApprovalSpec { mode })
  }

  async fn run(
    &mut self,
    req: &ToolCall,
    attempt: &SandboxAttempt<'_>,
    _ctx: &ToolCtx<'_>,
  ) -> Result<ToolOutput, ToolError> {
    let invocation = ToolInvocation {
      id: req.call_id.clone(),
      name: req.tool_name.clone(),
      arguments: req.args.to_string(),
    };

    match self.registry.dispatch(invocation) {
      Ok(output) => Ok(output),
      Err(err) => {
        let message = err.to_string();
        if attempt.sandbox != crate::tools::sandboxing::SandboxKind::None
          && looks_like_sandbox_denial(&message)
        {
          Err(ToolError::sandbox_denied(message))
        } else {
          Err(ToolError::Execution(message))
        }
      }
    }
  }
}

fn looks_like_sandbox_denial(message: &str) -> bool {
  let lower = message.to_lowercase();
  lower.contains("sandbox denied")
    || lower.contains("permission denied")
    || lower.contains("operation not permitted")
}

fn map_tool_error(err: ToolError) -> FunctionCallError {
  match err {
    ToolError::Rejected(message) => FunctionCallError::PermissionDenied(message),
    ToolError::SandboxDenied { output, .. } => FunctionCallError::Execution(output),
    ToolError::Execution(message) => FunctionCallError::Execution(message),
  }
}
