use async_trait::async_trait;
use serde::Deserialize;

use cokra_protocol::CollabMessagePostedEvent;
use cokra_protocol::EventMsg;
use cokra_protocol::TeamMessageKind;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct SendTeamMessageHandler;

#[derive(Debug, Deserialize)]
struct SendTeamMessageArgs {
  message: String,
  recipient_thread_id: Option<String>,
  channel: Option<String>,
  queue_name: Option<String>,
}

#[async_trait]
impl ToolHandler for SendTeamMessageHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: SendTeamMessageArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("send_team_message missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("send_team_message runtime is not configured".to_string())
    })?;
    let direct = args.recipient_thread_id.clone();
    let channel = args
      .channel
      .clone()
      .filter(|value| !value.trim().is_empty());
    let queue_name = args
      .queue_name
      .clone()
      .filter(|value| !value.trim().is_empty());
    let kind = if queue_name.is_some() {
      TeamMessageKind::Queue
    } else if channel.is_some() {
      TeamMessageKind::Channel
    } else if direct.is_some() {
      TeamMessageKind::Direct
    } else {
      TeamMessageKind::Broadcast
    };
    let route_key = if queue_name.is_some() {
      queue_name.clone()
    } else {
      channel.clone()
    };
    let message = team_runtime
      .post_message(
        runtime.thread_id.clone(),
        direct,
        kind,
        route_key,
        args.message.clone(),
      )
      .await;

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabMessagePosted(CollabMessagePostedEvent {
          sender_thread_id: runtime.thread_id.clone(),
          sender_nickname: None,
          sender_role: None,
          recipient_thread_id: args.recipient_thread_id,
          recipient_nickname: None,
          recipient_role: None,
          message: args.message,
        }))
        .await;
    }

    let mut out = ToolOutput::success(serde_json::to_string(&message).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize team message: {err}"))
    })?);
    out.id = invocation.id;
    Ok(out)
  }
}
