use async_trait::async_trait;
use serde::Deserialize;

use cokra_protocol::CollabMailboxDeliveredEvent;
use cokra_protocol::CollabMessagePostedEvent;
use cokra_protocol::EventMsg;
use cokra_protocol::TeamMessageDeliveryMode;
use cokra_protocol::TeamMessageKind;
use cokra_protocol::TeamMessagePriority;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::handlers::team_selectors::resolve_optional_agent_selector;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct SendTeamMessageHandler;

#[derive(Debug, Deserialize)]
struct SendTeamMessageArgs {
  message: String,
  recipient_thread_id: Option<String>,
  channel: Option<String>,
  queue_name: Option<String>,
  priority: Option<TeamMessagePriority>,
  correlation_id: Option<String>,
  task_id: Option<String>,
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
    let direct = resolve_optional_agent_selector(
      &team_runtime,
      args.recipient_thread_id.clone(),
      "recipient_thread_id",
    )?;
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
    let recipient = direct
      .as_deref()
      .and_then(|thread_id| team_runtime.collab_agent_ref(thread_id));
    let sender = team_runtime.collab_agent_ref(&runtime.thread_id);
    let message = team_runtime
      .post_message(
        runtime.thread_id.clone(),
        direct.clone(),
        kind,
        route_key,
        TeamMessageDeliveryMode::DurableMail,
        args.priority.unwrap_or_default(),
        args.correlation_id,
        args.task_id,
        args.message.clone(),
        None,
      )
      .await;

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabMessagePosted(CollabMessagePostedEvent {
          sender_thread_id: runtime.thread_id.clone(),
          sender_nickname: sender.as_ref().and_then(|agent| agent.nickname.clone()),
          sender_role: sender.as_ref().and_then(|agent| agent.role.clone()),
          recipient_thread_id: direct.clone(),
          recipient_nickname: recipient.as_ref().and_then(|agent| agent.nickname.clone()),
          recipient_role: recipient.and_then(|agent| agent.role),
          message: args.message,
        }))
        .await;
      match message.kind {
        TeamMessageKind::Direct => {
          if let Some(recipient_thread_id) = message.recipient_thread_id.clone() {
            let recipient = team_runtime.collab_agent_ref(&recipient_thread_id);
            let _ = tx_event
              .send(EventMsg::CollabMailboxDelivered(
                CollabMailboxDeliveredEvent {
                  thread_id: recipient_thread_id.clone(),
                  sender_thread_id: runtime.thread_id.clone(),
                  sender_nickname: sender.as_ref().and_then(|agent| agent.nickname.clone()),
                  sender_role: sender.as_ref().and_then(|agent| agent.role.clone()),
                  recipient_thread_id,
                  recipient_nickname: recipient.as_ref().and_then(|agent| agent.nickname.clone()),
                  recipient_role: recipient.and_then(|agent| agent.role),
                  message: message.message.clone(),
                  task_id: message.task_id.clone(),
                  delivery_mode: message.delivery_mode.clone(),
                  kind: message.kind.clone(),
                  created_at: message.created_at,
                },
              ))
              .await;
          }
        }
        TeamMessageKind::Broadcast | TeamMessageKind::Channel => {
          let mut recipient_thread_ids = team_runtime.list_spawned_agent_ids();
          if let Some(root_thread_id) = team_runtime.resolve_agent_selector("main") {
            recipient_thread_ids.push(root_thread_id);
          }
          recipient_thread_ids.sort();
          recipient_thread_ids.dedup();
          for recipient_thread_id in recipient_thread_ids {
            if recipient_thread_id == runtime.thread_id {
              continue;
            }
            let recipient = team_runtime.collab_agent_ref(&recipient_thread_id);
            let _ = tx_event
              .send(EventMsg::CollabMailboxDelivered(
                CollabMailboxDeliveredEvent {
                  thread_id: recipient_thread_id.clone(),
                  sender_thread_id: runtime.thread_id.clone(),
                  sender_nickname: sender.as_ref().and_then(|agent| agent.nickname.clone()),
                  sender_role: sender.as_ref().and_then(|agent| agent.role.clone()),
                  recipient_thread_id,
                  recipient_nickname: recipient.as_ref().and_then(|agent| agent.nickname.clone()),
                  recipient_role: recipient.and_then(|agent| agent.role),
                  message: message.message.clone(),
                  task_id: message.task_id.clone(),
                  delivery_mode: message.delivery_mode.clone(),
                  kind: message.kind.clone(),
                  created_at: message.created_at,
                },
              ))
              .await;
          }
        }
        TeamMessageKind::Queue => {}
      }
    }

    let out = ToolOutput::success(serde_json::to_string(&message).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize team message: {err}"))
    })?);
    Ok(out.with_id(invocation.id))
  }
}
