//! 1:1 codex: grep_files tool handler — uses session cwd for path resolution.
//!
//! Unlike read_file/write_file/list_dir which require absolute paths,
//! grep_files has an optional `path` parameter that defaults to cwd.
//! This mirrors codex's `turn.resolve_path(args.path)` pattern.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct GrepFilesHandler;

#[derive(Debug, Deserialize)]
struct GrepFilesArgs {
  pattern: String,
  path: Option<String>,
}

#[async_trait]
impl ToolHandler for GrepFilesHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: GrepFilesArgs = invocation.parse_arguments()?;

    // 1:1 codex: use session cwd via resolve_path for optional path param.
    let root = invocation.resolve_path(args.path.as_deref());

    let mut files = Vec::new();
    collect_files(&root, &mut files).map_err(|e| {
      FunctionCallError::Execution(format!("failed to scan {}: {e}", root.display()))
    })?;

    let mut matches = Vec::new();
    for file in files {
      if let Ok(content) = fs::read_to_string(&file) {
        for (idx, line) in content.lines().enumerate() {
          if line.contains(&args.pattern) {
            matches.push(format!("{}:{}:{}", file.display(), idx + 1, line));
          }
        }
      }
    }

    let mut out = ToolOutput::success(matches.join("\n"));
    out.id = invocation.id;
    Ok(out)
  }
}

fn collect_files(path: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
  if path.is_file() {
    out.push(path.to_path_buf());
    return Ok(());
  }

  for entry in fs::read_dir(path)? {
    let entry = entry?;
    let p = entry.path();
    if p.is_dir() {
      collect_files(&p, out)?;
    } else if p.is_file() {
      out.push(p);
    }
  }

  Ok(())
}
