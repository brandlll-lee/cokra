use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::exec::format_exec_output_for_model_structured;
use crate::exec::SandboxPermissions;
use crate::exec_policy::eval_exec_approval;
use crate::session::Session;
use crate::shell::default_user_shell;
use crate::tools::events::ToolEmitter;
use crate::tools::events::ToolEventCtx;
use crate::tools::events::ToolEventStage;
use crate::tools::network_approval::NetworkApprovalMode;
use crate::tools::network_approval::NetworkApprovalSpec;
use crate::tools::orchestrator::OrchestratorRunResult;
use crate::tools::orchestrator::ToolOrchestrator;
use crate::tools::registry::ToolRegistry;
use crate::tools::sandboxing::Approvable;
use crate::tools::sandboxing::ApprovalCtx;
use crate::tools::sandboxing::ExecApprovalRequirement;
use crate::tools::sandboxing::SandboxAttempt;
use crate::tools::sandboxing::Sandboxable;
use crate::tools::sandboxing::SandboxablePreference;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::sandboxing::ToolError;
use crate::tools::sandboxing::ToolRuntime;
use crate::tools::sandboxing::ToolTurnContext;
use crate::tools::spec::ToolSpec;
use crate::tools::validation::ToolCall as ValidationToolCall;
use crate::tools::validation::ToolValidator;
use crate::truncate::DEFAULT_TOOL_OUTPUT_TOKENS;
use crate::truncate::TruncationPolicy;
use cokra_protocol::AskForApproval;
use cokra_protocol::EventMsg;
use cokra_protocol::ReviewDecision;
use cokra_protocol::SandboxPolicy;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ShellToolCallParams;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::ToolRuntimeContext;
use crate::tools::runtimes::shell::ShellCommandRequest;
use crate::tools::runtimes::shell::ShellRequest;
use crate::tools::runtimes::shell::ShellRuntime;

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
    }
  }
}

#[derive(Clone)]
struct InvocationRuntimeState {
  session: Arc<Session>,
  tx_event: Option<mpsc::Sender<EventMsg>>,
  thread_id: String,
  turn_id: String,
}

pub struct ToolRouter {
  registry: Arc<ToolRegistry>,
  validator: Arc<ToolValidator>,
  orchestrator: Arc<ToolOrchestrator>,
}

impl ToolRouter {
  pub fn new(registry: Arc<ToolRegistry>, validator: Arc<ToolValidator>) -> Self {
    Self {
      registry,
      validator,
      orchestrator: Arc::new(ToolOrchestrator::new()),
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
      run_ctx.sandbox_policy.clone(),
      Some(InvocationRuntimeState {
        session: Arc::clone(&run_ctx.session),
        tx_event: run_ctx.tx_event.clone(),
        thread_id: run_ctx.thread_id.clone(),
        turn_id: run_ctx.turn_id.clone(),
      }),
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

    // 1:1 codex: for shell tool, pass the actual command string so TUI
    // renders "$ pwd" instead of "$ shell".
    let emitter = tool_emitter_for_call(&call, &run_ctx.cwd);
    let emit_exec_events = should_emit_exec_events(&call.tool_name);
    let event_ctx = ToolEventCtx {
      session: run_ctx.session.as_ref(),
      tx_event: run_ctx.tx_event.clone(),
      thread_id: &run_ctx.thread_id,
      turn_id: &run_ctx.turn_id,
      call_id: &call.call_id,
      tool_name: &call.tool_name,
      cwd: &run_ctx.cwd,
    };
    if emit_exec_events {
      emitter.begin(event_ctx.clone()).await;
    }

    let result = self.orchestrator.run(&mut runtime, &call, &tool_ctx).await;

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
        if emit_exec_events {
          emitter
            .emit(event_ctx.clone(), ToolEventStage::Success(output.clone()))
            .await;
        }
        Ok(output)
      }
      Err(err) => {
        let fc_err = map_tool_error(err);
        if emit_exec_events {
          emitter
            .emit(event_ctx.clone(), ToolEventStage::Failure(fc_err.clone()))
            .await;
        }
        Err(fc_err)
      }
    }
  }

  pub fn tool_supports_parallel(&self, call: &ToolCall) -> bool {
    let invocation = ToolInvocation {
      id: call.call_id.clone(),
      name: call.tool_name.clone(),
      payload: ToolPayload::Function {
        arguments: call.args.to_string(),
      },
      // cwd is unused for is_mutating checks, but required by struct.
      cwd: PathBuf::from("."),
      runtime: None,
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

fn tool_emitter_for_call(call: &ToolCall, cwd: &Path) -> ToolEmitter {
  if call.tool_name == "shell" {
    let raw_cmd = call
      .args
      .get("command")
      .and_then(|v| v.as_str())
      .unwrap_or("shell")
      .to_string();
    return ToolEmitter::shell_with_command(raw_cmd);
  }

  let Some(display_command) = summarize_tool_display_command(call, cwd) else {
    return ToolEmitter::new(call.tool_name.clone());
  };
  ToolEmitter::with_display_command(call.tool_name.clone(), display_command)
}

fn should_emit_exec_events(tool_name: &str) -> bool {
  // Tradeoff: team/collab tools now render through dedicated notice cells instead of the
  // generic exec transcript, because duplicating both views produced the exact UX noise the
  // user reported: Running/✓ rows plus raw JSON outputs for the same action.
  !matches!(
    tool_name,
    "spawn_agent"
      | "send_input"
      | "wait"
      | "close_agent"
      | "cleanup_team"
      | "team_status"
      | "send_team_message"
      | "read_team_messages"
      | "create_team_task"
      | "update_team_task"
      | "assign_team_task"
      | "claim_team_task"
      | "claim_next_team_task"
      | "claim_team_messages"
      | "handoff_team_task"
      | "submit_team_plan"
      | "approve_team_plan"
  )
}

fn summarize_tool_display_command(call: &ToolCall, cwd: &Path) -> Option<String> {
  match call.tool_name.as_str() {
    "read_file" => call
      .args
      .get("file_path")
      .and_then(Value::as_str)
      .map(|path| summarize_path_for_display(path, cwd)),
    "list_dir" => call
      .args
      .get("dir_path")
      .and_then(Value::as_str)
      .map(|path| summarize_path_for_display(path, cwd)),
    "grep_files" => {
      let pattern = call.args.get("pattern").and_then(Value::as_str)?;
      let path = call.args.get("path").and_then(Value::as_str);
      Some(match path {
        Some(path) if !path.is_empty() => {
          format!("{pattern} in {}", summarize_path_for_display(path, cwd))
        }
        _ => pattern.to_string(),
      })
    }
    "search_tool" => call
      .args
      .get("query")
      .and_then(Value::as_str)
      .map(ToString::to_string),
    _ => None,
  }
}

fn summarize_path_for_display(path: &str, cwd: &Path) -> String {
  let path = PathBuf::from(path);
  if let Ok(relative) = path.strip_prefix(cwd)
    && !relative.as_os_str().is_empty()
  {
    return relative.display().to_string();
  }
  path
    .file_name()
    .and_then(|name| name.to_str())
    .map(ToString::to_string)
    .filter(|name| !name.is_empty())
    .unwrap_or_else(|| path.display().to_string())
}

struct RegistryToolRuntime {
  registry: Arc<ToolRegistry>,
  spec: Option<ToolSpec>,
  approval_policy: AskForApproval,
  sandbox_policy: SandboxPolicy,
  runtime: Option<InvocationRuntimeState>,
}

impl RegistryToolRuntime {
  fn new(
    registry: Arc<ToolRegistry>,
    spec: Option<ToolSpec>,
    approval_policy: AskForApproval,
    sandbox_policy: SandboxPolicy,
    runtime: Option<InvocationRuntimeState>,
  ) -> Self {
    Self {
      registry,
      spec,
      approval_policy,
      sandbox_policy,
      runtime,
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

    // 1:1 codex: for shell tool, parse the actual command and route through
    // eval_exec_approval() for command safety classification.
    if req.tool_name == "shell"
      && let Some(command_str) = req.args.get("command").and_then(|v| v.as_str())
    {
      let shell = default_user_shell();
      let argv = shell.derive_exec_args(command_str, true);
      return Some(eval_exec_approval(
        &argv,
        &self.sandbox_policy,
        self.approval_policy.clone(),
        SandboxPermissions::UseDefault,
      ));
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
    // 1:1 codex: for shell tool, pass the actual command string so
    // approval prompt shows "$ pwd" instead of "$ shell".
    let display_command = if req.tool_name == "shell" {
      req
        .args
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or(&req.tool_name)
        .to_string()
    } else {
      req.tool_name.clone()
    };

    // 1:1 codex: always block until the user responds (no auto-approve hack).
    ctx
      .session
      .request_exec_approval(
        ctx.turn.thread_id.clone(),
        ctx.turn.turn_id.clone(),
        ctx.call_id.to_string(),
        req.tool_name.clone(),
        display_command,
        ctx.turn.cwd.clone(),
        ctx.turn.tx_event.clone(),
      )
      .await
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
    ctx: &ToolCtx<'_>,
  ) -> Result<ToolOutput, ToolError> {
    if matches!(
      req.tool_name.as_str(),
      "shell" | "container.exec" | "local_shell" | "unified_exec"
    ) {
      return run_shell_tool_call(req, self.approval_policy.clone(), attempt, ctx).await;
    }

    // 1:1 codex: thread session-level cwd into ToolInvocation so handlers
    // resolve paths against the correct working directory.
    let invocation = ToolInvocation {
      id: req.call_id.clone(),
      name: req.tool_name.clone(),
      payload: invocation_payload_for_call(req),
      cwd: ctx.turn.cwd.clone(),
      runtime: self.runtime.as_ref().map(|runtime| {
        Arc::new(ToolRuntimeContext {
          session: Arc::clone(&runtime.session),
          tx_event: runtime.tx_event.clone(),
          thread_id: runtime.thread_id.clone(),
          turn_id: runtime.turn_id.clone(),
        })
      }),
    };

    let is_mutating = self.registry.is_mutating(&invocation).unwrap_or(false);
    if is_mutating
      && let Some(runtime) = &invocation.runtime
      && let Some(team_runtime) = runtime_for_thread(&runtime.thread_id)
      && team_runtime.requires_plan_approval(&runtime.thread_id)
      && !matches!(
        invocation.name.as_str(),
        "approve_team_plan"
          | "submit_team_plan"
          | "team_status"
          | "read_team_messages"
          | "send_team_message"
          | "create_team_task"
          | "update_team_task"
          | "claim_team_task"
          | "cleanup_team"
          | "wait"
          | "close_agent"
      )
    {
      return Err(ToolError::Execution(
        "team plan approval required before mutating work; submit a plan and wait for approval"
          .to_string(),
      ));
    }

    // 1:1 codex: use dispatch_async to support async handlers (e.g. shell).
    match self.registry.dispatch_async(invocation).await {
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

async fn run_shell_tool_call(
  req: &ToolCall,
  approval_policy: AskForApproval,
  attempt: &SandboxAttempt<'_>,
  ctx: &ToolCtx<'_>,
) -> Result<ToolOutput, ToolError> {
  let shell = ctx.session.user_shell().await;
  let mut runtime = ShellRuntime::new(shell, approval_policy);

  let exec_output = match invocation_payload_for_call(req) {
    ToolPayload::LocalShell { params } => {
      let shell_req = ShellRequest {
        command: params.command,
        cwd: params
          .workdir
          .as_deref()
          .map(PathBuf::from)
          .unwrap_or_else(|| ctx.turn.cwd.clone()),
        timeout_ms: params.timeout_ms,
        env: Default::default(),
        justification: params.justification,
        prefix_rule: params.prefix_rule,
        sandbox_permissions: params
          .sandbox_permissions
          .unwrap_or(SandboxPermissions::UseDefault),
        additional_permissions: params.additional_permissions,
      };
      runtime.run(&shell_req, attempt, ctx).await?
    }
    ToolPayload::Function { .. } => {
      #[derive(serde::Deserialize)]
      struct ShellArgs {
        command: String,
        timeout_ms: Option<u64>,
        workdir: Option<String>,
        sandbox_permissions: Option<SandboxPermissions>,
        prefix_rule: Option<Vec<String>>,
        additional_permissions: Option<crate::exec::PermissionProfile>,
        justification: Option<String>,
      }

      let args = serde_json::from_value::<ShellArgs>(req.args.clone())
        .map_err(|err| ToolError::Execution(format!("invalid shell arguments: {err}")))?;
      let shell_req = ShellCommandRequest {
        command: args.command,
        cwd: args
          .workdir
          .as_deref()
          .map(PathBuf::from)
          .unwrap_or_else(|| ctx.turn.cwd.clone()),
        timeout_ms: args.timeout_ms,
        env: Default::default(),
        justification: args.justification,
        prefix_rule: args.prefix_rule,
        sandbox_permissions: args
          .sandbox_permissions
          .unwrap_or(SandboxPermissions::UseDefault),
        additional_permissions: args.additional_permissions,
      };
      runtime.run(&shell_req, attempt, ctx).await?
    }
    ToolPayload::Mcp { .. } | ToolPayload::Custom { .. } => {
      return Err(ToolError::Execution(format!(
        "unsupported shell payload for {}",
        req.tool_name
      )));
    }
  };

  Ok(
    ToolOutput::success(format_exec_output_for_model_structured(
      &exec_output,
      TruncationPolicy::Tokens(DEFAULT_TOOL_OUTPUT_TOKENS),
    ))
    .with_id(req.call_id.clone())
    .with_success(exec_output.exit_code == 0),
  )
}

fn invocation_payload_for_call(call: &ToolCall) -> ToolPayload {
  if let Some((server, tool)) = parse_mcp_tool_name(&call.tool_name) {
    return ToolPayload::Mcp {
      server,
      tool,
      raw_arguments: call.args.to_string(),
    };
  }

  if matches!(
    call.tool_name.as_str(),
    "local_shell" | "unified_exec" | "container.exec"
  ) && let Ok(params) = serde_json::from_value::<ShellToolCallParams>(call.args.clone())
  {
    return ToolPayload::LocalShell { params };
  }

  ToolPayload::Function {
    arguments: call.args.to_string(),
  }
}

fn parse_mcp_tool_name(tool_name: &str) -> Option<(String, String)> {
  let stripped = tool_name.strip_prefix("mcp__")?;
  let (server, tool) = stripped.split_once("__")?;
  if server.is_empty() || tool.is_empty() {
    return None;
  }
  Some((server.to_string(), tool.to_string()))
}

fn map_tool_error(err: ToolError) -> FunctionCallError {
  match err {
    ToolError::Rejected(message) => FunctionCallError::PermissionDenied(message),
    ToolError::SandboxDenied { output, .. } => FunctionCallError::Execution(output),
    ToolError::Execution(message) => FunctionCallError::Execution(message),
  }
}

#[cfg(test)]
mod tests {
  use super::should_emit_exec_events;

  #[test]
  fn collab_tools_skip_exec_transcript_events() {
    assert!(!should_emit_exec_events("spawn_agent"));
    assert!(!should_emit_exec_events("wait"));
    assert!(!should_emit_exec_events("team_status"));
    assert!(should_emit_exec_events("read_file"));
  }
}
