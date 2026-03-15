use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct WatchTeamInboxHandler;

#[derive(Debug, Deserialize)]
struct WatchTeamInboxArgs {
  after_version: Option<u64>,
  timeout_ms: Option<u64>,
  unread_only: Option<bool>,
}

#[derive(Debug, Serialize)]
struct WatchTeamInboxResult {
  mailbox_version: u64,
  timed_out: bool,
  messages: Vec<cokra_protocol::TeamMessage>,
}

#[async_trait]
impl ToolHandler for WatchTeamInboxHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: WatchTeamInboxArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("watch_team_inbox missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("watch_team_inbox runtime is not configured".to_string())
    })?;
    let (mailbox_version, messages, timed_out) = team_runtime
      .watch_inbox(
        &runtime.thread_id,
        args.after_version,
        args.timeout_ms,
        args.unread_only.unwrap_or(false),
      )
      .await
      .map_err(|err| FunctionCallError::Execution(err.to_string()))?;

    let out = ToolOutput::success(
      serde_json::to_string(&WatchTeamInboxResult {
        mailbox_version,
        timed_out,
        messages,
      })
      .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize watch_team_inbox result: {err}"))
      })?,
    );
    Ok(out.with_id(invocation.id))
  }
}
