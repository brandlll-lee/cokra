use serde::Deserialize;

use crate::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct PlanHandler;

#[derive(Debug, Deserialize)]
struct PlanArgs {
  text: String,
}

impl ToolHandler for PlanHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: PlanArgs = invocation.parse_arguments()?;
    let mut out = ToolOutput::success(args.text);
    out.id = invocation.id;
    Ok(out)
  }
}
