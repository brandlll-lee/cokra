use async_trait::async_trait;
use serde::Deserialize;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct AckTeamMessageHandler;

#[derive(Debug, Deserialize)]
struct AckTeamMessageArgs {
  message_id: String,
}

#[async_trait]
impl ToolHandler for AckTeamMessageHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: AckTeamMessageArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("ack_team_message missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("ack_team_message runtime is not configured".to_string())
    })?;
    let message = team_runtime
      .ack_message(&runtime.thread_id, &args.message_id)
      .await
      .ok_or_else(|| {
        FunctionCallError::RespondToModel(format!(
          "message {} cannot be acknowledged by this teammate",
          args.message_id
        ))
      })?;

    let out = ToolOutput::success(serde_json::to_string(&message).map_err(|err| {
      FunctionCallError::Fatal(format!(
        "failed to serialize acknowledged team message: {err}"
      ))
    })?);
    Ok(out.with_id(invocation.id))
  }
}
