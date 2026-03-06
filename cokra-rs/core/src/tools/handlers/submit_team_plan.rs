use async_trait::async_trait;
use serde::Deserialize;

use cokra_protocol::CollabPlanSubmittedEvent;
use cokra_protocol::EventMsg;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct SubmitTeamPlanHandler;

#[derive(Debug, Deserialize)]
struct SubmitTeamPlanArgs {
  summary: String,
  steps: Vec<String>,
  requires_approval: Option<bool>,
}

#[async_trait]
impl ToolHandler for SubmitTeamPlanHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: SubmitTeamPlanArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("submit_team_plan missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("submit_team_plan runtime is not configured".to_string())
    })?;
    if args.steps.is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "submit_team_plan requires at least one step".to_string(),
      ));
    }
    let plan = team_runtime
      .submit_plan(
        runtime.thread_id.clone(),
        args.summary,
        args.steps,
        args.requires_approval.unwrap_or(true),
      )
      .await;
    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabPlanSubmitted(CollabPlanSubmittedEvent {
          actor_thread_id: runtime.thread_id.clone(),
          plan: plan.clone(),
        }))
        .await;
    }
    let mut out = ToolOutput::success(serde_json::to_string(&plan).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize submitted plan: {err}"))
    })?);
    out.id = invocation.id;
    Ok(out)
  }
}
