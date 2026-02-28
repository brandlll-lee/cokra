use std::fs;

use serde::Deserialize;

use crate::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct ReadFileHandler;

#[derive(Debug, Deserialize)]
struct ReadFileArgs {
  file_path: String,
  offset: Option<usize>,
  limit: Option<usize>,
}

impl ToolHandler for ReadFileHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: ReadFileArgs = invocation.parse_arguments()?;

    let content = fs::read_to_string(&args.file_path).map_err(|e| {
      FunctionCallError::Execution(format!("failed to read {}: {e}", args.file_path))
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
    };

    let out = ReadFileHandler.handle(inv).expect("read file");
    assert_eq!(out.content, "b\nc".to_string());

    let _ = fs::remove_file(path);
  }
}
