use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct DeactivateToolsHandler;

#[derive(Debug, Deserialize)]
struct DeactivateToolsArgs {
  names: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DeactivateToolsResponse {
  requested: Vec<String>,
  deactivated: Vec<String>,
  active_external_total: usize,
  inactive_external_total: usize,
}

#[async_trait]
impl ToolHandler for DeactivateToolsHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: DeactivateToolsArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("deactivate_tools missing runtime context".to_string())
    })?;
    let deactivated = runtime.tool_registry.deactivate_tools(&args.names);
    let response = DeactivateToolsResponse {
      requested: args.names,
      deactivated,
      active_external_total: runtime.tool_registry.active_external_tool_names().len(),
      inactive_external_total: runtime.tool_registry.inactive_external_tool_names().len(),
    };
    let content = serde_json::to_string(&response).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize deactivate_tools: {err}"))
    })?;
    Ok(ToolOutput::success(content).with_id(invocation.id))
  }
}
