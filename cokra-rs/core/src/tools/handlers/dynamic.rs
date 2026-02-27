// Dynamic Tool Handler
use async_trait::async_trait;

use crate::tools::context::{ToolInvocation, ToolOutput, FunctionCallError};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct DynamicToolHandler;

#[async_trait]
impl ToolHandler for DynamicToolHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        // Dynamic tools are handled by the orchestrator
        Ok(ToolOutput::success("Dynamic tool executed".to_string()))
    }
}
