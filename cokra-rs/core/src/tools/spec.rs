// Tool Specifications
// Definitions for all built-in tools

use serde_json::json;

use crate::tools::registry::ToolSpec;

/// Build all tool specs
pub fn build_specs() -> Vec<ToolSpec> {
    vec![
        shell_tool(),
        apply_patch_tool(),
        read_file_tool(),
        write_file_tool(),
        list_dir_tool(),
        grep_files_tool(),
        search_tool(),
        mcp_tool(),
        spawn_agent_tool(),
        plan_tool(),
        request_user_input_tool(),
        view_image_tool(),
    ]
}

fn shell_tool() -> ToolSpec {
    ToolSpec::new(
        "shell",
        "Execute a shell command",
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds"
                }
            },
            "required": ["command"]
        }),
    )
}

fn apply_patch_tool() -> ToolSpec {
    ToolSpec::new(
        "apply_patch",
        "Apply a patch to files",
        json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "The patch content in unified diff format"
                }
            },
            "required": ["patch"]
        }),
    )
}

fn read_file_tool() -> ToolSpec {
    ToolSpec::new(
        "read_file",
        "Read file contents",
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line offset to start reading"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines"
                }
            },
            "required": ["file_path"]
        }),
    )
}

fn write_file_tool() -> ToolSpec {
    ToolSpec::new(
        "write_file",
        "Write content to a file",
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write"
                }
            },
            "required": ["file_path", "content"]
        }),
    )
}

fn list_dir_tool() -> ToolSpec {
    ToolSpec::new(
        "list_dir",
        "List directory contents",
        json!({
            "type": "object",
            "properties": {
                "dir_path": {
                    "type": "string",
                    "description": "Path to the directory"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "List recursively"
                }
            },
            "required": ["dir_path"]
        }),
    )
}

fn grep_files_tool() -> ToolSpec {
    ToolSpec::new(
        "grep_files",
        "Search files using ripgrep",
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Search pattern"
                },
                "path": {
                    "type": "string",
                    "description": "Path to search"
                }
            },
            "required": ["pattern"]
        }),
    )
}

fn search_tool() -> ToolSpec {
    ToolSpec::new(
        "search_tool",
        "Search tools using BM25",
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                }
            },
            "required": ["query"]
        }),
    )
}

fn mcp_tool() -> ToolSpec {
    ToolSpec::new(
        "mcp",
        "Call an MCP tool",
        json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "MCP server name"
                },
                "tool": {
                    "type": "string",
                    "description": "Tool name"
                },
                "arguments": {
                    "type": "object",
                    "description": "Tool arguments"
                }
            },
            "required": ["server", "tool"]
        }),
    )
}

fn spawn_agent_tool() -> ToolSpec {
    ToolSpec::new(
        "spawn_agent",
        "Spawn a sub-agent",
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Task description"
                },
                "role": {
                    "type": "string",
                    "description": "Agent role"
                }
            },
            "required": ["task"]
        }),
    )
}

fn plan_tool() -> ToolSpec {
    ToolSpec::new(
        "plan",
        "Show execution plan",
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Plan text"
                }
            },
            "required": ["text"]
        }),
    )
}

fn request_user_input_tool() -> ToolSpec {
    ToolSpec::new(
        "request_user_input",
        "Request input from user",
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Prompt for the user"
                }
            },
            "required": ["prompt"]
        }),
    )
}

fn view_image_tool() -> ToolSpec {
    ToolSpec::new(
        "view_image",
        "View an image file",
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the image"
                }
            },
            "required": ["path"]
        }),
    )
}
