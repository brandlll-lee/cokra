// Shell Handler
use async_trait::async_trait;
use std::sync::Arc;

use crate::tools::context::{ToolInvocation, ToolOutput, ToolPayload, FunctionCallError, ShellToolCallParams};
use crate::tools::registry::{ToolHandler, ToolKind};

/// Shell command handler
pub struct ShellHandler;

#[async_trait]
impl ToolHandler for ShellHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        // Shell commands may be mutating
        true
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let args: ShellToolCallParams = invocation.payload.parse_arguments()?;

        // TODO: Implement actual shell execution
        Ok(ToolOutput::success(format!("Executed: {:?}", args.command)))
    }
}
