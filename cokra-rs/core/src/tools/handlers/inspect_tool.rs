use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::catalog::ToolCatalog;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct InspectToolHandler {
  catalog: Arc<ToolCatalog>,
}

#[derive(Debug, Deserialize)]
struct InspectToolArgs {
  name: String,
}

impl InspectToolHandler {
  pub fn new(catalog: Arc<ToolCatalog>) -> Self {
    Self { catalog }
  }
}

#[async_trait]
impl ToolHandler for InspectToolHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    false
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: InspectToolArgs = invocation.parse_arguments()?;
    let name = args.name.trim();
    if name.is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "name must not be empty".to_string(),
      ));
    }

    let entry = self
      .catalog
      .inspect(name)
      .ok_or_else(|| FunctionCallError::RespondToModel(format!("unknown tool: {name}")))?;
    let content = serde_json::to_string(&entry).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize inspect_tool result: {err}"))
    })?;
    Ok(ToolOutput::success(content).with_id(invocation.id))
  }
}
