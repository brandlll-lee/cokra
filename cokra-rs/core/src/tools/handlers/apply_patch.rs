// Apply Patch Handler
use async_trait::async_trait;

use crate::tools::context::{ToolInvocation, ToolOutput, FunctionCallError};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct ApplyPatchHandler;

#[async_trait]
impl ToolHandler for ApplyPatchHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let args: ApplyPatchArgs = invocation.payload.parse_arguments()?;

        // TODO: Implement patch application
        Ok(ToolOutput::success("Patch applied".to_string()))
    }
}

#[derive(serde::Deserialize)]
struct ApplyPatchArgs {
    patch: String,
}
