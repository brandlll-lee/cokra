use async_trait::async_trait;
use serde::Serialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ResetActiveToolsHandler;

#[derive(Debug, Serialize)]
struct ResetActiveToolsResponse {
  active_external_total: usize,
  inactive_external_total: usize,
}

#[async_trait]
impl ToolHandler for ResetActiveToolsHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("reset_active_tools missing runtime context".to_string())
    })?;
    runtime.tool_registry.reset_active_external_tools();
    let response = ResetActiveToolsResponse {
      active_external_total: runtime.tool_registry.active_external_tool_names().len(),
      inactive_external_total: runtime.tool_registry.inactive_external_tool_names().len(),
    };
    let content = serde_json::to_string(&response).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize reset_active_tools: {err}"))
    })?;
    Ok(ToolOutput::success(content).with_id(invocation.id))
  }
}
