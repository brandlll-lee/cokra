use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::exec::PermissionProfile;
use crate::exec::SandboxPermissions;
use crate::session::Session;
use cokra_protocol::EventMsg;

/// Invocation payload passed to a tool handler.
///
/// 1:1 codex: includes session-level `cwd` so handlers can resolve paths
/// against the correct working directory instead of `std::env::current_dir()`.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
  pub id: String,
  pub name: String,
  pub payload: ToolPayload,
  /// Session-level working directory. Handlers that accept file paths should
  /// use this for resolution instead of the process-level cwd.
  pub cwd: PathBuf,
  /// Optional turn-scoped runtime context for handlers that need to emit
  /// events and block on user responses, such as `request_user_input`.
  pub runtime: Option<Arc<ToolRuntimeContext>>,
}

#[derive(Clone)]
pub struct ToolRuntimeContext {
  pub session: Arc<Session>,
  pub tx_event: Option<mpsc::Sender<EventMsg>>,
  pub thread_id: String,
  pub turn_id: String,
}

impl fmt::Debug for ToolRuntimeContext {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("ToolRuntimeContext")
      .field("thread_id", &self.thread_id)
      .field("turn_id", &self.turn_id)
      .finish_non_exhaustive()
  }
}

impl ToolInvocation {
  /// 1:1 codex: parse failures are `RespondToModel` so the LLM sees the
  /// error message and can self-correct, rather than aborting the turn.
  pub fn parse_arguments<T: DeserializeOwned>(&self) -> Result<T, FunctionCallError> {
    serde_json::from_str(self.raw_arguments()?).map_err(|e| {
      FunctionCallError::RespondToModel(format!("invalid arguments for {}: {e}", self.name))
    })
  }

  /// 1:1 codex: same RespondToModel treatment for raw Value parsing.
  pub fn parse_arguments_value(&self) -> Result<serde_json::Value, FunctionCallError> {
    serde_json::from_str(self.raw_arguments()?).map_err(|e| {
      FunctionCallError::RespondToModel(format!("invalid arguments for {}: {e}", self.name))
    })
  }

  pub fn raw_arguments(&self) -> Result<&str, FunctionCallError> {
    match &self.payload {
      ToolPayload::Function { arguments } | ToolPayload::Mcp { raw_arguments: arguments, .. } => {
        Ok(arguments)
      }
      ToolPayload::Custom { .. } => Err(FunctionCallError::RespondToModel(format!(
        "{} is a custom tool and does not accept JSON arguments",
        self.name
      ))),
      ToolPayload::LocalShell { .. } => Err(FunctionCallError::RespondToModel(format!(
        "{} uses local shell params, not JSON arguments",
        self.name
      ))),
    }
  }

  /// 1:1 codex TurnContext::resolve_path — resolve an optional path against
  /// the session cwd. If `path` is `None`, returns `self.cwd`. If `path` is
  /// absolute, returns it as-is. If relative, joins with `self.cwd`.
  pub fn resolve_path(&self, path: Option<&str>) -> PathBuf {
    match path {
      Some(p) => {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
          pb
        } else {
          self.cwd.join(pb)
        }
      }
      None => self.cwd.clone(),
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellToolCallParams {
  pub command: Vec<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub workdir: Option<String>,
  #[serde(default, alias = "timeout", skip_serializing_if = "Option::is_none")]
  pub timeout_ms: Option<u64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub sandbox_permissions: Option<SandboxPermissions>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub prefix_rule: Option<Vec<String>>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub additional_permissions: Option<PermissionProfile>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub justification: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ToolPayload {
  Function {
    arguments: String,
  },
  Custom {
    input: String,
  },
  LocalShell {
    params: ShellToolCallParams,
  },
  Mcp {
    server: String,
    tool: String,
    raw_arguments: String,
  },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutputBody {
  Text {
    text: String,
  },
}

impl ToolOutputBody {
  pub fn to_text(&self) -> String {
    match self {
      Self::Text { text } => text.clone(),
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpToolCallResult {
  pub content: Vec<serde_json::Value>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub structured_content: Option<serde_json::Value>,
  #[serde(default)]
  pub is_error: bool,
}

/// Standard output from a tool.
#[derive(Debug, Clone)]
pub enum ToolOutput {
  Function {
    id: String,
    body: ToolOutputBody,
    success: Option<bool>,
  },
  Mcp {
    id: String,
    result: Result<McpToolCallResult, String>,
  },
}

impl ToolOutput {
  pub fn success(content: impl Into<String>) -> Self {
    Self::Function {
      id: String::new(),
      body: ToolOutputBody::Text {
        text: content.into(),
      },
      success: Some(true),
    }
  }

  pub fn error(content: impl Into<String>) -> Self {
    Self::Function {
      id: String::new(),
      body: ToolOutputBody::Text {
        text: content.into(),
      },
      success: Some(false),
    }
  }

  pub fn with_id(self, id: impl Into<String>) -> Self {
    match self {
      Self::Function { body, success, .. } => Self::Function {
        id: id.into(),
        body,
        success,
      },
      Self::Mcp { result, .. } => Self::Mcp {
        id: id.into(),
        result,
      },
    }
  }

  pub fn with_success(self, success: bool) -> Self {
    match self {
      Self::Function { id, body, .. } => Self::Function {
        id,
        body,
        success: Some(success),
      },
      other => other,
    }
  }

  pub fn id(&self) -> &str {
    match self {
      Self::Function { id, .. } | Self::Mcp { id, .. } => id,
    }
  }

  pub fn text_content(&self) -> String {
    match self {
      Self::Function { body, .. } => body.to_text(),
      Self::Mcp { result, .. } => match result {
        Ok(result) => serde_json::to_string(result).unwrap_or_else(|_| "{}".to_string()),
        Err(message) => message.clone(),
      },
    }
  }

  pub fn is_error(&self) -> bool {
    match self {
      Self::Function { success, .. } => success == &Some(false),
      Self::Mcp { result, .. } => result.is_err(),
    }
  }
}

/// Shared tool runtime context.
#[derive(Debug, Clone)]
pub struct ToolContext {
  pub cwd: PathBuf,
}

impl Default for ToolContext {
  fn default() -> Self {
    Self {
      cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
  }
}

/// Tool invocation failures.
#[derive(Debug, Clone)]
pub enum FunctionCallError {
  InvalidArguments(String),
  ToolNotFound(String),
  PermissionDenied(String),
  Validation(String),
  RespondToModel(String),
  Fatal(String),
  Execution(String),
  Other(String),
}

impl fmt::Display for FunctionCallError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      FunctionCallError::InvalidArguments(msg)
      | FunctionCallError::ToolNotFound(msg)
      | FunctionCallError::PermissionDenied(msg)
      | FunctionCallError::Validation(msg)
      | FunctionCallError::RespondToModel(msg)
      | FunctionCallError::Fatal(msg)
      | FunctionCallError::Execution(msg)
      | FunctionCallError::Other(msg) => write!(f, "{msg}"),
    }
  }
}

impl FunctionCallError {
  /// 1:1 codex: only `Fatal` aborts the turn. All other variants should be
  /// sent back to the model as a tool output so it can self-correct.
  pub fn is_fatal(&self) -> bool {
    matches!(self, FunctionCallError::Fatal(_))
  }
}

impl std::error::Error for FunctionCallError {}
