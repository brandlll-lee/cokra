use super::AgentMessageContent;
use super::AgentMessageEvent;
use super::AgentMessageItem;
use super::AgentReasoningEvent;
use super::EventMsg;
use super::PlanItem;
use super::ReasoningItem;
use super::TurnItem;
use super::UserMessageEvent;
use super::UserMessageItem;
use super::WebSearchEndEvent;
use super::WebSearchItem;
use super::user_input::ByteRange;
use super::user_input::TextElement;
use super::user_input::UserInput;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;

/// Agent status enum
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentStatus {
  /// Waiting for initialization
  PendingInit,
  /// Currently executing
  Running,
  /// Done with final message
  Completed(Option<String>),
  /// Encountered error
  Errored(String),
  /// Shut down
  Shutdown,
  /// Agent not found
  NotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TeamTaskStatus {
  Pending,
  InProgress,
  Review,
  Completed,
  Failed,
  Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TeamTaskReadyState {
  Blocked,
  #[default]
  Ready,
  Claimed,
  Review,
  Completed,
  Failed,
  Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TeamTaskReviewState {
  #[default]
  NotRequested,
  Requested,
  Approved,
  ChangesRequested,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TaskBlockerKind {
  Dependency,
  #[default]
  Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskBlocker {
  pub id: String,
  #[serde(default)]
  pub kind: TaskBlockerKind,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub blocking_task_id: Option<String>,
  pub reason: String,
  #[serde(default)]
  pub active: bool,
  pub created_at: i64,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub cleared_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TaskEdgeKind {
  #[default]
  Blocks,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskEdge {
  pub from_task_id: String,
  pub to_task_id: String,
  #[serde(default)]
  pub kind: TaskEdgeKind,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub reason: Option<String>,
  pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum OwnershipScopeKind {
  #[default]
  File,
  Directory,
  Glob,
  Module,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum OwnershipAccessMode {
  SharedRead,
  #[default]
  ExclusiveWrite,
  Review,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScopeRequest {
  #[serde(default)]
  pub kind: OwnershipScopeKind,
  pub path: String,
  #[serde(default)]
  pub access: OwnershipAccessMode,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskNode {
  pub id: String,
  pub title: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub details: Option<String>,
  pub status: TeamTaskStatus,
  #[serde(default)]
  pub ready_state: TeamTaskReadyState,
  #[serde(default)]
  pub review_state: TeamTaskReviewState,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub owner_thread_id: Option<String>,
  #[serde(default)]
  pub blocked_by_task_ids: Vec<String>,
  #[serde(default)]
  pub blocks_task_ids: Vec<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub blocking_reason: Option<String>,
  #[serde(default)]
  pub blockers: Vec<TaskBlocker>,
  #[serde(default)]
  pub requested_scopes: Vec<ScopeRequest>,
  #[serde(default)]
  pub granted_scopes: Vec<ScopeRequest>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub assignee_thread_id: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub workflow_run_id: Option<String>,
  pub created_at: i64,
  pub updated_at: i64,
  #[serde(default)]
  pub notes: Vec<String>,
}

pub type TeamTask = TaskNode;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TeamMessageKind {
  #[default]
  Direct,
  Broadcast,
  Channel,
  Queue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TeamMessageDeliveryMode {
  #[default]
  DurableMail,
  EphemeralNudge,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TeamMessagePriority {
  Low,
  #[default]
  Normal,
  High,
  Urgent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TeamMessageAckState {
  #[default]
  NotRequired,
  Pending,
  Acknowledged,
  Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamMessage {
  pub id: String,
  pub sender_thread_id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub recipient_thread_id: Option<String>,
  #[serde(default)]
  pub kind: TeamMessageKind,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub route_key: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub claimed_by_thread_id: Option<String>,
  #[serde(default)]
  pub delivery_mode: TeamMessageDeliveryMode,
  #[serde(default)]
  pub priority: TeamMessagePriority,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub correlation_id: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub task_id: Option<String>,
  #[serde(default)]
  pub ack_state: TeamMessageAckState,
  pub message: String,
  pub created_at: i64,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub expires_at: Option<i64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub acknowledged_at: Option<i64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub acknowledged_by_thread_id: Option<String>,
  pub unread: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamMember {
  pub thread_id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub nickname: Option<String>,
  pub role: String,
  pub task: String,
  pub depth: usize,
  pub status: AgentStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamSnapshot {
  pub root_thread_id: String,
  pub members: Vec<TeamMember>,
  pub tasks: Vec<TeamTask>,
  #[serde(default)]
  pub task_edges: Vec<TaskEdge>,
  pub plans: Vec<TeamPlan>,
  pub unread_counts: HashMap<String, usize>,
  #[serde(default)]
  pub mailbox_version: u64,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub workflow: Option<WorkflowRuntimeSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TeamPlanStatus {
  Draft,
  PendingApproval,
  Approved,
  Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamPlan {
  pub id: String,
  pub author_thread_id: String,
  pub summary: String,
  pub steps: Vec<String>,
  pub status: TeamPlanStatus,
  pub requires_approval: bool,
  pub reviewer_thread_id: Option<String>,
  pub review_note: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub workflow_run_id: Option<String>,
  pub created_at: i64,
  pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum WorkflowRunStatus {
  #[default]
  Pending,
  Active,
  WaitingApproval,
  Completed,
  Failed,
  Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum WorkflowStepStatus {
  #[default]
  Pending,
  InProgress,
  Completed,
  Blocked,
  Failed,
  Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum WorkflowApprovalStatus {
  #[default]
  NotRequested,
  Pending,
  Approved,
  Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WorkflowApprovalState {
  #[serde(default)]
  pub status: WorkflowApprovalStatus,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub requested_by_thread_id: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub reviewer_thread_id: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub note: Option<String>,
  #[serde(default)]
  pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowStepState {
  pub id: String,
  pub title: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub details: Option<String>,
  #[serde(default)]
  pub status: WorkflowStepStatus,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub assigned_thread_id: Option<String>,
  pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowArtifact {
  pub id: String,
  pub kind: String,
  pub label: String,
  pub content: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub created_by_thread_id: Option<String>,
  pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowRun {
  pub id: String,
  pub workflow_name: String,
  pub title: String,
  pub owner_thread_id: String,
  pub status: WorkflowRunStatus,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub resume_token: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub current_step_id: Option<String>,
  #[serde(default)]
  pub steps: Vec<WorkflowStepState>,
  #[serde(default)]
  pub artifacts: Vec<WorkflowArtifact>,
  #[serde(default)]
  pub approval: WorkflowApprovalState,
  pub created_at: i64,
  pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WorkflowRuntimeSnapshot {
  pub root_thread_id: String,
  #[serde(default)]
  pub runs: Vec<WorkflowRun>,
}

/// Token usage tracking
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
  pub input_tokens: i64,
  pub cached_input_tokens: i64,
  pub output_tokens: i64,
  pub reasoning_output_tokens: i64,
  pub total_tokens: i64,
}

impl TokenUsage {
  pub fn new() -> Self {
    Self {
      input_tokens: 0,
      cached_input_tokens: 0,
      output_tokens: 0,
      reasoning_output_tokens: 0,
      total_tokens: 0,
    }
  }

  pub fn blended_total(&self) -> i64 {
    self.input_tokens - self.cached_input_tokens + self.output_tokens
  }
}

impl Default for TokenUsage {
  fn default() -> Self {
    Self::new()
  }
}

impl UserMessageItem {
  pub fn new(content: &[UserInput]) -> Self {
    Self {
      id: uuid::Uuid::new_v4().to_string(),
      content: content.to_vec(),
    }
  }

  pub fn as_legacy_event(&self, thread_id: &str, turn_id: &str) -> EventMsg {
    EventMsg::UserMessage(UserMessageEvent {
      thread_id: thread_id.to_string(),
      turn_id: turn_id.to_string(),
      items: self.content.clone(),
    })
  }

  pub fn message(&self) -> String {
    self
      .content
      .iter()
      .filter_map(|item| match item {
        UserInput::Text { text, .. } => Some(text.as_str()),
        _ => None,
      })
      .collect::<Vec<_>>()
      .join("\n")
  }

  pub fn text_elements(&self) -> Vec<TextElement> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for input in &self.content {
      if let UserInput::Text {
        text,
        text_elements,
      } = input
      {
        for elem in text_elements {
          out.push(TextElement {
            byte_range: ByteRange {
              start: offset + elem.byte_range.start,
              end: offset + elem.byte_range.end,
            },
            placeholder: elem.placeholder.clone(),
          });
        }
        offset += text.len();
      }
    }
    out
  }
}

impl AgentMessageItem {
  pub fn new(content: &[AgentMessageContent]) -> Self {
    Self {
      id: uuid::Uuid::new_v4().to_string(),
      content: content.to_vec(),
      phase: None,
    }
  }

  pub fn as_legacy_events(&self, thread_id: &str, turn_id: &str) -> Vec<EventMsg> {
    if self.content.is_empty() {
      return Vec::new();
    }
    vec![EventMsg::AgentMessage(AgentMessageEvent {
      thread_id: thread_id.to_string(),
      turn_id: turn_id.to_string(),
      item_id: self.id.clone(),
      content: self.content.clone(),
    })]
  }
}

impl ReasoningItem {
  pub fn as_legacy_events(&self, thread_id: &str, turn_id: &str) -> Vec<EventMsg> {
    let _ = (thread_id, turn_id);
    self
      .summary_text
      .iter()
      .map(|text| EventMsg::AgentReasoning(AgentReasoningEvent { text: text.clone() }))
      .collect()
  }
}

impl WebSearchItem {
  pub fn as_legacy_event(&self, thread_id: &str, turn_id: &str) -> EventMsg {
    let _ = (thread_id, turn_id);
    EventMsg::WebSearchEnd(WebSearchEndEvent {
      query: self.query.clone(),
      action: self.action.clone(),
      call_id: self.id.clone(),
    })
  }
}

impl TurnItem {
  pub fn id(&self) -> String {
    match self {
      TurnItem::UserMessage(item) => item.id.clone(),
      TurnItem::AgentMessage(item) => item.id.clone(),
      TurnItem::Plan(item) => item.id.clone(),
      TurnItem::Reasoning(item) => item.id.clone(),
      TurnItem::WebSearch(item) => item.id.clone(),
    }
  }

  pub fn item_type(&self) -> &'static str {
    match self {
      TurnItem::UserMessage(_) => "user-message",
      TurnItem::AgentMessage(_) => "agent-message",
      TurnItem::Plan(_) => "plan",
      TurnItem::Reasoning(_) => "reasoning",
      TurnItem::WebSearch(_) => "web-search",
    }
  }

  pub fn as_legacy_events(&self, thread_id: &str, turn_id: &str) -> Vec<EventMsg> {
    match self {
      TurnItem::UserMessage(item) => vec![item.as_legacy_event(thread_id, turn_id)],
      TurnItem::AgentMessage(item) => item.as_legacy_events(thread_id, turn_id),
      TurnItem::Plan(PlanItem { .. }) => Vec::new(),
      TurnItem::Reasoning(item) => item.as_legacy_events(thread_id, turn_id),
      TurnItem::WebSearch(item) => vec![item.as_legacy_event(thread_id, turn_id)],
    }
  }
}
