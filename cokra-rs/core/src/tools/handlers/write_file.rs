use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct WriteFileHandler;

#[derive(Debug, Deserialize)]
struct WriteFileArgs {
  file_path: String,
  content: String,
}

impl ToolHandler for WriteFileHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _: &ToolInvocation) -> bool {
    true
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: WriteFileArgs = invocation.parse_arguments()?;

    let path = Path::new(&args.file_path);
    if let Some(parent) = path.parent()
      && !parent.as_os_str().is_empty()
    {
      fs::create_dir_all(parent).map_err(|e| {
        FunctionCallError::Execution(format!("failed to create {}: {e}", parent.display()))
      })?;
    }

    fs::write(path, args.content.as_bytes()).map_err(|e| {
      FunctionCallError::Execution(format!("failed to write {}: {e}", path.display()))
    })?;

    let mut out = ToolOutput::success(format!("wrote {}", path.display()));
    out.id = invocation.id;
    Ok(out)
  }
}

#[cfg(test)]
mod tests {
  use std::fs;

  use super::WriteFileHandler;
  use crate::tools::context::ToolInvocation;
  use crate::tools::registry::ToolHandler;

  #[test]
  fn writes_file_content() {
    let path = std::env::temp_dir().join(format!("cokra-write-{}.txt", uuid::Uuid::new_v4()));

    let inv = ToolInvocation {
      id: "1".to_string(),
      name: "write_file".to_string(),
      arguments: serde_json::json!({
        "file_path": path.display().to_string(),
        "content": "hello"
      })
      .to_string(),
    };

    let out = WriteFileHandler.handle(inv).expect("write file");
    assert_eq!(out.is_error, false);
    let written = fs::read_to_string(&path).expect("read written file");
    assert_eq!(written, "hello".to_string());

    let _ = fs::remove_file(path);
  }
}
