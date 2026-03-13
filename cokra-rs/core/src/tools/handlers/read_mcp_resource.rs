use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::mcp::McpConnectionManager;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ReadMcpResourceHandler {
  manager: Arc<McpConnectionManager>,
}

#[derive(Debug, Deserialize)]
struct ReadMcpResourceArgs {
  server: String,
  uri: String,
}

#[derive(Debug, Serialize)]
struct ReadMcpResourceResponse {
  server: String,
  uri: String,
  contents: Vec<serde_json::Value>,
}

impl ReadMcpResourceHandler {
  pub fn new(manager: Arc<McpConnectionManager>) -> Self {
    Self { manager }
  }
}

#[async_trait]
impl ToolHandler for ReadMcpResourceHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: ReadMcpResourceArgs = invocation.parse_arguments()?;
    let server = args.server.trim();
    let uri = args.uri.trim();
    if server.is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "server must not be empty".to_string(),
      ));
    }
    if uri.is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "uri must not be empty".to_string(),
      ));
    }
    if !self.manager.server_names().iter().any(|name| name == server) {
      return Err(FunctionCallError::RespondToModel(format!(
        "unknown MCP server `{server}`"
      )));
    }

    let result = self
      .manager
      .read_resource(server, uri)
      .await
      .map_err(|err| FunctionCallError::Execution(err.to_string()))?;
    let content = serde_json::to_string(&ReadMcpResourceResponse {
      server: server.to_string(),
      uri: uri.to_string(),
      contents: result
        .contents
        .into_iter()
        .map(|content| serde_json::to_value(content).unwrap_or_else(|_| serde_json::json!({})))
        .collect(),
    })
    .map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize read_mcp_resource result: {err}"))
    })?;
    Ok(ToolOutput::success(content).with_id(invocation.id))
  }
}
