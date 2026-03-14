use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;

use crate::lsp;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct DiagnosticsHandler;

#[derive(Debug, Deserialize)]
struct DiagnosticsArgs {
  path: String,
  #[serde(default = "default_max_diagnostics")]
  max_diagnostics: usize,
}

fn default_max_diagnostics() -> usize {
  50
}

#[async_trait]
impl ToolHandler for DiagnosticsHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _: &ToolInvocation) -> bool {
    false
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let id = invocation.id.clone();
    let args: DiagnosticsArgs = invocation.parse_arguments()?;
    let path = invocation.resolve_path(Some(&args.path));

    let diagnostics = lsp::diagnostics_for_path(&path, args.max_diagnostics)
      .await
      .map_err(map_lsp_error)?;

    if diagnostics.is_empty() {
      return Ok(ToolOutput::success(format!("No diagnostics for {}", path.display())).with_id(id));
    }

    Ok(ToolOutput::success(lsp::format_diagnostics(&path, &diagnostics)).with_id(id))
  }
}

pub async fn collect_file_diagnostics(path: &Path) -> String {
  lsp::collect_file_diagnostics(path).await
}

fn map_lsp_error(error: lsp::LspError) -> FunctionCallError {
  match error {
    lsp::LspError::Disabled
    | lsp::LspError::FileNotFound(_)
    | lsp::LspError::UnsupportedFile(_)
    | lsp::LspError::ServerUnavailable(_) => FunctionCallError::RespondToModel(error.to_string()),
    lsp::LspError::RequestFailed(_) => FunctionCallError::Execution(error.to_string()),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::tools::context::ToolPayload;
  use tempfile::NamedTempFile;

  #[test]
  fn default_max_diagnostics_is_50() {
    let args: DiagnosticsArgs = serde_json::from_str(r#"{"path":"/foo/bar.rs"}"#).unwrap();
    assert_eq!(args.max_diagnostics, 50);
  }

  #[tokio::test]
  async fn rejects_nonexistent_file() {
    let inv = ToolInvocation {
      id: "t1".to_string(),
      name: "diagnostics".to_string(),
      payload: ToolPayload::Function {
        arguments: r#"{"path":"/nonexistent/file.rs"}"#.to_string(),
      },
      cwd: std::env::temp_dir(),
      runtime: None,
    };
    let err = DiagnosticsHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("file not found"));
  }

  #[tokio::test]
  async fn rejects_unknown_extension() {
    let tmp = NamedTempFile::with_suffix(".unknown123").unwrap();
    let inv = ToolInvocation {
      id: "t2".to_string(),
      name: "diagnostics".to_string(),
      payload: ToolPayload::Function {
        arguments: serde_json::json!({"path": tmp.path().display().to_string()}).to_string(),
      },
      cwd: std::env::temp_dir(),
      runtime: None,
    };
    let err = DiagnosticsHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("no LSP server"));
  }
}
