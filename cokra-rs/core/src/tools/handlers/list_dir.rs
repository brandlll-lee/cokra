use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct ListDirHandler;

#[derive(Debug, Deserialize)]
struct ListDirArgs {
  dir_path: String,
  recursive: Option<bool>,
}

impl ToolHandler for ListDirHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: ListDirArgs = invocation.parse_arguments()?;
    let root = PathBuf::from(args.dir_path);

    if !root.exists() {
      return Err(FunctionCallError::Execution(format!(
        "directory does not exist: {}",
        root.display()
      )));
    }

    let mut entries = Vec::new();
    list_entries(&root, args.recursive.unwrap_or(false), &mut entries).map_err(|e| {
      FunctionCallError::Execution(format!("failed to list {}: {e}", root.display()))
    })?;

    entries.sort();
    let mut out = ToolOutput::success(entries.join("\n"));
    out.id = invocation.id;
    Ok(out)
  }
}

fn list_entries(path: &Path, recursive: bool, out: &mut Vec<String>) -> std::io::Result<()> {
  for entry in fs::read_dir(path)? {
    let entry = entry?;
    let path = entry.path();
    out.push(path.display().to_string());

    if recursive && path.is_dir() {
      list_entries(&path, true, out)?;
    }
  }

  Ok(())
}
