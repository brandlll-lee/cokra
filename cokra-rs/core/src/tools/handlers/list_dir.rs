// List Dir Handler
use async_trait::async_trait;

use crate::tools::context::{ToolInvocation, ToolOutput, FunctionCallError};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct ListDirHandler;

#[async_trait]
impl ToolHandler for ListDirHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let args: ListDirArgs = invocation.payload.parse_arguments()?;

        // TODO: Implement directory listing
        Ok(ToolOutput::success(format!("Listed directory: {}", args.dir_path)))
    }
}

#[derive(serde::Deserialize)]
struct ListDirArgs {
    dir_path: String,
    recursive: Option<bool>,
}
