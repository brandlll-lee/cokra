pub mod apply_patch;
pub mod dynamic;
pub mod grep_files;
pub mod list_dir;
pub mod mcp;
pub mod plan;
pub mod read_file;
pub mod request_user_input;
pub mod shell;
pub mod spawn_agent;
pub mod view_image;
pub mod write_file;

use std::sync::Arc;

use crate::tools::registry::ToolRegistry;

pub fn register_builtin_handlers(registry: &mut ToolRegistry) {
  registry.register_handler("shell", Arc::new(shell::ShellHandler));
  registry.register_handler("apply_patch", Arc::new(apply_patch::ApplyPatchHandler));
  registry.register_handler("read_file", Arc::new(read_file::ReadFileHandler));
  registry.register_handler("write_file", Arc::new(write_file::WriteFileHandler));
  registry.register_handler("list_dir", Arc::new(list_dir::ListDirHandler));
  registry.register_handler("grep_files", Arc::new(grep_files::GrepFilesHandler));
  registry.register_handler("search_tool", Arc::new(dynamic::DynamicToolHandler));
  registry.register_handler("mcp", Arc::new(mcp::McpHandler));
  registry.register_handler("spawn_agent", Arc::new(spawn_agent::SpawnAgentHandler));
  registry.register_handler("plan", Arc::new(plan::PlanHandler));
  registry.register_handler(
    "request_user_input",
    Arc::new(request_user_input::RequestUserInputHandler),
  );
  registry.register_handler("view_image", Arc::new(view_image::ViewImageHandler));
}
