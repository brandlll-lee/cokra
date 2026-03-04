use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct RequestUserInputHandler;

#[derive(Debug, Deserialize)]
struct RequestUserInputArgs {
  prompt: String,
}

#[async_trait]
impl ToolHandler for RequestUserInputHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: RequestUserInputArgs = invocation.parse_arguments()?;
    let mut out = ToolOutput::success(format!("user input required: {}", args.prompt));
    out.id = invocation.id;
    Ok(out)
  }
}
