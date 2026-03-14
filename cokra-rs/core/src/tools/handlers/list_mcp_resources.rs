use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::mcp::McpConnectionManager;
use crate::mcp::McpResourceDescriptor;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ListMcpResourcesHandler {
  manager: Arc<McpConnectionManager>,
}

#[derive(Debug, Deserialize)]
struct ListMcpResourcesArgs {
  #[serde(default)]
  server: Option<String>,
}

#[derive(Debug, Serialize)]
struct ListMcpResourcesResponse {
  total_resources: usize,
  resources: Vec<McpResourceDescriptor>,
}

impl ListMcpResourcesHandler {
  pub fn new(manager: Arc<McpConnectionManager>) -> Self {
    Self { manager }
  }
}

#[async_trait]
impl ToolHandler for ListMcpResourcesHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: ListMcpResourcesArgs = invocation.parse_arguments()?;
    let resources = filtered_resources(&self.manager, args.server.as_deref())?;
    let content = serde_json::to_string(&ListMcpResourcesResponse {
      total_resources: resources.len(),
      resources,
    })
    .map_err(|err| {
      FunctionCallError::Fatal(format!(
        "failed to serialize list_mcp_resources result: {err}"
      ))
    })?;
    Ok(ToolOutput::success(content).with_id(invocation.id))
  }
}

fn filtered_resources(
  manager: &McpConnectionManager,
  server: Option<&str>,
) -> Result<Vec<McpResourceDescriptor>, FunctionCallError> {
  let server = server.map(str::trim).filter(|value| !value.is_empty());
  if let Some(server_name) = server
    && !manager
      .server_names()
      .iter()
      .any(|name| name == server_name)
  {
    return Err(FunctionCallError::RespondToModel(format!(
      "unknown MCP server `{server_name}`"
    )));
  }

  let mut resources = manager.resource_descriptors();
  if let Some(server_name) = server {
    resources.retain(|resource| resource.server_name == server_name);
  }
  Ok(resources)
}
