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

pub struct HandoffTeamTaskHandler;

#[derive(Debug, Deserialize)]
struct HandoffTeamTaskArgs {
  task_id: String,
  to_thread_id: String,
  note: Option<String>,
  review_mode: Option<bool>,
}

#[async_trait]
impl ToolHandler for HandoffTeamTaskHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: HandoffTeamTaskArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("handoff_team_task missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("handoff_team_task runtime is not configured".to_string())
    })?;
    let task = team_runtime
      .handoff_task(
        &args.task_id,
        args.to_thread_id,
        args.note,
        args.review_mode.unwrap_or(true),
      )
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

    let mut out = ToolOutput::success(serde_json::to_string(&task).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize handed off task: {err}"))
    })?);
    Ok(out.with_id(invocation.id))
  }
}
