//! glob tool handler — file discovery by glob pattern.
//!
//! Modelled after OpenCode's `glob` tool and Gemini CLI's `glob` tool.
//! Uses ripgrep's `--files` mode with `--glob` filtering for fast gitignore-aware file discovery.

use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct GlobHandler;

const MAX_RESULTS: usize = 100;

fn default_path() -> Option<String> {
  None
}

#[derive(Debug, Deserialize)]
struct GlobArgs {
  pattern: String,
  #[serde(default = "default_path")]
  path: Option<String>,
}

#[async_trait]
impl ToolHandler for GlobHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let id = invocation.id.clone();
    let cwd = invocation.cwd.clone();
    let args: GlobArgs = invocation.parse_arguments()?;

    let search_dir = match &args.path {
      Some(p) => {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
          pb
        } else {
          cwd.join(p)
        }
      }
      None => cwd,
    };

    if !search_dir.is_dir() {
      return Err(FunctionCallError::RespondToModel(format!(
        "path is not a directory: {}",
        search_dir.display()
      )));
    }

    // Try ripgrep first, fall back to walkdir-style glob
    match glob_via_rg(&search_dir, &args.pattern).await {
      Ok(files) => {
        let truncated = files.len() > MAX_RESULTS;
        let limited: Vec<&str> = files.iter().take(MAX_RESULTS).map(|s| s.as_str()).collect();

        if limited.is_empty() {
          return Ok(ToolOutput::success("No files found".to_string()).with_id(id));
        }

        let mut output = limited.join("\n");
        if truncated {
          output.push_str(&format!(
            "\n\n(Results truncated. {} files matched, showing first {}. Use a more specific pattern or path.)",
            files.len(),
            MAX_RESULTS
          ));
        }
        Ok(ToolOutput::success(output).with_id(id))
      }
      Err(_) => {
        // Fallback: use std glob
        glob_via_std(&search_dir, &args.pattern, &id)
      }
    }
  }
}

/// Use ripgrep `--files --glob <pattern>` for fast, gitignore-aware file listing.
async fn glob_via_rg(
  search_dir: &std::path::Path,
  pattern: &str,
) -> Result<Vec<String>, FunctionCallError> {
  let rg = which::which("rg")
    .map_err(|_| FunctionCallError::Execution("rg (ripgrep) not found in PATH".to_string()))?;

  let output = tokio::process::Command::new(rg)
    .args(["--files", "--glob", pattern, "--no-messages", "--hidden"])
    .current_dir(search_dir)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .output()
    .await
    .map_err(|e| FunctionCallError::Execution(format!("failed to run rg: {e}")))?;

  let stdout = String::from_utf8_lossy(&output.stdout);
  let mut files: Vec<String> = stdout
    .lines()
    .filter(|l| !l.is_empty())
    .map(|l| {
      let full = search_dir.join(l);
      full.display().to_string()
    })
    .collect();

  // Sort by path for deterministic output
  files.sort();
  Ok(files)
}

/// Fallback using Rust's `glob` crate pattern matching via std::fs.
fn glob_via_std(
  search_dir: &std::path::Path,
  pattern: &str,
  id: &str,
) -> Result<ToolOutput, FunctionCallError> {
  let full_pattern = search_dir.join(pattern);
  let pattern_str = full_pattern.display().to_string();

  let entries: Vec<String> = glob::glob(&pattern_str)
    .map_err(|e| FunctionCallError::RespondToModel(format!("invalid glob pattern: {e}")))?
    .filter_map(|entry| entry.ok())
    .filter(|p| p.is_file())
    .take(MAX_RESULTS + 1)
    .map(|p| p.display().to_string())
    .collect();

  if entries.is_empty() {
    return Ok(ToolOutput::success("No files found".to_string()).with_id(id.to_string()));
  }

  let truncated = entries.len() > MAX_RESULTS;
  let limited: Vec<&str> = entries.iter().take(MAX_RESULTS).map(|s| s.as_str()).collect();

  let mut output = limited.join("\n");
  if truncated {
    output.push_str("\n\n(Results truncated. Use a more specific pattern or path.)");
  }
  Ok(ToolOutput::success(output).with_id(id.to_string()))
}

#[cfg(test)]
mod tests {
  use std::fs;

  use super::*;
  use crate::tools::context::ToolPayload;

  fn make_inv(id: &str, args: serde_json::Value, cwd: &std::path::Path) -> ToolInvocation {
    ToolInvocation {
      id: id.to_string(),
      name: "glob".to_string(),
      payload: ToolPayload::Function {
        arguments: args.to_string(),
      },
      cwd: cwd.to_path_buf(),
      runtime: None,
    }
  }

  #[tokio::test]
  async fn finds_matching_files() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("foo.rs"), "fn main() {}").unwrap();
    fs::write(dir.path().join("bar.rs"), "fn test() {}").unwrap();
    fs::write(dir.path().join("baz.txt"), "text").unwrap();

    let inv = make_inv(
      "1",
      serde_json::json!({ "pattern": "*.rs" }),
      dir.path(),
    );
    let out = GlobHandler.handle_async(inv).await.unwrap();
    let text = out.text_content();
    assert!(text.contains("foo.rs"));
    assert!(text.contains("bar.rs"));
    assert!(!text.contains("baz.txt"));
  }

  #[tokio::test]
  async fn returns_no_files_found() {
    let dir = tempfile::tempdir().unwrap();
    let inv = make_inv(
      "2",
      serde_json::json!({ "pattern": "*.nonexistent" }),
      dir.path(),
    );
    let out = GlobHandler.handle_async(inv).await.unwrap();
    assert!(out.text_content().contains("No files found"));
  }

  #[tokio::test]
  async fn uses_explicit_path() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("subdir");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("test.js"), "console.log()").unwrap();
    fs::write(dir.path().join("root.js"), "top").unwrap();

    let inv = make_inv(
      "3",
      serde_json::json!({
        "pattern": "*.js",
        "path": sub.display().to_string()
      }),
      dir.path(),
    );
    let out = GlobHandler.handle_async(inv).await.unwrap();
    let text = out.text_content();
    assert!(text.contains("test.js"));
    assert!(!text.contains("root.js"));
  }

  #[tokio::test]
  async fn rejects_nonexistent_directory() {
    let inv = make_inv(
      "4",
      serde_json::json!({
        "pattern": "*.rs",
        "path": "/tmp/cokra_nonexistent_dir_test_glob"
      }),
      &std::env::temp_dir(),
    );
    let err = GlobHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("not a directory"));
  }

  #[tokio::test]
  async fn relative_path_resolved_against_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("mydir");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("hello.py"), "print('hi')").unwrap();

    let inv = make_inv(
      "5",
      serde_json::json!({
        "pattern": "*.py",
        "path": "mydir"
      }),
      dir.path(),
    );
    let out = GlobHandler.handle_async(inv).await.unwrap();
    assert!(out.text_content().contains("hello.py"));
  }

  #[tokio::test]
  async fn ignores_files_not_matching_pattern() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("keep.ts"), "export {}").unwrap();
    fs::write(dir.path().join("skip.rs"), "fn x() {}").unwrap();
    fs::write(dir.path().join("skip.py"), "pass").unwrap();

    let inv = make_inv(
      "6",
      serde_json::json!({ "pattern": "*.ts" }),
      dir.path(),
    );
    let out = GlobHandler.handle_async(inv).await.unwrap();
    let text = out.text_content();
    assert!(text.contains("keep.ts"));
    assert!(!text.contains("skip.rs"));
    assert!(!text.contains("skip.py"));
  }

  #[tokio::test]
  async fn finds_files_in_nested_directories() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("a").join("b");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("deep.txt"), "deep").unwrap();
    fs::write(dir.path().join("shallow.txt"), "shallow").unwrap();

    let inv = make_inv(
      "7",
      serde_json::json!({ "pattern": "**/*.txt" }),
      dir.path(),
    );
    let out = GlobHandler.handle_async(inv).await.unwrap();
    let text = out.text_content();
    assert!(text.contains("deep.txt"));
    assert!(text.contains("shallow.txt"));
  }

  #[tokio::test]
  async fn default_path_uses_cwd() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("cwd_file.md"), "# hello").unwrap();

    let inv = make_inv(
      "8",
      serde_json::json!({ "pattern": "*.md" }),
      dir.path(),
    );
    let out = GlobHandler.handle_async(inv).await.unwrap();
    assert!(out.text_content().contains("cwd_file.md"));
  }
}
