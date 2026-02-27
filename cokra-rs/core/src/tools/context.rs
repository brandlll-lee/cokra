// Tool Context
// Core context types for tool invocation

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Tool invocation context passed to all handlers
#[derive(Clone)]
pub struct ToolInvocation {
    /// Session reference
    pub session_id: String,

    /// Turn ID
    pub turn_id: String,

    /// Call ID for this tool call
    pub call_id: String,

    /// Tool name
    pub tool_name: String,

    /// Payload for the tool
    pub payload: ToolPayload,
}

/// Different payload types for tool calls
#[derive(Clone, Debug)]
pub enum ToolPayload {
    /// Function call with arguments
    Function {
        arguments: String,
    },

    /// Custom input
    Custom {
        input: String,
    },

    /// Local shell command
    LocalShell {
        params: ShellToolCallParams,
    },

    /// MCP tool call
    Mcp {
        server: String,
        tool: String,
        raw_arguments: String,
    },
}

/// Shell tool call parameters
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShellToolCallParams {
    /// Command to execute
    pub command: Vec<String>,

    /// Working directory
    pub workdir: Option<String>,

    /// Timeout in milliseconds
    pub timeout_ms: Option<u64>,

    /// Environment variables
    pub env: Option<std::collections::HashMap<String, String>>,
}

/// Output from tool execution
#[derive(Clone, Debug)]
pub enum ToolOutput {
    /// Function output
    Function {
        body: FunctionCallOutputBody,
        success: Option<bool>,
    },

    /// MCP result
    Mcp {
        result: Result<CallToolResult, String>,
    },
}

/// Function call output body
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionCallOutputBody {
    /// Output content
    pub content: String,

    /// Exit code (for shell commands)
    pub exit_code: Option<i32>,

    /// Duration in milliseconds
    pub duration_ms: Option<u64>,
}

/// MCP call tool result
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallToolResult {
    /// Content items
    pub content: Vec<ContentItem>,

    /// Whether this is an error
    pub is_error: Option<bool>,
}

/// Content item in MCP result
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentItem {
    /// Content type
    #[serde(rename = "type")]
    pub content_type: String,

    /// Text content
    pub text: Option<String>,

    /// Image URL
    pub image_url: Option<String>,
}

impl ToolPayload {
    /// Parse arguments as JSON
    pub fn parse_arguments<T: for<'de> Deserialize<'de>>(&self) -> Result<T, FunctionCallError> {
        match self {
            ToolPayload::Function { arguments } => {
                serde_json::from_str(arguments)
                    .map_err(|e| FunctionCallError::ParseError(e.to_string()))
            }
            ToolPayload::Custom { input } => {
                serde_json::from_str(input)
                    .map_err(|e| FunctionCallError::ParseError(e.to_string()))
            }
            ToolPayload::LocalShell { params } => {
                serde_json::from_value(serde_json::to_value(params).unwrap())
                    .map_err(|e| FunctionCallError::ParseError(e.to_string()))
            }
            ToolPayload::Mcp { raw_arguments, .. } => {
                serde_json::from_str(raw_arguments)
                    .map_err(|e| FunctionCallError::ParseError(e.to_string()))
            }
        }
    }
}

impl ToolOutput {
    /// Create success output
    pub fn success(content: String) -> Self {
        ToolOutput::Function {
            body: FunctionCallOutputBody {
                content,
                exit_code: Some(0),
                duration_ms: None,
            },
            success: Some(true),
        }
    }

    /// Create error output
    pub fn error(message: String) -> Self {
        ToolOutput::Function {
            body: FunctionCallOutputBody {
                content: message.clone(),
                exit_code: Some(1),
                duration_ms: None,
            },
            success: Some(false),
        }
    }
}

/// Function call error
#[derive(thiserror::Error, Debug)]
pub enum FunctionCallError {
    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Execution error: {0}")]
    ExecutionError(String),

    #[error("Timeout after {0}ms")]
    Timeout(u64),

    #[error("Rejected by user")]
    Rejected,

    #[error("Sandbox error: {0}")]
    SandboxError(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),
}
