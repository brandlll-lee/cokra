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

pub struct AddTaskDependencyHandler;

#[derive(Debug, Deserialize)]
struct AddTaskDependencyArgs {
  task_id: String,
  dependency_task_id: String,
  reason: Option<String>,
}

#[async_trait]
impl ToolHandler for AddTaskDependencyHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: AddTaskDependencyArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("add_task_dependency missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("add_task_dependency runtime is not configured".to_string())
    })?;
    let task = team_runtime
      .add_task_dependency(&args.task_id, &args.dependency_task_id, args.reason)
      .await
      .ok_or_else(|| {
        FunctionCallError::RespondToModel(format!(
          "failed to add dependency {} -> {}",
          args.dependency_task_id, args.task_id
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
      FunctionCallError::Fatal(format!("failed to serialize dependent task: {err}"))
    })?);
    Ok(out.with_id(invocation.id))
  }
}
