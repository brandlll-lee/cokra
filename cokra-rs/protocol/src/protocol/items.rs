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
  Completed,
  Failed,
  Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamTask {
  pub id: String,
  pub title: String,
  pub details: Option<String>,
  pub status: TeamTaskStatus,
  pub assignee_thread_id: Option<String>,
  pub created_at: i64,
  pub updated_at: i64,
  pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamMessage {
  pub id: String,
  pub sender_thread_id: String,
  pub recipient_thread_id: Option<String>,
  pub message: String,
  pub created_at: i64,
  pub unread: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamMember {
  pub thread_id: String,
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
  pub unread_counts: HashMap<String, usize>,
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
