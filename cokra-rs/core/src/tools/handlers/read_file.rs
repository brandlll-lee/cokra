//! 1:1 codex: read_file tool handler — requires absolute paths.
//!
//! Unlike grep_files/shell which resolve relative paths against session cwd,
//! read_file rejects relative paths outright. The model is expected to send
//! absolute paths (it learns the cwd from the environment_context message).

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ReadFileHandler;

#[derive(Debug, Deserialize)]
struct ReadFileArgs {
  file_path: String,
  offset: Option<usize>,
  limit: Option<usize>,
}

#[async_trait]
impl ToolHandler for ReadFileHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: ReadFileArgs = invocation.parse_arguments()?;

    // 1:1 codex: require absolute paths. The model knows the cwd from the
    // environment_context and should always send absolute paths for read_file.
    let path = PathBuf::from(&args.file_path);
    if !path.is_absolute() {
      return Err(FunctionCallError::RespondToModel(
        "file_path must be an absolute path".to_string(),
      ));
    }

    let content = std::fs::read_to_string(&path).map_err(|e| {
      FunctionCallError::Execution(format!("failed to read {}: {e}", path.display()))
    })?;

    let offset = args.offset.unwrap_or(0);
    let lines: Vec<&str> = content.lines().collect();
    let start = offset.min(lines.len());
    let end = match args.limit {
      Some(limit) => (start + limit).min(lines.len()),
      None => lines.len(),
    };
    let slice = if start < end {
      lines[start..end].join("\n")
    } else {
      String::new()
    };

    let mut out = ToolOutput::success(slice);
    out.id = invocation.id;
    Ok(out)
  }
}

#[cfg(test)]
mod tests {
  use std::fs;

  use super::ReadFileHandler;
  use crate::tools::context::ToolInvocation;
  use crate::tools::registry::ToolHandler;

  #[test]
  fn reads_lines_with_offset_limit() {
    let path = std::env::temp_dir().join(format!("cokra-read-{}.txt", uuid::Uuid::new_v4()));
    fs::write(&path, "a\nb\nc\nd\n").expect("write test file");

    let inv = ToolInvocation {
      id: "1".to_string(),
      name: "read_file".to_string(),
      arguments: serde_json::json!({
        "file_path": path.display().to_string(),
        "offset": 1,
        "limit": 2
      })
      .to_string(),
      cwd: std::env::temp_dir(),
    };

    let out = ReadFileHandler.handle(inv).expect("read file");
    assert_eq!(out.content, "b\nc".to_string());

    let _ = fs::remove_file(path);
  }

  #[test]
  fn rejects_relative_path() {
    let inv = ToolInvocation {
      id: "2".to_string(),
      name: "read_file".to_string(),
      arguments: serde_json::json!({
        "file_path": "relative/path.rs"
      })
      .to_string(),
      cwd: std::env::temp_dir(),
    };

    let err = ReadFileHandler
      .handle(inv)
      .expect_err("should reject relative path");
    assert!(err.to_string().contains("absolute path"));
  }
}
