use std::path::Path;

use serde::Deserialize;

use crate::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct ViewImageHandler;

#[derive(Debug, Deserialize)]
struct ViewImageArgs {
  path: String,
}

impl ToolHandler for ViewImageHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: ViewImageArgs = invocation.parse_arguments()?;
    let path = Path::new(&args.path);

    if !path.exists() {
      return Err(FunctionCallError::Execution(format!(
        "image not found: {}",
        path.display()
      )));
    }

    let mut out = ToolOutput::success(format!("image ready: {}", path.display()));
    out.id = invocation.id;
    Ok(out)
  }
}
