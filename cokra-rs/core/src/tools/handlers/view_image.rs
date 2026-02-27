// View Image Handler
use async_trait::async_trait;

use crate::tools::context::{ToolInvocation, ToolOutput, FunctionCallError};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct ViewImageHandler;

#[async_trait]
impl ToolHandler for ViewImageHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let args: ViewImageArgs = invocation.payload.parse_arguments()?;

        // TODO: Implement image viewing
        Ok(ToolOutput::success(format!("Viewed image: {}", args.path)))
    }
}

#[derive(serde::Deserialize)]
struct ViewImageArgs {
    path: String,
}
