//! 1:1 codex: write_file tool handler — requires absolute paths.
//! Appends LSP diagnostics to the output on successful write.

use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

use super::diagnostics::collect_file_diagnostics;

pub struct WriteFileHandler;

#[derive(Debug, Deserialize)]
struct WriteFileArgs {
  file_path: String,
  content: String,
}

#[async_trait]
impl ToolHandler for WriteFileHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _: &ToolInvocation) -> bool {
    true
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let id = invocation.id.clone();
    let args: WriteFileArgs = invocation.parse_arguments()?;

    // 1:1 codex: require absolute paths.
    let path = PathBuf::from(&args.file_path);
    if !path.is_absolute() {
      return Err(FunctionCallError::RespondToModel(
        "file_path must be an absolute path".to_string(),
      ));
    }

    if let Some(parent) = path.parent()
      && !parent.as_os_str().is_empty()
    {
      fs::create_dir_all(parent).map_err(|e| {
        FunctionCallError::Execution(format!("failed to create {}: {e}", parent.display()))
      })?;
    }

    fs::write(&path, args.content.as_bytes()).map_err(|e| {
      FunctionCallError::Execution(format!("failed to write {}: {e}", path.display()))
    })?;

    let diag_suffix = collect_file_diagnostics(&path).await;
    Ok(ToolOutput::success(format!("wrote {}{}", path.display(), diag_suffix)).with_id(id))
  }
}

#[cfg(test)]
mod tests {
  use std::fs;

  use super::WriteFileHandler;
  use crate::tools::context::ToolInvocation;
  use crate::tools::registry::ToolHandler;

  #[tokio::test]
  async fn writes_file_content() {
    let path = std::env::temp_dir().join(format!("cokra-write-{}.txt", uuid::Uuid::new_v4()));

    let inv = ToolInvocation {
      id: "1".to_string(),
      name: "write_file".to_string(),
      payload: crate::tools::context::ToolPayload::Function {
        arguments: serde_json::json!({
          "file_path": path.display().to_string(),
          "content": "hello"
        })
        .to_string(),
      },
      cwd: std::env::temp_dir(),
      runtime: None,
    };

    let out = WriteFileHandler
      .handle_async(inv)
      .await
      .expect("write file");
    assert!(!out.is_error());
    let written = fs::read_to_string(&path).expect("read written file");
    assert_eq!(written, "hello".to_string());

    let _ = fs::remove_file(path);
  }

  #[tokio::test]
  async fn rejects_relative_path() {
    let inv = ToolInvocation {
      id: "2".to_string(),
      name: "write_file".to_string(),
      payload: crate::tools::context::ToolPayload::Function {
        arguments: serde_json::json!({
          "file_path": "relative/file.txt",
          "content": "hello"
        })
        .to_string(),
      },
      cwd: std::env::temp_dir(),
      runtime: None,
    };

    let err = WriteFileHandler
      .handle_async(inv)
      .await
      .expect_err("should reject relative path");
    assert!(err.to_string().contains("absolute path"));
  }
}
