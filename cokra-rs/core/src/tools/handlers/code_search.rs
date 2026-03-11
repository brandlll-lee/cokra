use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct CodeSearchHandler;

fn default_limit() -> usize {
  10
}

fn default_matches() -> usize {
  8
}

#[derive(Debug, Deserialize)]
struct CodeSearchArgs {
  query: String,
  path: Option<String>,
  #[serde(default = "default_limit")]
  limit: usize,
  #[serde(default = "default_matches")]
  max_matches_per_file: usize,
}

#[async_trait]
impl ToolHandler for CodeSearchHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: CodeSearchArgs = invocation.parse_arguments()?;
    let query = args.query.trim();
    if query.is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "query must not be empty".to_string(),
      ));
    }

    if args.limit == 0 {
      return Err(FunctionCallError::RespondToModel(
        "limit must be greater than zero".to_string(),
      ));
    }

    if args.max_matches_per_file == 0 {
      return Err(FunctionCallError::RespondToModel(
        "max_matches_per_file must be greater than zero".to_string(),
      ));
    }

    let root = invocation.resolve_path(args.path.as_deref());
    let params = cokra_file_search::SearchParams {
      root,
      query: query.to_string(),
      max_scanned_files: 1500,
      max_hits: args.limit.min(50),
      max_matches_per_file: args.max_matches_per_file.min(50),
      max_file_bytes: 256 * 1024,
    };

    let output = tokio::task::spawn_blocking(move || cokra_file_search::search(params))
      .await
      .map_err(|err| FunctionCallError::Execution(format!("code_search failed: {err}")))?
      .map_err(|err| FunctionCallError::Execution(format!("code_search failed: {err:#}")))?;

    let result = serde_json::json!({
      "query": output.query,
      "root": output.root.display().to_string(),
      "truncated": output.truncated,
      "files": output.hits.into_iter().map(|hit| {
        serde_json::json!({
          "path": hit.path.display().to_string(),
          "score": hit.score,
          "matches": hit.matches.into_iter().map(|m| {
            serde_json::json!({"line": m.line, "text": m.text})
          }).collect::<Vec<_>>()
        })
      }).collect::<Vec<_>>()
    });

    Ok(ToolOutput::success(result.to_string()).with_id(invocation.id))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use pretty_assertions::assert_eq;
  use tempfile::tempdir;

  #[tokio::test]
  async fn returns_json_with_ranked_files() -> anyhow::Result<()> {
    let temp = tempdir()?;
    std::fs::write(temp.path().join("a.rs"), "pub struct ToolRegistry {}\n")?;
    std::fs::write(temp.path().join("b.rs"), "fn other() {}\n")?;

    let inv = ToolInvocation {
      id: "1".to_string(),
      name: "code_search".to_string(),
      payload: crate::tools::context::ToolPayload::Function {
        arguments: serde_json::json!({
          "query": "ToolRegistry",
          "path": temp.path().display().to_string(),
          "limit": 5,
          "max_matches_per_file": 3
        })
        .to_string(),
      },
      cwd: temp.path().to_path_buf(),
      runtime: None,
    };

    let out = CodeSearchHandler.handle_async(inv).await?;
    assert!(!out.is_error());
    let parsed: serde_json::Value = serde_json::from_str(&out.text_content())?;
    assert_eq!(parsed["query"], "ToolRegistry");
    assert!(
      parsed["files"]
        .as_array()
        .is_some_and(|arr| !arr.is_empty())
    );
    Ok(())
  }
}
