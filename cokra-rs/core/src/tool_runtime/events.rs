use serde::Deserialize;
use serde::Serialize;

use super::ToolResult;
use super::ToolSource;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionStage {
  ApprovalRequested,
  Started,
  Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionStatus {
  Pending,
  Running,
  Succeeded,
  Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionEvent {
  pub call_id: String,
  pub tool_id: String,
  pub tool_name: String,
  pub source: ToolSource,
  pub stage: ToolExecutionStage,
  pub status: ToolExecutionStatus,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub result: Option<ToolResult>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub error: Option<String>,
}
