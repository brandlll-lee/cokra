use async_trait::async_trait;
use serde::Deserialize;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct PlanHandler;

#[derive(Debug, Deserialize)]
struct PlanArgs {
  text: String,
}

#[async_trait]
impl ToolHandler for PlanHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: PlanArgs = invocation.parse_arguments()?;
    if let Some(runtime) = &invocation.runtime
      && let Some(team_runtime) = runtime_for_thread(&runtime.thread_id)
    {
      team_runtime
        .record_plan_artifact(runtime.thread_id.clone(), args.text.clone())
        .await;
    }
    Ok(ToolOutput::success(args.text).with_id(invocation.id))
  }
}
