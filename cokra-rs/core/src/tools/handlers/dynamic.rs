use serde::Deserialize;

use crate::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct DynamicToolHandler;

#[derive(Debug, Deserialize)]
struct SearchArgs {
  query: String,
}

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
