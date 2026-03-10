use async_trait::async_trait;
use serde::Deserialize;

use cokra_protocol::CollabMessagesReadEvent;
use cokra_protocol::EventMsg;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ReadTeamMessagesHandler;

#[derive(Debug, Deserialize)]
struct ReadTeamMessagesArgs {
  unread_only: Option<bool>,
}

#[async_trait]
impl ToolHandler for ReadTeamMessagesHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: ReadTeamMessagesArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("read_team_messages missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("read_team_messages runtime is not configured".to_string())
    })?;
    let messages = team_runtime
      .read_messages(&runtime.thread_id, args.unread_only.unwrap_or(false))
      .await;

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabMessagesRead(CollabMessagesReadEvent {
          reader_thread_id: runtime.thread_id.clone(),
          reader_nickname: None,
          reader_role: None,
          count: messages.len(),
        }))
        .await;
    }

    let mut out = ToolOutput::success(serde_json::to_string(&messages).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize read messages: {err}"))
    })?);
    Ok(out.with_id(invocation.id))
  }
}
