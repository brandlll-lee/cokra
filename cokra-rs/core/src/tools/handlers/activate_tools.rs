use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ActivateToolsHandler;

#[derive(Debug, Deserialize)]
struct ActivateToolsArgs {
  names: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ActivateToolsResponse {
  requested: Vec<String>,
  activated: Vec<String>,
  active_external_total: usize,
  inactive_external_total: usize,
}

#[async_trait]
impl ToolHandler for ActivateToolsHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: ActivateToolsArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("activate_tools missing runtime context".to_string())
    })?;
    let activated = runtime.tool_registry.activate_tools(&args.names);
    let response = ActivateToolsResponse {
      requested: args.names,
      activated,
      active_external_total: runtime.tool_registry.active_external_tool_names().len(),
      inactive_external_total: runtime.tool_registry.inactive_external_tool_names().len(),
    };
    let content = serde_json::to_string(&response).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize activate_tools: {err}"))
    })?;
    Ok(ToolOutput::success(content).with_id(invocation.id))
  }
}
