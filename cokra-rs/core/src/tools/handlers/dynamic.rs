use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct DynamicToolHandler;

#[derive(Debug, Deserialize)]
struct SearchArgs {
  query: String,
}

#[async_trait]
impl ToolHandler for DynamicToolHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: SearchArgs = invocation.parse_arguments()?;
    let mut out = ToolOutput::success(format!("search query accepted: {}", args.query));
    out.id = invocation.id;
    Ok(out)
  }
}
