// Plan Handler
use async_trait::async_trait;

use crate::tools::context::{ToolInvocation, ToolOutput, FunctionCallError};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct PlanHandler;

#[async_trait]
impl ToolHandler for PlanHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let args: PlanArgs = invocation.payload.parse_arguments()?;

        // Plan is just for documentation, return success
        Ok(ToolOutput::success(args.text))
    }
}

#[derive(serde::Deserialize)]
struct PlanArgs {
    text: String,
}
