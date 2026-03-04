use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ApplyPatchHandler;

#[derive(Debug, Deserialize)]
struct ApplyPatchArgs {
  patch: String,
}

#[async_trait]
impl ToolHandler for ApplyPatchHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _: &ToolInvocation) -> bool {
    true
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: ApplyPatchArgs = invocation.parse_arguments()?;
    let mut out = ToolOutput::success(format!("apply_patch accepted ({} bytes)", args.patch.len()));
    out.id = invocation.id;
    Ok(out)
  }
}
