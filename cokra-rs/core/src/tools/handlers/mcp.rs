use std::sync::Arc;
use async_trait::async_trait;

use crate::mcp::McpConnectionManager;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct McpHandler {
  manager: Arc<McpConnectionManager>,
}

impl McpHandler {
  pub fn new(manager: Arc<McpConnectionManager>) -> Self {
    Self { manager }
  }
}

#[async_trait]
impl ToolHandler for McpHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Mcp
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let Some((_server, _tool)) = self.manager.resolve_tool_name(&invocation.name) else {
      return Err(FunctionCallError::ToolNotFound(format!(
        "unknown MCP tool `{}`",
        invocation.name
      )));
    };

    let arguments = invocation.parse_arguments_value().ok();
    let result = self.manager.call_tool(&invocation.name, arguments).await;

    Ok(ToolOutput::Mcp {
      id: invocation.id,
      result: result.map_err(|err| err.to_string()),
    })
  }
}
