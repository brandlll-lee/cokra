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

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabAgentInteractionBegin(
          CollabAgentInteractionBeginEvent {
            thread_id: runtime.thread_id.clone(),
            agent_id: args.agent_id.clone(),
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
            result: "sent".to_string(),
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
