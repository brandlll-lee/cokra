use async_trait::async_trait;

use cokra_protocol::CollabTaskUpdatedEvent;
use cokra_protocol::EventMsg;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ClaimNextTeamTaskHandler;

#[async_trait]
impl ToolHandler for ClaimNextTeamTaskHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("claim_next_team_task missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("claim_next_team_task runtime is not configured".to_string())
    })?;
    let task = team_runtime
      .claim_next_task(&runtime.thread_id)
      .await
      .ok_or_else(|| {
        FunctionCallError::RespondToModel("no claimable team task found".to_string())
      })?;

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabTaskUpdated(CollabTaskUpdatedEvent {
          actor_thread_id: runtime.thread_id.clone(),
          task: task.clone(),
        }))
        .await;
    }

    let out = ToolOutput::success(serde_json::to_string(&task).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize claimed task: {err}"))
    })?);
    Ok(out.with_id(invocation.id))
  }
}
