// Grep Files Handler
use async_trait::async_trait;

use crate::tools::context::{ToolInvocation, ToolOutput, FunctionCallError};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct GrepFilesHandler;

#[async_trait]
impl ToolHandler for GrepFilesHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let args: GrepFilesArgs = invocation.payload.parse_arguments()?;

        // TODO: Implement grep
        Ok(ToolOutput::success(format!("Searched for: {}", args.pattern)))
    }
}

#[derive(serde::Deserialize)]
struct GrepFilesArgs {
    pattern: String,
    path: Option<String>,
}
