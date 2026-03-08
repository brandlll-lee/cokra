use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use cokra_protocol::CollabAgentInteractionBeginEvent;
use cokra_protocol::CollabAgentInteractionEndEvent;
use cokra_protocol::EventMsg;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct SendInputHandler;

#[derive(Debug, Deserialize)]
struct SendInputArgs {
  agent_id: String,
  message: String,
}

#[derive(Debug, Serialize)]
struct SendInputResult {
  agent_id: String,
  status: String,
}

#[async_trait]
impl ToolHandler for SendInputHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: SendInputArgs = invocation.parse_arguments()?;
    let runtime = invocation
      .runtime
      .ok_or_else(|| FunctionCallError::Fatal("send_input missing runtime context".to_string()))?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("send_input runtime is not configured".to_string())
    })?;
    let receiver = team_runtime.collab_agent_ref(&args.agent_id);
    let outbound_message = args.message.clone();

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabAgentInteractionBegin(
          CollabAgentInteractionBeginEvent {
            thread_id: runtime.thread_id.clone(),
            agent_id: args.agent_id.clone(),
            nickname: receiver.as_ref().and_then(|agent| agent.nickname.clone()),
            role: receiver.as_ref().and_then(|agent| agent.role.clone()),
            message: outbound_message.clone(),
          },
        ))
        .await;
    }

    team_runtime
      .send_input(&args.agent_id, args.message)
      .await
      .map_err(|err| FunctionCallError::Execution(err.to_string()))?;

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabAgentInteractionEnd(
          CollabAgentInteractionEndEvent {
            thread_id: runtime.thread_id.clone(),
            agent_id: args.agent_id.clone(),
            nickname: receiver.as_ref().and_then(|agent| agent.nickname.clone()),
            role: receiver.and_then(|agent| agent.role),
            message: outbound_message,
            // Tradeoff: `send_input` queues work asynchronously, so we report the best-known
            // status instead of blocking here just to fetch a newer state.
            status: cokra_protocol::AgentStatus::Running,
          },
        ))
        .await;
    }

    let mut out = ToolOutput::success(
      serde_json::to_string(&SendInputResult {
        agent_id: args.agent_id,
        status: "sent".to_string(),
      })
      .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize send_input result: {err}"))
      })?,
    );
    out.id = invocation.id;
    Ok(out)
  }
}
