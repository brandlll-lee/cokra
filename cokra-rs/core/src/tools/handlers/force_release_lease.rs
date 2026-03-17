use async_trait::async_trait;
use serde::Deserialize;

use cokra_protocol::CollabTeamSnapshotEvent;
use cokra_protocol::EventMsg;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ForceReleaseLeaseHandler;

#[derive(Debug, Deserialize)]
struct ForceReleaseLeaseArgs {
  lease_id: String,
}

#[async_trait]
impl ToolHandler for ForceReleaseLeaseHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: ForceReleaseLeaseArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("force_release_lease missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("force_release_lease runtime is not configured".to_string())
    })?;
    let lease = team_runtime
      .force_release_lease(&runtime.thread_id, &args.lease_id)
      .await
      .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;

    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabTeamSnapshot(CollabTeamSnapshotEvent {
          actor_thread_id: runtime.thread_id.clone(),
          snapshot: team_runtime.snapshot(),
        }))
        .await;
    }

    let out = ToolOutput::success(serde_json::to_string(&lease).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize force released lease: {err}"))
    })?);
    Ok(out.with_id(invocation.id))
  }
}
