use async_trait::async_trait;
use serde::Deserialize;

use cokra_protocol::CollabTaskUpdatedEvent;
use cokra_protocol::EventMsg;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ClaimTeamTaskHandler;

#[derive(Debug, Deserialize)]
struct ClaimTeamTaskArgs {
  task_id: String,
  note: Option<String>,
}

#[async_trait]
impl ToolHandler for ClaimTeamTaskHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: ClaimTeamTaskArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("claim_team_task missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("claim_team_task runtime is not configured".to_string())
    })?;
    let Some(task) = team_runtime
      .claim_task(&args.task_id, runtime.thread_id.clone(), args.note)
      .await
      .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?
    else {
      team_runtime.note_attention(
        &runtime.thread_id,
        format!("task {} is not claimable by this teammate", args.task_id),
      );
      return Err(FunctionCallError::RespondToModel(format!(
        "task {} is unknown, not ready, or assigned to another teammate",
        args.task_id
      )));
    };
    team_runtime.clear_attention(&runtime.thread_id);

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
