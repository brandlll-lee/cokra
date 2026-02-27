// Shell handler placeholder
use crate::tools::context::{ToolInvocation, ToolOutput, FunctionCallError};
use crate::tools::registry::ToolHandler;

pub struct ShellHandler;
impl ToolHandler for ShellHandler {
    fn kind(&self) -> crate::tools::registry::ToolKind { crate::tools::registry::ToolKind::Function }
    fn handle(&self, _inv: ToolInvocation) -> Result<ToolOutput, FunctionCallError> { Ok(ToolOutput::success("placeholder")) }
}
