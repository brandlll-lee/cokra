use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use cokra_protocol::CollabCloseBeginEvent;
use cokra_protocol::CollabCloseEndEvent;
use cokra_protocol::EventMsg;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct CloseAgentHandler;

#[derive(Debug, Deserialize)]
struct CloseAgentArgs {
  #[serde(alias = "agent")]
  agent_id: String,
}

#[derive(Debug, Serialize)]
struct CloseAgentResult {
  agent_id: String,
  status: cokra_protocol::AgentStatus,
}

#[async_trait]
impl ToolHandler for CloseAgentHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: CloseAgentArgs = invocation.parse_arguments()?;
    let runtime = invocation
      .runtime
      .ok_or_else(|| FunctionCallError::Fatal("close_agent missing runtime context".to_string()))?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("close_agent runtime is not configured".to_string())
    })?;
    let agent_id = team_runtime
      .resolve_agent_selector(&args.agent_id)
      .ok_or_else(|| FunctionCallError::Execution(format!("agent not found: {}", args.agent_id)))?;
    let receiver = team_runtime.collab_agent_ref(&agent_id);

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabCloseBegin(CollabCloseBeginEvent {
          call_id: invocation.id.clone(),
          sender_thread_id: runtime.thread_id.clone(),
          receiver_thread_id: agent_id.clone(),
        }))
        .await;
    }

    let status = team_runtime
      .close_agent(&agent_id)
      .await
      .map_err(|err| FunctionCallError::Execution(err.to_string()))?;

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabCloseEnd(CollabCloseEndEvent {
          call_id: invocation.id.clone(),
          sender_thread_id: runtime.thread_id.clone(),
          receiver_thread_id: agent_id.clone(),
          receiver_nickname: receiver.as_ref().and_then(|agent| agent.nickname.clone()),
          receiver_role: receiver.and_then(|agent| agent.role),
          status: status.clone(),
        }))
        .await;
    }

    let out = ToolOutput::success(
      serde_json::to_string(&CloseAgentResult { agent_id, status }).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize close_agent result: {err}"))
      })?,
    );
    Ok(out.with_id(invocation.id))
  }
}
