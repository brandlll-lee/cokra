use std::fmt;
use std::path::PathBuf;

use serde::de::DeserializeOwned;

/// Invocation payload passed to a tool handler.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
  pub id: String,
  pub name: String,
  pub arguments: String,
}

impl ToolInvocation {
  pub fn parse_arguments<T: DeserializeOwned>(&self) -> Result<T, FunctionCallError> {
    serde_json::from_str(&self.arguments).map_err(|e| {
      FunctionCallError::InvalidArguments(format!("invalid arguments for {}: {e}", self.name))
    })
  }

  pub fn parse_arguments_value(&self) -> Result<serde_json::Value, FunctionCallError> {
    serde_json::from_str(&self.arguments).map_err(|e| {
      FunctionCallError::InvalidArguments(format!("invalid arguments for {}: {e}", self.name))
    })
  }
}

/// Standard output from a tool.
#[derive(Debug, Clone)]
pub struct ToolOutput {
  pub id: String,
  pub content: String,
  pub is_error: bool,
}

impl ToolOutput {
  pub fn success(content: impl Into<String>) -> Self {
    Self {
      id: String::new(),
      content: content.into(),
      is_error: false,
    }
  }

  pub fn error(content: impl Into<String>) -> Self {
    Self {
      id: String::new(),
      content: content.into(),
      is_error: true,
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
      | FunctionCallError::Execution(msg)
      | FunctionCallError::Other(msg) => write!(f, "{msg}"),
    }
  }
}

impl std::error::Error for FunctionCallError {}
