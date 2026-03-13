pub mod apply_patch;
pub mod approve_team_plan;
pub mod assign_team_task;
pub mod claim_next_team_task;
pub mod claim_team_messages;
pub mod claim_team_task;
pub mod cleanup_team;
pub mod close_agent;
pub mod code_search;
pub mod create_team_task;
pub mod diagnostics;
pub mod dynamic;
pub mod edit_file;
pub mod glob;
pub mod grep_files;
pub mod handoff_team_task;
pub mod inspect_tool;
pub mod list_dir;
pub mod list_mcp_resource_templates;
pub mod list_mcp_resources;
pub mod mcp;
pub mod plan;
pub mod read_file;
pub mod read_mcp_resource;
pub mod read_many_files;
pub mod read_team_messages;
pub mod request_user_input;
pub mod save_memory;
pub mod send_input;
pub mod send_team_message;
pub mod shell;
pub mod skill;
pub mod spawn_agent;
pub mod submit_team_plan;
pub mod team_status;
pub mod todo;
pub mod update_team_task;
pub mod view_image;
pub mod wait;
pub mod web_fetch;
pub mod web_search;
pub mod write_file;

use std::sync::Arc;

use crate::mcp::McpConnectionManager;
use crate::tools::registry::ToolRegistry;

pub fn register_builtin_handlers(
  registry: &mut ToolRegistry,
  mcp_manager: Arc<McpConnectionManager>,
) {
  registry.register_handler("apply_patch", Arc::new(apply_patch::ApplyPatchHandler));
  registry.register_handler("edit_file", Arc::new(edit_file::EditFileHandler));
  registry.register_handler("glob", Arc::new(glob::GlobHandler));
  registry.register_handler("web_fetch", Arc::new(web_fetch::WebFetchHandler));
  registry.register_handler("read_file", Arc::new(read_file::ReadFileHandler));
  registry.register_handler("write_file", Arc::new(write_file::WriteFileHandler));
  registry.register_handler("list_dir", Arc::new(list_dir::ListDirHandler));
  registry.register_handler(
    "list_mcp_resources",
    Arc::new(list_mcp_resources::ListMcpResourcesHandler::new(Arc::clone(
      &mcp_manager,
    ))),
  );
  registry.register_handler(
    "list_mcp_resource_templates",
    Arc::new(
      list_mcp_resource_templates::ListMcpResourceTemplatesHandler::new(Arc::clone(&mcp_manager)),
    ),
  );
  registry.register_handler(
    "read_mcp_resource",
    Arc::new(read_mcp_resource::ReadMcpResourceHandler::new(Arc::clone(
      &mcp_manager,
    ))),
  );
  registry.register_handler("grep_files", Arc::new(grep_files::GrepFilesHandler));
  registry.register_handler("code_search", Arc::new(code_search::CodeSearchHandler));
  for tool_name in mcp_manager.tool_names() {
    registry.register_handler(
      tool_name,
      Arc::new(mcp::McpHandler::new(Arc::clone(&mcp_manager))),
    );
  }
  registry.register_handler("spawn_agent", Arc::new(spawn_agent::SpawnAgentHandler));
  registry.register_handler("send_input", Arc::new(send_input::SendInputHandler));
  registry.register_handler("wait", Arc::new(wait::WaitHandler));
  registry.register_handler("close_agent", Arc::new(close_agent::CloseAgentHandler));
  registry.register_handler(
    "claim_team_task",
    Arc::new(claim_team_task::ClaimTeamTaskHandler),
  );
  registry.register_handler(
    "claim_team_messages",
    Arc::new(claim_team_messages::ClaimTeamMessagesHandler),
  );
  registry.register_handler(
    "claim_next_team_task",
    Arc::new(claim_next_team_task::ClaimNextTeamTaskHandler),
  );
  registry.register_handler(
    "assign_team_task",
    Arc::new(assign_team_task::AssignTeamTaskHandler),
  );
  registry.register_handler(
    "handoff_team_task",
    Arc::new(handoff_team_task::HandoffTeamTaskHandler),
  );
  registry.register_handler("cleanup_team", Arc::new(cleanup_team::CleanupTeamHandler));
  registry.register_handler(
    "submit_team_plan",
    Arc::new(submit_team_plan::SubmitTeamPlanHandler),
  );
  registry.register_handler(
    "approve_team_plan",
    Arc::new(approve_team_plan::ApproveTeamPlanHandler),
  );
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
  registry.register_handler("web_search", Arc::new(web_search::WebSearchHandler));
  registry.register_handler("save_memory", Arc::new(save_memory::SaveMemoryHandler));
  registry.register_handler("diagnostics", Arc::new(diagnostics::DiagnosticsHandler));
  registry.register_handler("skill", Arc::new(skill::SkillHandler));
  registry.register_handler(
    "read_many_files",
    Arc::new(read_many_files::ReadManyFilesHandler),
  );
  // todo_read 已废弃（1:1 opencode registry.ts:110 `// TodoReadTool`）
  // todo_write 返回完整 JSON 列表，模型不需要单独读取工具
  registry.register_handler("todo_write", Arc::new(todo::TodoWriteHandler));
}
