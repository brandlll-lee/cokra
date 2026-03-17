use async_trait::async_trait;
use serde::Deserialize;

use cokra_protocol::CollabTaskUpdatedEvent;
use cokra_protocol::EventMsg;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::handlers::team_selectors::resolve_required_agent_selector;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct AssignTeamTaskHandler;

#[derive(Debug, Deserialize)]
struct AssignTeamTaskArgs {
  task_id: String,
  assignee_thread_id: String,
  note: Option<String>,
  override_assignee: Option<bool>,
}

#[async_trait]
impl ToolHandler for AssignTeamTaskHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: AssignTeamTaskArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("assign_team_task missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("assign_team_task runtime is not configured".to_string())
    })?;
    let assignee_thread_id = resolve_required_agent_selector(
      &team_runtime,
      &args.assignee_thread_id,
      "assignee_thread_id",
    )?;
    let task = team_runtime
      .assign_task(
        &args.task_id,
        assignee_thread_id,
        args.note,
        args.override_assignee.unwrap_or(false),
      )
      .await
      .ok_or_else(|| {
        FunctionCallError::RespondToModel(format!(
          "task {} is unknown or already assigned to a different teammate",
          args.task_id
        ))
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
      FunctionCallError::Fatal(format!("failed to serialize assigned task: {err}"))
    })?);
    Ok(out.with_id(invocation.id))
  }
}
