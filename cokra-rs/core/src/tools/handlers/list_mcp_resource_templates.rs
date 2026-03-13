use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::mcp::McpConnectionManager;
use crate::mcp::McpResourceTemplateDescriptor;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ListMcpResourceTemplatesHandler {
  manager: Arc<McpConnectionManager>,
}

#[derive(Debug, Deserialize)]
struct ListMcpResourceTemplatesArgs {
  #[serde(default)]
  server: Option<String>,
}

#[derive(Debug, Serialize)]
struct ListMcpResourceTemplatesResponse {
  total_templates: usize,
  templates: Vec<McpResourceTemplateDescriptor>,
}

impl ListMcpResourceTemplatesHandler {
  pub fn new(manager: Arc<McpConnectionManager>) -> Self {
    Self { manager }
  }
}

#[async_trait]
impl ToolHandler for ListMcpResourceTemplatesHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: ListMcpResourceTemplatesArgs = invocation.parse_arguments()?;
    let templates = filtered_templates(&self.manager, args.server.as_deref())?;
    let content = serde_json::to_string(&ListMcpResourceTemplatesResponse {
      total_templates: templates.len(),
      templates,
    })
    .map_err(|err| {
      FunctionCallError::Fatal(format!(
        "failed to serialize list_mcp_resource_templates result: {err}"
      ))
    })?;
    Ok(ToolOutput::success(content).with_id(invocation.id))
  }
}

fn filtered_templates(
  manager: &McpConnectionManager,
  server: Option<&str>,
) -> Result<Vec<McpResourceTemplateDescriptor>, FunctionCallError> {
  let server = server.map(str::trim).filter(|value| !value.is_empty());
  if let Some(server_name) = server
    && !manager.server_names().iter().any(|name| name == server_name)
  {
    return Err(FunctionCallError::RespondToModel(format!(
      "unknown MCP server `{server_name}`"
    )));
  }

  let mut templates = manager.resource_template_descriptors();
  if let Some(server_name) = server {
    templates.retain(|template| template.server_name == server_name);
  }
  Ok(templates)
}

