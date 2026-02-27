// Request User Input Handler
use async_trait::async_trait;

use crate::tools::context::{ToolInvocation, ToolOutput, FunctionCallError};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct RequestUserInputHandler;

#[async_trait]
impl ToolHandler for RequestUserInputHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let args: RequestUserInputArgs = invocation.payload.parse_arguments()?;

        // TODO: Implement user input request
        Ok(ToolOutput::success("User input requested".to_string()))
    }
}

#[derive(serde::Deserialize)]
struct RequestUserInputArgs {
    prompt: String,
}
