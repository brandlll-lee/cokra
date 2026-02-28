use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

use crate::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct ShellHandler;

#[derive(Debug, Deserialize)]
struct ShellArgs {
  command: String,
  timeout_ms: Option<u64>,
  workdir: Option<PathBuf>,
}

impl ToolHandler for ShellHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _: &ToolInvocation) -> bool {
    true
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: ShellArgs = invocation.parse_arguments()?;

    let mut cmd = Command::new("bash");
    cmd.arg("-lc").arg(&args.command);

    if let Some(workdir) = args.workdir {
      cmd.current_dir(workdir);
    }

    let output = cmd
      .output()
      .map_err(|e| FunctionCallError::Execution(format!("shell failed to start: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let exit = output.status.code().unwrap_or(-1);

    let mut content = format!("exit_code: {exit}\n");
    if !stdout.is_empty() {
      content.push_str("stdout:\n");
      content.push_str(&stdout);
      if !content.ends_with('\n') {
        content.push('\n');
      }
    }
    if !stderr.is_empty() {
      content.push_str("stderr:\n");
      content.push_str(&stderr);
      if !content.ends_with('\n') {
        content.push('\n');
      }
    }

    let mut out = ToolOutput::success(content);
    out.id = invocation.id;
    out.is_error = exit != 0;

    if args.timeout_ms.is_some() {
      // Parsed for compatibility; timeout support can be implemented with async process manager.
    }

    Ok(out)
  }
}
