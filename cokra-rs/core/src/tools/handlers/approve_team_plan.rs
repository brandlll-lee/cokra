use async_trait::async_trait;
use serde::Deserialize;

use cokra_protocol::CollabPlanDecisionEvent;
use cokra_protocol::EventMsg;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ApproveTeamPlanHandler;

#[derive(Debug, Deserialize)]
struct ApproveTeamPlanArgs {
  plan_id: String,
  approved: bool,
  note: Option<String>,
}

#[async_trait]
impl ToolHandler for ApproveTeamPlanHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: ApproveTeamPlanArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("approve_team_plan missing runtime context".to_string())
    })?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("approve_team_plan runtime is not configured".to_string())
    })?;
    let plan = team_runtime
      .decide_plan(
        &args.plan_id,
        runtime.thread_id.clone(),
        args.approved,
        args.note,
      )
      .await
      .ok_or_else(|| {
        FunctionCallError::RespondToModel(format!("unknown plan id: {}", args.plan_id))
      })?;
    if let Some(tx_event) = &runtime.tx_event {
      let _ = tx_event
        .send(EventMsg::CollabPlanDecision(CollabPlanDecisionEvent {
          actor_thread_id: runtime.thread_id.clone(),
          plan: plan.clone(),
        }))
        .await;
    }
    let out = ToolOutput::success(serde_json::to_string(&plan).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize plan decision: {err}"))
    })?);
    Ok(out.with_id(invocation.id))
  }
}
