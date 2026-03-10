//! 1:1 codex: Shell tool handler — parse args + delegate to exec layer.
//!
//! This handler NO LONGER spawns processes directly. All execution goes
//! through the unified exec pipeline:
//!
//!   ShellArgs → ExecParams → SandboxManager::transform() → execute_command()
//!
//! See `crate::exec` and `crate::tools::runtimes::shell` for the runtime.

use std::collections::HashMap;

use async_trait::async_trait;
use serde::Deserialize;

use crate::exec::format_exec_error;
use crate::exec::format_exec_output_for_model_structured;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use crate::tools::runtimes::shell::process_shell_command;
use crate::truncate::DEFAULT_TOOL_OUTPUT_TOKENS;
use crate::truncate::TruncationPolicy;

pub struct ShellHandler;

#[derive(Debug, Deserialize)]
struct ShellArgs {
  command: String,
  timeout_ms: Option<u64>,
  workdir: Option<String>,
}

#[async_trait]
impl ToolHandler for ShellHandler {
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
    let args: ShellArgs = invocation.parse_arguments()?;

    // Spec 3.2: use session-cached shell (for now, detect once per call;
    // will be upgraded to session.user_shell() when Session caching lands).
    let shell = crate::shell::default_user_shell();

    // 1:1 codex: resolve workdir against session cwd, not process cwd.
    let cwd = invocation.resolve_path(args.workdir.as_deref());

    // Spec 0+1+2: all execution goes through the unified exec pipeline.
    // The sandbox policy is DangerFullAccess here because the handler
    // doesn't have access to the turn's sandbox policy. The real sandbox
    // policy enforcement happens in the ToolOrchestrator/ToolRouter layer
    // when using ShellRuntime. This handle_async path is the legacy
    // compatibility path for direct registry dispatch.
    let result = process_shell_command(
      &shell,
      &args.command,
      cwd,
      args.timeout_ms,
      HashMap::new(),
      &cokra_protocol::SandboxPolicy::DangerFullAccess,
    )
    .await;

    match result {
      Ok(output) => {
        let content = format_exec_output_for_model_structured(
          &output,
          TruncationPolicy::Tokens(DEFAULT_TOOL_OUTPUT_TOKENS),
        );
        Ok(
          ToolOutput::success(content)
            .with_id(invocation.id)
            .with_success(output.exit_code == 0),
        )
      }
      Err(exec_error) => {
        let message = format_exec_error(&exec_error);
        Err(FunctionCallError::Execution(message))
      }
    }
  }
}
