use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct GrepFilesHandler;

#[derive(Debug, Deserialize)]
struct GrepFilesArgs {
  pattern: String,
  path: Option<String>,
}

impl ToolHandler for GrepFilesHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: GrepFilesArgs = invocation.parse_arguments()?;
    let root = PathBuf::from(args.path.unwrap_or_else(|| ".".to_string()));

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
