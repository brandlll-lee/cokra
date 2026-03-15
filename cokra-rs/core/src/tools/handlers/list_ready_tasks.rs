use async_trait::async_trait;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ListReadyTasksHandler;

#[async_trait]
impl ToolHandler for ListReadyTasksHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("list_ready_tasks missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("list_ready_tasks runtime is not configured".to_string())
    })?;
    let tasks = team_runtime.list_ready_tasks(&runtime.thread_id).await;

    let out = ToolOutput::success(serde_json::to_string(&tasks).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize ready tasks: {err}"))
    })?);
    Ok(out.with_id(invocation.id))
  }
}
