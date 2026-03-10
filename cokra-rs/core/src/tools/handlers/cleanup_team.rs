use async_trait::async_trait;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct CleanupTeamHandler;

#[async_trait]
impl ToolHandler for CleanupTeamHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("cleanup_team missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("cleanup_team runtime is not configured".to_string())
    })?;
    for agent_id in team_runtime.list_spawned_agent_ids() {
      let _ = team_runtime.close_agent(&agent_id).await;
    }
    team_runtime.clear_state().await;

    Ok(ToolOutput::success("{\"status\":\"cleaned\"}".to_string()).with_id(invocation.id))
  }
}
