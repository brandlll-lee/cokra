use async_trait::async_trait;
use serde::Deserialize;

use cokra_protocol::CollabTaskUpdatedEvent;
use cokra_protocol::EventMsg;
use cokra_protocol::TeamTaskStatus;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct UpdateTeamTaskHandler;

#[derive(Debug, Deserialize)]
struct UpdateTeamTaskArgs {
  task_id: String,
  status: Option<TeamTaskStatus>,
  assignee_thread_id: Option<String>,
  clear_assignee: Option<bool>,
  note: Option<String>,
}

#[async_trait]
impl ToolHandler for UpdateTeamTaskHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: UpdateTeamTaskArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("update_team_task missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("update_team_task runtime is not configured".to_string())
    })?;
    let assignee_thread_id = if args.clear_assignee.unwrap_or(false) {
      Some(None)
    } else {
      args.assignee_thread_id.map(Some)
    };
    let task = team_runtime
      .update_task(&args.task_id, args.status, assignee_thread_id, args.note)
      .await
      .ok_or_else(|| {
        FunctionCallError::RespondToModel(format!("unknown task id: {}", args.task_id))
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
      FunctionCallError::Fatal(format!("failed to serialize updated team task: {err}"))
    })?);
    Ok(out.with_id(invocation.id))
  }
}
