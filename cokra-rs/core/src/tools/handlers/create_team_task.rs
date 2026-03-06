use async_trait::async_trait;
use serde::Deserialize;

use cokra_protocol::CollabTaskUpdatedEvent;
use cokra_protocol::EventMsg;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct CreateTeamTaskHandler;

#[derive(Debug, Deserialize)]
struct CreateTeamTaskArgs {
  title: String,
  details: Option<String>,
  assignee_thread_id: Option<String>,
}

#[async_trait]
impl ToolHandler for CreateTeamTaskHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: CreateTeamTaskArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("create_team_task missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("create_team_task runtime is not configured".to_string())
    })?;
    let task = team_runtime.create_task(args.title, args.details, args.assignee_thread_id);

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabTaskUpdated(CollabTaskUpdatedEvent {
          actor_thread_id: runtime.thread_id.clone(),
          task: task.clone(),
        }))
        .await;
    }

    let mut out =
      ToolOutput::success(serde_json::to_string(&task).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize team task: {err}"))
      })?);
    out.id = invocation.id;
    Ok(out)
  }
}
