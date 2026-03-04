use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct PlanHandler;

#[derive(Debug, Deserialize)]
struct PlanArgs {
  text: String,
}

#[async_trait]
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
