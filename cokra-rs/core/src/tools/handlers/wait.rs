use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use futures::future::join_all;
use serde::Deserialize;

use cokra_protocol::AgentStatus;
use cokra_protocol::CollabWaitingBeginEvent;
use cokra_protocol::CollabWaitingEndEvent;
use cokra_protocol::EventMsg;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct WaitHandler;

const MIN_WAIT_TIMEOUT_MS: i64 = 10_000;
const DEFAULT_WAIT_TIMEOUT_MS: i64 = 120_000;
const MAX_WAIT_TIMEOUT_MS: i64 = 3_600_000;

#[derive(Debug, Deserialize)]
struct WaitArgs {
  #[serde(alias = "agents")]
  agent_ids: Option<Vec<String>>,
  timeout_ms: Option<i64>,
}

fn normalize_timeout_ms(value: Option<i64>) -> i64 {
  let timeout_ms = value.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
  timeout_ms.clamp(MIN_WAIT_TIMEOUT_MS, MAX_WAIT_TIMEOUT_MS)
}

fn is_final_status(status: &AgentStatus) -> bool {
  matches!(
    status,
    AgentStatus::Completed(_)
      | AgentStatus::Errored(_)
      | AgentStatus::Shutdown
      | AgentStatus::NotFound
  )
}

async fn wait_for_agent_status(
  team_runtime: std::sync::Arc<crate::agent::team_runtime::TeamRuntime>,
  agent_id: String,
  deadline: tokio::time::Instant,
) -> AgentStatus {
  let Some(mut status_rx) = team_runtime.subscribe_status(&agent_id) else {
    return AgentStatus::NotFound;
  };

  loop {
    let current = status_rx.borrow().clone();
    if is_final_status(&current) {
      return current;
    }

    match tokio::time::timeout_at(deadline, status_rx.changed()).await {
      Ok(Ok(())) => {}
      Ok(Err(_)) | Err(_) => return status_rx.borrow().clone(),
    }
  }
}

#[async_trait]
impl ToolHandler for WaitHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: WaitArgs = invocation.parse_arguments()?;
    let runtime = invocation
      .runtime
      .ok_or_else(|| FunctionCallError::Fatal("wait missing runtime context".to_string()))?;
    let team_runtime = runtime_for_thread(&runtime.thread_id)
      .ok_or_else(|| FunctionCallError::Execution("wait runtime is not configured".to_string()))?;
    let agent_ids = args
      .agent_ids
      .filter(|ids| !ids.is_empty())
      .map(|ids| {
        ids
          .into_iter()
          // Tradeoff: keep unresolved selectors as-is so the output can report NotFound.
          .map(|id| team_runtime.resolve_agent_selector(&id).unwrap_or(id))
          .collect::<Vec<_>>()
      })
      .filter(|ids| !ids.is_empty())
      .unwrap_or_else(|| team_runtime.list_spawned_agent_ids());

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabWaitingBegin(CollabWaitingBeginEvent {
          sender_thread_id: runtime.thread_id.clone(),
          receiver_thread_ids: agent_ids.clone(),
          receiver_agents: team_runtime.collab_agent_refs(&agent_ids),
          call_id: invocation.id.clone(),
        }))
        .await;
    }

    let deadline = tokio::time::Instant::now()
      + Duration::from_millis(normalize_timeout_ms(args.timeout_ms) as u64);
    let statuses = join_all(
      agent_ids
        .iter()
        .cloned()
        .map(|agent_id| wait_for_agent_status(team_runtime.clone(), agent_id, deadline)),
    )
    .await;
    let statuses: HashMap<String, AgentStatus> = agent_ids.into_iter().zip(statuses).collect();

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabWaitingEnd(CollabWaitingEndEvent {
          sender_thread_id: runtime.thread_id.clone(),
          call_id: invocation.id.clone(),
          agent_statuses: team_runtime.collab_agent_status_entries(&statuses),
          statuses: statuses.clone(),
        }))
        .await;
    }

    let out = ToolOutput::success(serde_json::to_string(&statuses).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize wait result: {err}"))
    })?);
    Ok(out.with_id(invocation.id))
  }
}
