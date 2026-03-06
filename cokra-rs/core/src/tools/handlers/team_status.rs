use async_trait::async_trait;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct TeamStatusHandler;

#[async_trait]
impl ToolHandler for TeamStatusHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let runtime = invocation
      .runtime
      .ok_or_else(|| FunctionCallError::Fatal("team_status missing runtime context".to_string()))?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("team_status runtime is not configured".to_string())
    })?;
    let mut out = ToolOutput::success(serde_json::to_string(&team_runtime.snapshot()).map_err(
      |err| FunctionCallError::Fatal(format!("failed to serialize team status: {err}")),
    )?);
    out.id = invocation.id;
    Ok(out)
  }
}
