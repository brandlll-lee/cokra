use serde::Deserialize;

use crate::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct SpawnAgentHandler;

#[derive(Debug, Deserialize)]
struct SpawnAgentArgs {
  task: String,
  role: Option<String>,
}

impl ToolHandler for SpawnAgentHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: SpawnAgentArgs = invocation.parse_arguments()?;
    let role = args.role.unwrap_or_else(|| "default".to_string());
    let mut out = ToolOutput::success(format!(
      "spawned agent(role={role}) for task: {}",
      args.task
    ));
    out.id = invocation.id;
    Ok(out)
  }
}
