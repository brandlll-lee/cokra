//! 1:1 codex: grep_files tool handler — uses ripgrep with bounded output.
//!
//! Unlike read_file/write_file/list_dir which require absolute paths,
//! grep_files has an optional `path` parameter that defaults to cwd.
//! This mirrors codex's `turn.resolve_path(args.path)` pattern.

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::process::Command;
use tokio::time::timeout;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct GrepFilesHandler;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 2000;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

fn default_limit() -> usize {
  DEFAULT_LIMIT
}

#[derive(Debug, Deserialize)]
struct GrepFilesArgs {
  pattern: String,
  #[serde(default)]
  include: Option<String>,
  path: Option<String>,
  #[serde(default = "default_limit")]
  limit: usize,
}

#[async_trait]
impl ToolHandler for GrepFilesHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: GrepFilesArgs = invocation.parse_arguments()?;
    let pattern = args.pattern.trim();
    if pattern.is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "pattern must not be empty".to_string(),
      ));
    }

    if args.limit == 0 {
      return Err(FunctionCallError::RespondToModel(
        "limit must be greater than zero".to_string(),
      ));
    }

    let limit = args.limit.min(MAX_LIMIT);
    let search_path = invocation.resolve_path(args.path.as_deref());
    verify_path_exists(&search_path).await?;

    let include = args.include.as_deref().map(str::trim).and_then(|val| {
      if val.is_empty() {
        None
      } else {
        Some(val.to_string())
      }
    });

    let search_results = run_rg_search(
      pattern,
      include.as_deref(),
      &search_path,
      limit,
      invocation.cwd.as_path(),
    )
    .await?;

    let content = if search_results.is_empty() {
      "No matches found.".to_string()
    } else {
      search_results.join("\n")
    };

    let mut out = ToolOutput::success(content);
    if search_results.is_empty() {
      out.is_error = true;
    }
    out.id = invocation.id;
    Ok(out)
  }
}

async fn verify_path_exists(path: &Path) -> Result<(), FunctionCallError> {
  tokio::fs::metadata(path).await.map_err(|err| {
    FunctionCallError::RespondToModel(format!("unable to access `{}`: {err}", path.display()))
  })?;
  Ok(())
}

async fn run_rg_search(
  pattern: &str,
  include: Option<&str>,
  search_path: &Path,
  limit: usize,
  cwd: &Path,
) -> Result<Vec<String>, FunctionCallError> {
  let mut command = Command::new("rg");
  command
    .current_dir(cwd)
    .arg("--files-with-matches")
    .arg("--sortr=modified")
    .arg("--regexp")
    .arg(pattern)
    .arg("--no-messages");

  if let Some(glob) = include {
    command.arg("--glob").arg(glob);
  }

  command.arg("--").arg(search_path);

  let output = timeout(COMMAND_TIMEOUT, command.output())
    .await
    .map_err(|_| FunctionCallError::RespondToModel("rg timed out after 30 seconds".to_string()))?
    .map_err(|err| {
      FunctionCallError::RespondToModel(format!(
        "failed to launch rg: {err}. Ensure ripgrep is installed and on PATH."
      ))
    })?;

  match output.status.code() {
    Some(0) => Ok(parse_results(&output.stdout, limit)),
    Some(1) => Ok(Vec::new()),
    _ => {
      let stderr = String::from_utf8_lossy(&output.stderr);
      Err(FunctionCallError::RespondToModel(format!(
        "rg failed: {stderr}"
      )))
    }
  }
}

fn parse_results(stdout: &[u8], limit: usize) -> Vec<String> {
  let mut results = Vec::new();
  for line in stdout.split(|byte| *byte == b'\n') {
    if line.is_empty() {
      continue;
    }
    if let Ok(text) = std::str::from_utf8(line) {
      if text.is_empty() {
        continue;
      }
      results.push(text.to_string());
      if results.len() == limit {
        break;
      }
    }
  }
  results
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::process::Command as StdCommand;
  use tempfile::tempdir;

  fn rg_available() -> bool {
    StdCommand::new("rg")
      .arg("--version")
      .output()
      .map(|output| output.status.success())
      .unwrap_or(false)
  }

  #[test]
  fn parses_basic_results() {
    let stdout = b"/tmp/file_a.rs\n/tmp/file_b.rs\n";
    let parsed = parse_results(stdout, 10);
    assert_eq!(
      parsed,
      vec!["/tmp/file_a.rs".to_string(), "/tmp/file_b.rs".to_string()]
    );
  }

  #[test]
  fn parse_truncates_after_limit() {
    let stdout = b"/tmp/file_a.rs\n/tmp/file_b.rs\n/tmp/file_c.rs\n";
    let parsed = parse_results(stdout, 2);
    assert_eq!(
      parsed,
      vec!["/tmp/file_a.rs".to_string(), "/tmp/file_b.rs".to_string()]
    );
  }

  #[tokio::test]
  async fn run_search_returns_results() -> anyhow::Result<()> {
    if !rg_available() {
      return Ok(());
    }
    let temp = tempdir().expect("create temp dir");
    let dir = temp.path();
    std::fs::write(dir.join("match_one.txt"), "alpha beta gamma").expect("write");
    std::fs::write(dir.join("match_two.txt"), "alpha delta").expect("write");
    std::fs::write(dir.join("other.txt"), "omega").expect("write");

    let results = run_rg_search("alpha", None, dir, 10, dir).await?;
    assert_eq!(results.len(), 2);
    assert!(results.iter().any(|path| path.ends_with("match_one.txt")));
    assert!(results.iter().any(|path| path.ends_with("match_two.txt")));
    Ok(())
  }

  #[tokio::test]
  async fn run_search_with_glob_filter() -> anyhow::Result<()> {
    if !rg_available() {
      return Ok(());
    }
    let temp = tempdir().expect("create temp dir");
    let dir = temp.path();
    std::fs::write(dir.join("match_one.rs"), "alpha beta gamma").expect("write");
    std::fs::write(dir.join("match_two.txt"), "alpha delta").expect("write");

    let results = run_rg_search("alpha", Some("*.rs"), dir, 10, dir).await?;
    assert_eq!(results.len(), 1);
    assert!(results.iter().all(|path| path.ends_with("match_one.rs")));
    Ok(())
  }

  #[tokio::test]
  async fn run_search_respects_limit() -> anyhow::Result<()> {
    if !rg_available() {
      return Ok(());
    }
    let temp = tempdir().expect("create temp dir");
    let dir = temp.path();
    std::fs::write(dir.join("one.txt"), "alpha one").expect("write");
    std::fs::write(dir.join("two.txt"), "alpha two").expect("write");
    std::fs::write(dir.join("three.txt"), "alpha three").expect("write");

    let results = run_rg_search("alpha", None, dir, 2, dir).await?;
    assert_eq!(results.len(), 2);
    Ok(())
  }

  #[tokio::test]
  async fn run_search_handles_no_matches() -> anyhow::Result<()> {
    if !rg_available() {
      return Ok(());
    }
    let temp = tempdir().expect("create temp dir");
    let dir = temp.path();
    std::fs::write(dir.join("one.txt"), "omega").expect("write");

    let results = run_rg_search("alpha", None, dir, 5, dir).await?;
    assert!(results.is_empty());
    Ok(())
  }
}
