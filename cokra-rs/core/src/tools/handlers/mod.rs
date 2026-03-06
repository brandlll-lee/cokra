pub mod apply_patch;
pub mod close_agent;
pub mod create_team_task;
pub mod dynamic;
pub mod grep_files;
pub mod list_dir;
pub mod mcp;
pub mod plan;
pub mod read_file;
pub mod read_team_messages;
pub mod request_user_input;
pub mod send_input;
pub mod send_team_message;
pub mod shell;
pub mod spawn_agent;
pub mod team_status;
pub mod update_team_task;
pub mod view_image;
pub mod wait;
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
  registry.register_handler("send_input", Arc::new(send_input::SendInputHandler));
  registry.register_handler("wait", Arc::new(wait::WaitHandler));
  registry.register_handler("close_agent", Arc::new(close_agent::CloseAgentHandler));
  registry.register_handler("team_status", Arc::new(team_status::TeamStatusHandler));
  registry.register_handler(
    "send_team_message",
    Arc::new(send_team_message::SendTeamMessageHandler),
  );
  registry.register_handler(
    "read_team_messages",
    Arc::new(read_team_messages::ReadTeamMessagesHandler),
  );
  registry.register_handler(
    "create_team_task",
    Arc::new(create_team_task::CreateTeamTaskHandler),
  );
  registry.register_handler(
    "update_team_task",
    Arc::new(update_team_task::UpdateTeamTaskHandler),
  );
  registry.register_handler("plan", Arc::new(plan::PlanHandler));
  registry.register_handler(
    "request_user_input",
    Arc::new(request_user_input::RequestUserInputHandler),
  );
  registry.register_handler("view_image", Arc::new(view_image::ViewImageHandler));
}
