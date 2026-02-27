// Tool Handlers Module
pub mod shell;
pub mod apply_patch;
pub mod read_file;
pub mod write_file;
pub mod list_dir;
pub mod grep_files;
pub mod mcp;
pub mod spawn_agent;
pub mod plan;
pub mod request_user_input;
pub mod view_image;
pub mod dynamic;

pub use shell::ShellHandler;
pub use apply_patch::ApplyPatchHandler;
pub use read_file::ReadFileHandler;
pub use write_file::WriteFileHandler;
pub use list_dir::ListDirHandler;
pub use grep_files::GrepFilesHandler;
pub use mcp::McpHandler;
pub use spawn_agent::SpawnAgentHandler;
pub use plan::PlanHandler;
pub use request_user_input::RequestUserInputHandler;
pub use view_image::ViewImageHandler;
pub use dynamic::DynamicToolHandler;

use crate::tools::context::FunctionCallError;
use serde::de::DeserializeOwned;

/// Parse arguments from JSON
fn parse_arguments<T: DeserializeOwned>(arguments: &str) -> Result<T, FunctionCallError> {
    serde_json::from_str(arguments)
        .map_err(|e| FunctionCallError::ParseError(e.to_string()))
}
