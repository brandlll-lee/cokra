pub struct ToolInvocation;
pub struct ToolOutput;
pub enum FunctionCallError { Other(String) }
impl ToolOutput { pub fn success(_: &str) -> Self { ToolOutput } }
pub struct ShellToolCallParams;
