use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

use super::diagnostics::collect_file_diagnostics;

pub struct ApplyPatchHandler;

#[derive(Debug, Deserialize)]
struct ApplyPatchArgs {
  patch: String,
}

#[async_trait]
impl ToolHandler for ApplyPatchHandler {
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
    let args: ApplyPatchArgs = invocation.parse_arguments()?;
    let cwd = &invocation.cwd;

    match cokra_apply_patch::apply_patch(&args.patch, cwd) {
      Ok(affected) => {
        let summary = cokra_apply_patch::format_summary(&affected);
        let total = affected.added.len() + affected.modified.len() + affected.deleted.len();
        let mut diagnostics = String::new();
        for path in affected.added.iter().chain(affected.modified.iter()) {
          diagnostics.push_str(&collect_file_diagnostics(path).await);
        }
        Ok(
          ToolOutput::success(format!(
            "apply_patch applied ({} file(s) changed)\n{summary}{}",
            total, diagnostics
          ))
          .with_id(invocation.id),
        )
      }
      Err(e) => Err(FunctionCallError::RespondToModel(format!(
        "apply_patch failed: {e}"
      ))),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::tools::context::ToolInvocation;
  use crate::tools::context::ToolPayload;
  use crate::tools::registry::ToolHandler;
  use std::path::PathBuf;
  use tempfile::tempdir;

  fn make_invocation(patch: &str, cwd: PathBuf) -> ToolInvocation {
    let args = serde_json::json!({ "patch": patch });
    ToolInvocation {
      id: "test-id".to_string(),
      name: "apply_patch".to_string(),
      payload: ToolPayload::Function {
        arguments: serde_json::to_string(&args).expect("serialize"),
      },
      cwd,
      runtime: None,
    }
  }

  #[tokio::test]
  async fn test_handler_applies_update_patch() {
    let dir = tempdir().expect("tempdir");
    let file = dir.path().join("hello.txt");
    std::fs::write(&file, "old line\n").expect("write");

    let patch = format!(
      "*** Begin Patch\n*** Update File: {}\n@@\n-old line\n+new line\n*** End Patch",
      file.display()
    );
    let inv = make_invocation(&patch, dir.path().to_path_buf());
    let handler = ApplyPatchHandler;
    let result = handler.handle_async(inv).await.expect("handle");
    assert!(result.text_content().contains("1 file(s) changed"));

    let contents = std::fs::read_to_string(&file).expect("read");
    assert_eq!(contents, "new line\n");
  }

  #[tokio::test]
  async fn test_handler_applies_add_file_patch() {
    let dir = tempdir().expect("tempdir");
    let file = dir.path().join("new.txt");

    let patch = format!(
      "*** Begin Patch\n*** Add File: {}\n+hello world\n*** End Patch",
      file.display()
    );
    let inv = make_invocation(&patch, dir.path().to_path_buf());
    let handler = ApplyPatchHandler;
    let result = handler.handle_async(inv).await.expect("handle");
    assert!(result.text_content().contains("1 file(s) changed"));

    let contents = std::fs::read_to_string(&file).expect("read");
    assert_eq!(contents, "hello world\n");
  }

  #[tokio::test]
  async fn test_handler_applies_delete_file_patch() {
    let dir = tempdir().expect("tempdir");
    let file = dir.path().join("del.txt");
    std::fs::write(&file, "x").expect("write");

    let patch = format!(
      "*** Begin Patch\n*** Delete File: {}\n*** End Patch",
      file.display()
    );
    let inv = make_invocation(&patch, dir.path().to_path_buf());
    let handler = ApplyPatchHandler;
    let result = handler.handle_async(inv).await.expect("handle");
    assert!(result.text_content().contains("1 file(s) changed"));
    assert!(!file.exists());
  }

  #[tokio::test]
  async fn test_handler_returns_error_for_invalid_patch() {
    let dir = tempdir().expect("tempdir");
    let patch = "this is not a valid patch";
    let inv = make_invocation(patch, dir.path().to_path_buf());
    let handler = ApplyPatchHandler;
    let result = handler.handle_async(inv).await;
    assert!(result.is_err());
  }

  #[tokio::test]
  async fn test_handler_resolves_relative_paths() {
    let dir = tempdir().expect("tempdir");
    let patch = "*** Begin Patch\n*** Add File: sub/file.txt\n+content\n*** End Patch";
    let inv = make_invocation(patch, dir.path().to_path_buf());
    let handler = ApplyPatchHandler;
    let result = handler.handle_async(inv).await.expect("handle");
    assert!(result.text_content().contains("1 file(s) changed"));

    let contents = std::fs::read_to_string(dir.path().join("sub/file.txt")).expect("read");
    assert_eq!(contents, "content\n");
  }

  #[tokio::test]
  async fn test_handler_multiple_chunks_update() {
    let dir = tempdir().expect("tempdir");
    let file = dir.path().join("multi.txt");
    std::fs::write(&file, "aaa\nbbb\nccc\nddd\n").expect("write");

    let patch = format!(
      "*** Begin Patch\n*** Update File: {}\n@@\n aaa\n-bbb\n+BBB\n@@\n ccc\n-ddd\n+DDD\n*** End Patch",
      file.display()
    );
    let inv = make_invocation(&patch, dir.path().to_path_buf());
    let handler = ApplyPatchHandler;
    handler.handle_async(inv).await.expect("handle");

    let contents = std::fs::read_to_string(&file).expect("read");
    assert_eq!(contents, "aaa\nBBB\nccc\nDDD\n");
  }

  #[tokio::test]
  async fn test_handler_nonexistent_file_update_fails() {
    let dir = tempdir().expect("tempdir");
    let file = dir.path().join("nope.txt");
    let patch = format!(
      "*** Begin Patch\n*** Update File: {}\n@@\n-old\n+new\n*** End Patch",
      file.display()
    );
    let inv = make_invocation(&patch, dir.path().to_path_buf());
    let handler = ApplyPatchHandler;
    let result = handler.handle_async(inv).await;
    assert!(result.is_err());
  }

  #[tokio::test]
  async fn test_handler_empty_patch_fails() {
    let dir = tempdir().expect("tempdir");
    let patch = "*** Begin Patch\n*** End Patch";
    let inv = make_invocation(patch, dir.path().to_path_buf());
    let handler = ApplyPatchHandler;
    let result = handler.handle_async(inv).await;
    assert!(result.is_err());
  }
}
