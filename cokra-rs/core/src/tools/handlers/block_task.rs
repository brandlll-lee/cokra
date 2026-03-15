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

pub struct BlockTaskHandler;

#[derive(Debug, Deserialize)]
struct BlockTaskArgs {
  task_id: String,
  reason: String,
}

#[async_trait]
impl ToolHandler for BlockTaskHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: BlockTaskArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("block_task missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("block_task runtime is not configured".to_string())
    })?;
    let task = team_runtime
      .block_task(&args.task_id, args.reason)
      .await
      .ok_or_else(|| {
        FunctionCallError::RespondToModel(format!("failed to block task {}", args.task_id))
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
      FunctionCallError::Fatal(format!("failed to serialize blocked task: {err}"))
    })?);
    Ok(out.with_id(invocation.id))
  }
}
