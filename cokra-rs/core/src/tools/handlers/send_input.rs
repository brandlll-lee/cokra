use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use cokra_protocol::CollabAgentInteractionBeginEvent;
use cokra_protocol::CollabAgentInteractionEndEvent;
use cokra_protocol::CollabMailboxDeliveredEvent;
use cokra_protocol::EventMsg;
use cokra_protocol::TeamMessageDeliveryMode;
use cokra_protocol::TeamMessageKind;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::handlers::team_selectors::resolve_required_agent_selector;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct SendInputHandler;

#[derive(Debug, Deserialize)]
struct SendInputArgs {
  #[serde(alias = "agent")]
  agent_id: String,
  #[serde(alias = "input")]
  message: String,
}

#[derive(Debug, Serialize)]
struct SendInputResult {
  agent_id: String,
  status: String,
}

#[async_trait]
impl ToolHandler for SendInputHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: SendInputArgs = invocation.parse_arguments()?;
    let runtime = invocation
      .runtime
      .ok_or_else(|| FunctionCallError::Fatal("send_input missing runtime context".to_string()))?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("send_input runtime is not configured".to_string())
    })?;
    let agent_id = resolve_required_agent_selector(&team_runtime, &args.agent_id, "agent_id")?;
    let receiver = team_runtime.collab_agent_ref(&agent_id);
    let outbound_message = args.message.clone();

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabAgentInteractionBegin(
          CollabAgentInteractionBeginEvent {
            thread_id: runtime.thread_id.clone(),
            agent_id: agent_id.clone(),
            nickname: receiver.as_ref().and_then(|agent| agent.nickname.clone()),
            role: receiver.as_ref().and_then(|agent| agent.role.clone()),
            message: outbound_message.clone(),
          },
        ))
        .await;
    }

    team_runtime
      .send_input(&agent_id, args.message)
      .await
      .map_err(|err| FunctionCallError::Execution(err.to_string()))?;
    let state = team_runtime.wait_state(&agent_id).unwrap_or_default();

    if let Some(tx_event) = &runtime.tx_event {
      let sender = team_runtime.collab_agent_ref(&runtime.thread_id);
      let recipient = team_runtime.collab_agent_ref(&agent_id);
      let _ = tx_event
        .send(EventMsg::CollabMailboxDelivered(
          CollabMailboxDeliveredEvent {
            thread_id: agent_id.clone(),
            sender_thread_id: runtime.thread_id.clone(),
            sender_nickname: sender.as_ref().and_then(|agent| agent.nickname.clone()),
            sender_role: sender.as_ref().and_then(|agent| agent.role.clone()),
            recipient_thread_id: agent_id.clone(),
            recipient_nickname: recipient.as_ref().and_then(|agent| agent.nickname.clone()),
            recipient_role: recipient.as_ref().and_then(|agent| agent.role.clone()),
            message: outbound_message.clone(),
            task_id: None,
            delivery_mode: TeamMessageDeliveryMode::EphemeralNudge,
            kind: TeamMessageKind::Direct,
            created_at: Utc::now().timestamp(),
          },
        ))
        .await;
    }

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabAgentInteractionEnd(
          CollabAgentInteractionEndEvent {
            thread_id: runtime.thread_id.clone(),
            agent_id: agent_id.clone(),
            nickname: receiver.as_ref().and_then(|agent| agent.nickname.clone()),
            role: receiver.and_then(|agent| agent.role),
            message: outbound_message,
            lifecycle: state.lifecycle,
            turn_outcome: state.turn_outcome,
            last_turn_summary: state.last_turn_summary,
            attention_reason: state.attention_reason,
            pending_wake_count: state.pending_wake_count,
          },
        ))
        .await;
    }

    let out = ToolOutput::success(
      serde_json::to_string(&SendInputResult {
        agent_id,
        status: "sent".to_string(),
      })
      .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize send_input result: {err}"))
      })?,
    );
    Ok(out.with_id(invocation.id))
  }
}
