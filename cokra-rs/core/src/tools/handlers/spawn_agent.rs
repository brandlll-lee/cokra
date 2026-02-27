// Spawn Agent Handler
use async_trait::async_trait;

use crate::tools::context::{ToolInvocation, ToolOutput, FunctionCallError};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct SpawnAgentHandler;

#[async_trait]
impl ToolHandler for SpawnAgentHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let args: SpawnAgentArgs = invocation.payload.parse_arguments()?;

        // TODO: Implement agent spawning
        Ok(ToolOutput::success(format!("Spawned agent for: {}", args.task)))
    }
}

#[derive(serde::Deserialize)]
struct SpawnAgentArgs {
    task: String,
    role: Option<String>,
}
