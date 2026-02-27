// MCP Handler
use async_trait::async_trait;

use crate::tools::context::{ToolInvocation, ToolOutput, FunctionCallError, CallToolResult};
use crate::tools::registry::ToolKind;
use crate::tools::registry::ToolHandler;

pub struct McpHandler;

#[async_trait]
impl ToolHandler for McpHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Mcp
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        // Extract MCP params
        let (server, tool, args) = match &invocation.payload {
            crate::tools::context::ToolPayload::Mcp { server, tool, raw_arguments } => {
                (server.clone(), tool.clone(), raw_arguments.clone())
            }
            _ => return Err(FunctionCallError::InvalidArguments("Expected MCP payload".to_string())),
        };

        // TODO: Implement MCP call
        Ok(ToolOutput::Mcp {
            result: Ok(CallToolResult {
                content: vec![],
                is_error: Some(false),
            }),
        })
    }
}
