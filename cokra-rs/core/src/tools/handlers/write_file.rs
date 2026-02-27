// Write File Handler
use async_trait::async_trait;

use crate::tools::context::{ToolInvocation, ToolOutput, FunctionCallError};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct WriteFileHandler;

#[async_trait]
impl ToolHandler for WriteFileHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let args: WriteFileArgs = invocation.payload.parse_arguments()?;

        // TODO: Implement file writing
        Ok(ToolOutput::success(format!("Wrote file: {}", args.file_path)))
    }
}

#[derive(serde::Deserialize)]
struct WriteFileArgs {
    file_path: String,
    content: String,
}
