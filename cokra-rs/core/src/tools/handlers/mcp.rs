use serde::Deserialize;

use crate::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct McpHandler;

#[derive(Debug, Deserialize)]
struct McpArgs {
  server: String,
  tool: String,
  arguments: Option<serde_json::Value>,
}

impl ToolHandler for McpHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Mcp
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: McpArgs = invocation.parse_arguments()?;
    let mut out = ToolOutput::success(format!(
      "mcp call staged: server={}, tool={}, args={}",
      args.server,
      args.tool,
      args.arguments.unwrap_or_else(|| serde_json::json!({}))
    ));
    out.id = invocation.id;
    Ok(out)
  }
}
