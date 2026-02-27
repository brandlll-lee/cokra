// Cokra Tools Module
// Tool system for extensible command execution

pub mod handlers;
pub mod registry;
pub mod router;
pub mod context;
pub mod spec;
pub mod events;
pub mod parallel;
pub mod sandboxing;
pub mod orchestrator;

pub use registry::{ToolRegistry, ToolRegistryBuilder, ToolHandler, ToolKind};
pub use router::{ToolRouter, ToolCall, ToolCallSource};
pub use context::{ToolInvocation, ToolPayload, ToolOutput};
pub use spec::{ToolSpec, ConfiguredToolSpec};

/// Built-in tool names
pub const BUILTIN_TOOLS: &[&str] = &[
    "shell",
    "apply_patch",
    "read_file",
    "write_file",
    "list_dir",
    "grep_files",
    "search_tool",
    "mcp",
    "mcp_resource",
    "spawn_agent",
    "send_input",
    "wait",
    "close_agent",
    "resume_agent",
    "plan",
    "request_user_input",
    "js_repl",
    "unified_exec",
    "test_sync",
    "view_image",
];

/// Maximum line length for truncation
pub const MAX_LINE_LENGTH: usize = 500;

/// Maximum output bytes for model
pub const MAX_OUTPUT_BYTES: usize = 100 * 1024; // 100 KiB
