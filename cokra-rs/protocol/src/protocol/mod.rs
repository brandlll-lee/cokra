// Cokra Protocol Layer
// Complete protocol definitions for Cokra

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

pub mod config_types;
pub mod items;
pub mod models;
pub mod user_input;

pub use config_types::*;
pub use items::*;
pub use models::*;
pub use user_input::*;

// ============================================================================
// EVENT MESSAGES - All events emitted during Cokra operation
// ============================================================================

/// All events sent through the event stream
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventMsg {
  // ========== LIFECYCLE & STATE ==========
  Error(ErrorEvent),
  Warning(WarningEvent),
  TurnStarted(TurnStartedEvent),
  TurnComplete(TurnCompleteEvent),
  TurnAborted(TurnAbortedEvent),

  // ========== CONTENT EVENTS ==========
  TokenCount(TokenCountEvent),
  AgentMessage(AgentMessageEvent),
  UserMessage(UserMessageEvent),
  AgentMessageDelta(AgentMessageDeltaEvent),
  AgentMessageContentDelta(AgentMessageContentDeltaEvent),

  // ========== CONFIGURATION EVENTS ==========
  SessionConfigured(SessionConfiguredEvent),
  ThreadNameUpdated(ThreadNameUpdatedEvent),

  // ========== EXECUTION EVENTS ==========
  ExecCommandBegin(ExecCommandBeginEvent),
  ExecCommandOutputDelta(ExecCommandOutputDeltaEvent),
  ExecCommandEnd(ExecCommandEndEvent),

  // ========== APPROVAL EVENTS ==========
  ExecApprovalRequest(ExecApprovalRequestEvent),
  RequestUserInput(RequestUserInputEvent),

  // ========== NOTIFICATION EVENTS ==========
  StreamError(StreamErrorEvent),
  ShutdownComplete,

  // ========== COLLABORATION EVENTS ==========
  CollabAgentSpawnBegin(CollabAgentSpawnBeginEvent),
  CollabAgentSpawnEnd(CollabAgentSpawnEndEvent),
  CollabAgentInteractionBegin(CollabAgentInteractionBeginEvent),
  CollabAgentInteractionEnd(CollabAgentInteractionEndEvent),

  // ========== NEW ITEM-BASED PROTOCOL ==========
  ItemStarted(ItemStartedEvent),
  ItemCompleted(ItemCompletedEvent),
}

// ============================================================================
// OPERATION TYPES - All operations submitted to Cokra
// ============================================================================

/// All operations that can be submitted to Cokra
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Op {
  /// Configure session runtime defaults before processing turns.
  ConfigureSession {
    cwd: PathBuf,
    approval_policy: AskForApproval,
    sandbox_policy: SandboxPolicy,
    model: String,
  },

  /// Interrupt current operation
  Interrupt,

  /// Clean background terminals
  CleanBackgroundTerminals,

  /// User input (text, images, etc.)
  UserInput {
    items: Vec<UserInput>,
    final_output_json_schema: Option<serde_json::Value>,
  },

  /// User turn (main task execution)
  UserTurn {
    items: Vec<UserInput>,
    cwd: PathBuf,
    approval_policy: AskForApproval,
    sandbox_policy: SandboxPolicy,
    model: String,
    effort: Option<ReasoningEffortConfig>,
    summary: Option<ReasoningSummaryConfig>,
    final_output_json_schema: Option<serde_json::Value>,
    collaboration_mode: Option<CollaborationMode>,
    personality: Option<Personality>,
  },

  /// Override turn context
  OverrideTurnContext {
    cwd: Option<PathBuf>,
    approval_policy: Option<AskForApproval>,
    sandbox_policy: Option<SandboxPolicy>,
    model: Option<String>,
    collaboration_mode: Option<CollaborationMode>,
    personality: Option<Personality>,
  },

  /// Approval response for execution
  ExecApproval {
    id: String,
    turn_id: Option<String>,
    decision: ReviewDecision,
  },

  /// User input answer
  UserInputAnswer {
    id: String,
    response: RequestUserInputResponse,
  },

  /// Set thread name
  SetThreadName { name: String },

  /// Undo last N turns
  Undo { num_turns: u32 },

  /// Shutdown Cokra
  Shutdown,

  /// List available models
  ListModels,
}

/// A submitted operation with a caller-provided unique identifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submission {
  pub id: String,
  pub op: Op,
}

/// A single event emitted from core runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
  pub id: String,
  pub msg: EventMsg,
}

// ============================================================================
// TURN ITEM TYPES
// ============================================================================

/// Items shown in the turn interface
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TurnItem {
  UserMessage(UserMessageItem),
  AgentMessage(AgentMessageItem),
  Plan(PlanItem),
  Reasoning(ReasoningItem),
  WebSearch(WebSearchItem),
}

/// User message item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessageItem {
  pub id: String,
  pub content: Vec<UserInput>,
}

/// Agent message item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessageItem {
  pub id: String,
  pub content: Vec<AgentMessageContent>,
  pub phase: Option<MessagePhase>,
}

/// Agent message content types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMessageContent {
  Text { text: String },
}

/// Plan item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanItem {
  pub id: String,
  pub text: String,
}

/// Reasoning item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningItem {
  pub id: String,
  pub summary_text: Vec<String>,
  pub raw_content: Vec<String>,
}

/// Web search item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchItem {
  pub id: String,
  pub query: String,
  pub action: WebSearchAction,
}

// ============================================================================
// EVENT TYPE DEFINITIONS
// ============================================================================

/// Error event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub error: String,
  pub user_facing_message: String,
  pub details: String,
}

/// Warning event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarningEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub message: String,
}

/// Turn started event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartedEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub mode: ModeKind,
  pub model: String,
  pub start_time: i64,
}

/// Turn completed event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnCompleteEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub status: CompletionStatus,
  pub end_time: i64,
}

/// Turn aborted event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnAbortedEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub reason: String,
}

/// Completion status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompletionStatus {
  Success,
  Errored {
    error: String,
    user_facing_message: String,
    details: String,
  },
}

/// Token count event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCountEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub input_tokens: i64,
  pub cached_input_tokens: i64,
  pub output_tokens: i64,
  pub reasoning_output_tokens: i64,
  pub total_tokens: i64,
}

/// Agent message event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessageEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub item_id: String,
  pub content: Vec<AgentMessageContent>,
}

/// Agent message delta event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessageDeltaEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub item_id: String,
  pub delta: String,
}

/// Alias event used by codex-style stream consumers.
pub type AgentMessageContentDeltaEvent = AgentMessageDeltaEvent;

/// User message event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessageEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub items: Vec<UserInput>,
}

/// Session configured event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfiguredEvent {
  pub thread_id: String,
  pub model: String,
  pub approval_policy: String,
  pub sandbox_mode: String,
}

/// Thread name updated event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadNameUpdatedEvent {
  pub thread_id: String,
  pub name: String,
}

/// Exec command begin event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecCommandBeginEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub command_id: String,
  pub command: String,
  pub cwd: PathBuf,
}

/// Exec command output delta
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecCommandOutputDeltaEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub command_id: String,
  pub output: String,
}

/// Exec command end event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecCommandEndEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub command_id: String,
  pub exit_code: i32,
  pub output: String,
}

/// Exec approval request event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecApprovalRequestEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub id: String,
  pub command: String,
  pub cwd: PathBuf,
}

/// Request user input event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestUserInputEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub id: String,
  pub prompt: String,
}

/// Stream error event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamErrorEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub error: String,
}

/// Collab agent spawn begin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabAgentSpawnBeginEvent {
  pub thread_id: String,
  pub agent_id: String,
  pub role: String,
}

/// Collab agent spawn end
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabAgentSpawnEndEvent {
  pub thread_id: String,
  pub agent_id: String,
  pub status: String,
}

/// Collab agent interaction begin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabAgentInteractionBeginEvent {
  pub thread_id: String,
  pub agent_id: String,
}

/// Collab agent interaction end
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabAgentInteractionEndEvent {
  pub thread_id: String,
  pub agent_id: String,
  pub result: String,
}

/// Item started event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemStartedEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub item_id: String,
  pub item_type: String,
}

/// Item completed event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemCompletedEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub item_id: String,
  pub result: String,
}

// ============================================================================
// CORE TYPE DEFINITIONS
// ============================================================================

/// Thread identifier using UUID
#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct ThreadId {
  uuid: Uuid,
}

impl ThreadId {
  pub fn new() -> Self {
    Self {
      uuid: Uuid::new_v4(),
    }
  }

  pub fn generate() -> String {
    Uuid::new_v4().to_string()
  }

  pub fn as_uuid(&self) -> Uuid {
    self.uuid
  }
}

impl Default for ThreadId {
  fn default() -> Self {
    Self::new()
  }
}

impl std::fmt::Display for ThreadId {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", self.uuid)
  }
}

/// Turn identifier
pub type TurnId = String;

/// Session source for tracking agent hierarchy
#[derive(Debug, Clone)]
pub enum SessionSource {
  Root,
  SubAgent { depth: i32 },
}

// ============================================================================
// HELPER STRUCTS AND ENUMS
// ============================================================================

/// Base URL for models
pub const BASE_URL: &str = "https://api.openai.com/v1";

/// Default model for Cokra
pub const DEFAULT_MODEL: &str = "gpt-5.2-codex";

/// Maximum agent spawn depth
pub const MAX_THREAD_SPAWN_DEPTH: i32 = 1;

/// Mode kind enum
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModeKind {
  Default,
  Plan,
}

/// Message phase enum
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessagePhase {
  Draft,
  Final,
}

/// Web search action enum
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WebSearchAction {
  Search,
  Open,
}
