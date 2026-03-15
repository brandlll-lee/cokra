// Cokra Protocol Layer
// Complete protocol definitions for Cokra

use serde::Deserialize;
use serde::Serialize;
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

  // ========== MODEL & CONTEXT ==========
  ModelReroute(ModelRerouteEvent),
  ContextCompacted(ContextCompactedEvent),
  ThreadRolledBack(ThreadRolledBackEvent),

  // ========== CONTENT EVENTS ==========
  TokenCount(TokenCountEvent),
  AgentMessage(AgentMessageEvent),
  UserMessage(UserMessageEvent),
  AgentMessageDelta(AgentMessageDeltaEvent),
  AgentMessageContentDelta(AgentMessageContentDeltaEvent),

  // ========== AGENT REASONING ==========
  AgentReasoning(AgentReasoningEvent),
  AgentReasoningDelta(AgentReasoningDeltaEvent),
  AgentReasoningRawContent(AgentReasoningRawContentEvent),
  AgentReasoningRawContentDelta(AgentReasoningRawContentDeltaEvent),
  AgentReasoningSectionBreak(AgentReasoningSectionBreakEvent),

  // ========== CONFIGURATION EVENTS ==========
  SessionConfigured(SessionConfiguredEvent),
  ThreadNameUpdated(ThreadNameUpdatedEvent),

  // ========== MCP EVENTS ==========
  McpStartupUpdate(McpStartupUpdateEvent),
  McpStartupComplete(McpStartupCompleteEvent),
  McpToolCallBegin(McpToolCallBeginEvent),
  McpToolCallEnd(McpToolCallEndEvent),

  // ========== WEB SEARCH EVENTS ==========
  WebSearchBegin(WebSearchBeginEvent),
  WebSearchEnd(WebSearchEndEvent),

  // ========== EXECUTION EVENTS ==========
  ExecCommandBegin(ExecCommandBeginEvent),
  ExecCommandOutputDelta(ExecCommandOutputDeltaEvent),
  TerminalInteraction(TerminalInteractionEvent),
  ExecCommandEnd(ExecCommandEndEvent),

  // ========== IMAGE EVENTS ==========
  ViewImageToolCall(ViewImageToolCallEvent),

  // ========== APPROVAL EVENTS ==========
  ExecApprovalRequest(ExecApprovalRequestEvent),
  RequestUserInput(RequestUserInputEvent),
  DynamicToolCallRequest(DynamicToolCallRequestEvent),
  ElicitationRequest(ElicitationRequestEvent),
  ApplyPatchApprovalRequest(ApplyPatchApprovalRequestEvent),

  // ========== NOTICE & BACKGROUND EVENTS ==========
  DeprecationNotice(DeprecationNoticeEvent),
  BackgroundEvent(BackgroundEventEvent),

  // ========== UNDO EVENTS ==========
  UndoStarted(UndoStartedEvent),
  UndoCompleted(UndoCompletedEvent),

  // ========== STREAM & PATCH EVENTS ==========
  StreamError(StreamErrorEvent),
  PatchApplyBegin(PatchApplyBeginEvent),
  PatchApplyEnd(PatchApplyEndEvent),
  TurnDiff(TurnDiffEvent),

  // ========== QUERY/RESPONSE EVENTS ==========
  GetHistoryEntryResponse(GetHistoryEntryResponseEvent),
  McpListToolsResponse(McpListToolsResponseEvent),
  ListCustomPromptsResponse(ListCustomPromptsResponseEvent),
  ListSkillsResponse(ListSkillsResponseEvent),
  ListRemoteSkillsResponse(ListRemoteSkillsResponseEvent),
  RemoteSkillDownloaded(RemoteSkillDownloadedEvent),

  // ========== SKILLS ==========
  SkillsUpdateAvailable,

  // ========== PLAN ==========
  PlanUpdate(UpdatePlanArgs),

  // ========== TODO ==========
  TodoUpdate(TodoUpdateEvent),

  // ========== SHUTDOWN ==========
  ShutdownComplete,

  // ========== REVIEW MODE ==========
  EnteredReviewMode(ReviewRequestEvent),
  ExitedReviewMode(ExitedReviewModeEvent),

  // ========== RAW / ITEM-BASED PROTOCOL ==========
  RawResponseItem(RawResponseItemEvent),
  ItemStarted(ItemStartedEvent),
  ItemCompleted(ItemCompletedEvent),

  // ========== ITEM-BASED DELTAS ==========
  PlanDelta(PlanDeltaEvent),
  ReasoningContentDelta(ReasoningContentDeltaEvent),
  ReasoningRawContentDelta(ReasoningRawContentDeltaEvent),

  // ========== COLLABORATION EVENTS ==========
  CollabAgentSpawnBegin(CollabAgentSpawnBeginEvent),
  CollabAgentSpawnEnd(CollabAgentSpawnEndEvent),
  CollabAgentInteractionBegin(CollabAgentInteractionBeginEvent),
  CollabAgentInteractionEnd(CollabAgentInteractionEndEvent),
  CollabWaitingBegin(CollabWaitingBeginEvent),
  CollabWaitingEnd(CollabWaitingEndEvent),
  CollabCloseBegin(CollabCloseBeginEvent),
  CollabCloseEnd(CollabCloseEndEvent),
  CollabResumeBegin(CollabResumeBeginEvent),
  CollabResumeEnd(CollabResumeEndEvent),
  CollabMessagePosted(CollabMessagePostedEvent),
  CollabMessagesRead(CollabMessagesReadEvent),
  CollabTaskUpdated(CollabTaskUpdatedEvent),
  CollabTeamSnapshot(CollabTeamSnapshotEvent),
  CollabPlanSubmitted(CollabPlanSubmittedEvent),
  CollabPlanDecision(CollabPlanDecisionEvent),
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

  /// Compact current conversation context.
  Compact,

  /// Clean background terminals
  CleanBackgroundTerminals,

  /// User input (text, images, etc.)
  UserInput {
    items: Vec<UserInput>,
    final_output_json_schema: Option<serde_json::Value>,
  },

  /// Inject additional user input into the currently active turn.
  SteerInput {
    expected_turn_id: Option<TurnId>,
    items: Vec<UserInput>,
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
  pub cwd: PathBuf,
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
  pub context_window_limit: Option<usize>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub previous_model: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub model_switched_at: Option<i64>,
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
  /// The actual tool name (e.g. "shell", "read_file", "list_dir").
  pub tool_name: String,
  /// For shell: the raw command string. For other tools: same as tool_name.
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
  /// The actual tool name (e.g. "shell", "read_file", "list_dir").
  pub tool_name: String,
  pub command: String,
  pub cwd: PathBuf,
}

/// Request user input event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestUserInputEvent {
  pub thread_id: String,
  pub turn_id: String,
  #[serde(alias = "id")]
  pub call_id: String,
  pub questions: Vec<RequestUserInputQuestion>,
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
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub nickname: Option<String>,
  pub role: String,
  pub task: String,
}

/// Collab agent spawn end
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabAgentSpawnEndEvent {
  pub thread_id: String,
  pub agent_id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub nickname: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub role: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub task: Option<String>,
  pub status: AgentStatus,
}

/// Collab agent interaction begin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabAgentInteractionBeginEvent {
  pub thread_id: String,
  pub agent_id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub nickname: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub role: Option<String>,
  pub message: String,
}

/// Collab agent interaction end
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabAgentInteractionEndEvent {
  pub thread_id: String,
  pub agent_id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub nickname: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub role: Option<String>,
  pub message: String,
  pub status: AgentStatus,
}

/// Item started event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemStartedEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub item: TurnItem,
}

/// Item completed event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemCompletedEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub item: TurnItem,
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

  pub fn parse(input: &str) -> Option<Self> {
    Some(Self {
      uuid: Uuid::parse_str(input).ok()?,
    })
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

// ============================================================================
// NEW EVENT TYPES — Ported from codex protocol for 1:1 TUI parity
// ============================================================================

// ---------- Model & Context ----------

/// Model reroute event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRerouteEvent {
  pub from_model: String,
  pub to_model: String,
  pub reason: ModelRerouteReason,
}

/// Reason a model was rerouted
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModelRerouteReason {
  HighRiskCyberActivity,
}

/// Reason why context compaction happened.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextCompactionReason {
  Threshold,
  Overflow,
  Manual,
}

/// Context compacted event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextCompactedEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub reason: ContextCompactionReason,
  pub tokens_before_est: usize,
  pub tokens_after_est: usize,
  pub reserve_tokens: usize,
  pub keep_recent_tokens: usize,
}

/// Thread rolled back event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadRolledBackEvent {
  pub num_turns: u32,
}

// ---------- Agent Reasoning ----------

/// Agent reasoning event (final, complete)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentReasoningEvent {
  pub text: String,
}

/// Agent reasoning delta event (streaming)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentReasoningDeltaEvent {
  pub delta: String,
}

/// Agent reasoning raw content event (final, complete)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentReasoningRawContentEvent {
  pub text: String,
}

/// Agent reasoning raw content delta event (streaming)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentReasoningRawContentDeltaEvent {
  pub delta: String,
}

/// Agent reasoning section break event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentReasoningSectionBreakEvent {
  #[serde(default)]
  pub item_id: String,
  #[serde(default)]
  pub summary_index: i64,
}

// ---------- MCP Events ----------

/// MCP server startup status update
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupUpdateEvent {
  pub server: String,
  pub status: McpStartupStatus,
}

/// MCP startup status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpStartupStatus {
  Starting,
  Ready,
  Failed { error: String },
  Cancelled,
}

/// MCP startup complete event
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpStartupCompleteEvent {
  pub ready: Vec<String>,
  pub failed: Vec<McpStartupFailure>,
  pub cancelled: Vec<String>,
}

/// MCP startup failure info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStartupFailure {
  pub server: String,
  pub error: String,
}

/// MCP tool call begin event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpToolCallBeginEvent {
  pub call_id: String,
  pub invocation: McpInvocation,
}

/// MCP tool call end event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpToolCallEndEvent {
  pub call_id: String,
  pub invocation: McpInvocation,
  pub duration_ms: u64,
  pub result: McpToolCallResult,
}

/// MCP tool invocation details
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpInvocation {
  pub server: String,
  pub tool: String,
  pub arguments: Option<serde_json::Value>,
}

/// MCP tool call result
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum McpToolCallResult {
  Ok { content: Vec<McpContentBlock> },
  Err(String),
}

/// MCP content block
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum McpContentBlock {
  Text { text: String },
  Image { data: String, mime_type: String },
  Resource { uri: String, text: Option<String> },
}

/// MCP auth status
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum McpAuthStatus {
  Unsupported,
  NotLoggedIn,
  BearerToken,
  OAuth,
}

/// MCP tool metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
  pub name: String,
  pub description: Option<String>,
}

/// MCP resource metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
  pub uri: String,
  pub name: Option<String>,
  pub description: Option<String>,
}

/// MCP resource template metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceTemplate {
  pub uri_template: String,
  pub name: Option<String>,
  pub description: Option<String>,
}

/// MCP list tools response event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpListToolsResponseEvent {
  pub tools: std::collections::HashMap<String, Vec<McpTool>>,
  pub resources: std::collections::HashMap<String, Vec<McpResource>>,
  pub resource_templates: std::collections::HashMap<String, Vec<McpResourceTemplate>>,
  pub auth_statuses: std::collections::HashMap<String, McpAuthStatus>,
}

// ---------- Web Search Events ----------

/// Web search begin event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchBeginEvent {
  pub call_id: String,
}

/// Web search end event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchEndEvent {
  pub call_id: String,
  pub query: String,
  pub action: WebSearchAction,
}

// ---------- Terminal Interaction ----------

/// Terminal interaction event (unified exec stdin)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalInteractionEvent {
  pub call_id: String,
  pub process_id: String,
  pub stdin: String,
}

// ---------- Image Events ----------

/// View image tool call event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewImageToolCallEvent {
  pub call_id: String,
  pub path: PathBuf,
}

// ---------- Additional Approval Events ----------

/// Dynamic tool call request event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicToolCallRequestEvent {
  pub call_id: String,
  pub turn_id: String,
  pub tool: String,
  pub arguments: serde_json::Value,
}

/// Elicitation request event (MCP server prompting user)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationRequestEvent {
  pub server_name: String,
  pub id: String,
  pub message: String,
}

/// Apply patch approval request event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyPatchApprovalRequestEvent {
  pub call_id: String,
  #[serde(default)]
  pub turn_id: String,
  pub changes: std::collections::HashMap<PathBuf, FileChange>,
  pub reason: Option<String>,
  pub grant_root: Option<PathBuf>,
}

/// File change descriptor for patches
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FileChange {
  Add {
    content: String,
  },
  Delete {
    content: String,
  },
  Update {
    unified_diff: String,
    move_path: Option<PathBuf>,
  },
}

// ---------- Notice & Background Events ----------

/// Deprecation notice event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeprecationNoticeEvent {
  pub summary: String,
  pub details: Option<String>,
}

/// Background event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundEventEvent {
  pub message: String,
}

// ---------- Undo Events ----------

/// Undo started event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoStartedEvent {
  pub message: Option<String>,
}

/// Undo completed event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoCompletedEvent {
  pub success: bool,
  pub message: Option<String>,
}

// ---------- Patch Events ----------

/// Patch apply begin event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchApplyBeginEvent {
  pub call_id: String,
  #[serde(default)]
  pub turn_id: String,
  pub auto_approved: bool,
  pub changes: std::collections::HashMap<PathBuf, FileChange>,
}

/// Patch apply end event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchApplyEndEvent {
  pub call_id: String,
  #[serde(default)]
  pub turn_id: String,
  pub stdout: String,
  pub stderr: String,
  pub success: bool,
  #[serde(default)]
  pub changes: std::collections::HashMap<PathBuf, FileChange>,
  pub status: PatchApplyStatus,
}

/// Patch apply status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PatchApplyStatus {
  Completed,
  Failed,
  Declined,
}

/// Turn diff event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnDiffEvent {
  pub unified_diff: String,
}

// ---------- Query/Response Events ----------

/// History entry for replay
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
  pub events: Vec<EventMsg>,
}

/// Get history entry response event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetHistoryEntryResponseEvent {
  pub offset: usize,
  pub log_id: u64,
  pub entry: Option<HistoryEntry>,
}

/// Custom prompt metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPrompt {
  pub id: String,
  pub name: String,
  pub description: Option<String>,
}

/// List custom prompts response event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCustomPromptsResponseEvent {
  pub custom_prompts: Vec<CustomPrompt>,
}

/// Skill metadata for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
  pub name: String,
  pub description: Option<String>,
  pub path: PathBuf,
  pub enabled: bool,
}

/// Skill error info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillErrorInfo {
  pub path: PathBuf,
  pub error: String,
}

/// Skills list entry (per cwd)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsListEntry {
  pub cwd: PathBuf,
  pub skills: Vec<SkillMetadata>,
  pub errors: Vec<SkillErrorInfo>,
}

/// List skills response event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSkillsResponseEvent {
  pub skills: Vec<SkillsListEntry>,
}

/// Remote skill summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSkillSummary {
  pub id: String,
  pub name: String,
  pub description: String,
}

/// List remote skills response event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRemoteSkillsResponseEvent {
  pub skills: Vec<RemoteSkillSummary>,
}

/// Remote skill downloaded event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSkillDownloadedEvent {
  pub id: String,
  pub name: String,
  pub path: PathBuf,
}

// ---------- Plan Events ----------

/// Plan step status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepStatus {
  Pending,
  InProgress,
  Completed,
}

/// Plan item argument
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanItemArg {
  pub step: String,
  pub status: StepStatus,
}

/// Update plan arguments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePlanArgs {
  #[serde(default)]
  pub explanation: Option<String>,
  pub plan: Vec<PlanItemArg>,
}

// ---------- Todo Events ----------

/// Todo item status (1:1 opencode Todo.Info)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoItemStatus {
  Pending,
  InProgress,
  Completed,
  Cancelled,
}

/// Todo item priority
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoItemPriority {
  High,
  Medium,
  Low,
}

/// A single todo item for protocol-level transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItemEvent {
  pub id: String,
  #[serde(alias = "description")]
  pub content: String,
  pub status: TodoItemStatus,
  #[serde(default)]
  pub priority: Option<TodoItemPriority>,
}

/// Todo list update event emitted by todo_write handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoUpdateEvent {
  pub todos: Vec<TodoItemEvent>,
}

// ---------- Review Mode Events ----------

/// Review target specification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ReviewTarget {
  UncommittedChanges,
  BaseBranch { branch: String },
  Commit { sha: String, title: Option<String> },
  Custom { instructions: String },
}

/// Review request event (entering review mode)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewRequestEvent {
  pub target: ReviewTarget,
  pub user_facing_hint: Option<String>,
}

/// Review finding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewFinding {
  pub title: String,
  pub body: String,
  pub confidence_score: f32,
  pub priority: i32,
  pub code_location: ReviewCodeLocation,
}

/// Review code location
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewCodeLocation {
  pub absolute_file_path: PathBuf,
  pub line_range: ReviewLineRange,
}

/// Review line range
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewLineRange {
  pub start: u32,
  pub end: u32,
}

/// Review output event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewOutputEvent {
  pub findings: Vec<ReviewFinding>,
  pub overall_correctness: String,
  pub overall_explanation: String,
  pub overall_confidence_score: f32,
}

/// Exited review mode event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitedReviewModeEvent {
  pub review_output: Option<ReviewOutputEvent>,
}

// ---------- Raw / Item-Based Delta Events ----------

/// Raw response item event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawResponseItemEvent {
  pub item: serde_json::Value,
}

/// Plan delta event (item-based streaming)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanDeltaEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub item_id: String,
  pub delta: String,
}

/// Reasoning content delta event (item-based streaming)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningContentDeltaEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub item_id: String,
  pub delta: String,
  #[serde(default)]
  pub summary_index: i64,
}

/// Reasoning raw content delta event (item-based streaming)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningRawContentDeltaEvent {
  pub thread_id: String,
  pub turn_id: String,
  pub item_id: String,
  pub delta: String,
  #[serde(default)]
  pub content_index: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollabAgentRef {
  pub thread_id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub nickname: Option<String>,
  #[serde(default, alias = "agent_type", skip_serializing_if = "Option::is_none")]
  pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollabAgentStatusEntry {
  pub thread_id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub nickname: Option<String>,
  #[serde(default, alias = "agent_type", skip_serializing_if = "Option::is_none")]
  pub role: Option<String>,
  pub status: AgentStatus,
}

// ---------- Additional Collaboration Events ----------

/// Collab waiting begin event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabWaitingBeginEvent {
  pub sender_thread_id: String,
  pub receiver_thread_ids: Vec<String>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub receiver_agents: Vec<CollabAgentRef>,
  pub call_id: String,
}

/// Collab waiting end event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabWaitingEndEvent {
  pub sender_thread_id: String,
  pub call_id: String,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub agent_statuses: Vec<CollabAgentStatusEntry>,
  pub statuses: std::collections::HashMap<String, AgentStatus>,
}

/// Collab close begin event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabCloseBeginEvent {
  pub call_id: String,
  pub sender_thread_id: String,
  pub receiver_thread_id: String,
}

/// Collab close end event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabCloseEndEvent {
  pub call_id: String,
  pub sender_thread_id: String,
  pub receiver_thread_id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub receiver_nickname: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub receiver_role: Option<String>,
  pub status: AgentStatus,
}

/// Collab resume begin event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabResumeBeginEvent {
  pub call_id: String,
  pub sender_thread_id: String,
  pub receiver_thread_id: String,
}

/// Collab resume end event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabResumeEndEvent {
  pub call_id: String,
  pub sender_thread_id: String,
  pub receiver_thread_id: String,
  pub status: AgentStatus,
}

/// Collab team message posted event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabMessagePostedEvent {
  pub sender_thread_id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub sender_nickname: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub sender_role: Option<String>,
  pub recipient_thread_id: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub recipient_nickname: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub recipient_role: Option<String>,
  pub message: String,
}

/// Collab team messages read event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabMessagesReadEvent {
  pub reader_thread_id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub reader_nickname: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub reader_role: Option<String>,
  pub count: usize,
}

/// Collab task updated event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabTaskUpdatedEvent {
  pub actor_thread_id: String,
  pub task: TeamTask,
}

/// Collab team snapshot event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabTeamSnapshotEvent {
  pub actor_thread_id: String,
  pub snapshot: TeamSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabPlanSubmittedEvent {
  pub actor_thread_id: String,
  pub plan: TeamPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabPlanDecisionEvent {
  pub actor_thread_id: String,
  pub plan: TeamPlan,
}

// ---------- Error Info ----------

/// Detailed error classification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CokraErrorInfo {
  ContextWindowExceeded,
  UsageLimitExceeded,
  ServerOverloaded,
  HttpConnectionFailed { http_status_code: Option<u16> },
  ResponseStreamConnectionFailed { http_status_code: Option<u16> },
  InternalServerError,
  Unauthorized,
  BadRequest,
  SandboxError,
  ResponseStreamDisconnected { http_status_code: Option<u16> },
  ResponseTooManyFailedAttempts { http_status_code: Option<u16> },
  ThreadRollbackFailed,
  Other,
}

/// Rate limit error classification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RateLimitErrorClassification {
  ServerOverloaded,
  UsageLimit,
  Generic,
}

// ---------- Rate Limit Types ----------

/// Token usage info (cumulative + per-turn)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsageInfo {
  pub total_token_usage: TokenUsage,
  pub last_token_usage: TokenUsage,
  pub model_context_window: Option<i64>,
}

/// Rate limit snapshot
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RateLimitSnapshot {
  pub limit_id: Option<String>,
  pub limit_name: Option<String>,
  pub primary: Option<RateLimitWindow>,
  pub secondary: Option<RateLimitWindow>,
  pub credits: Option<CreditsSnapshot>,
  pub plan_type: Option<String>,
}

/// Rate limit window info
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RateLimitWindow {
  pub used_percent: f64,
  pub window_minutes: Option<i64>,
  pub resets_at: Option<i64>,
}

/// Credits snapshot
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreditsSnapshot {
  pub has_credits: bool,
  pub unlimited: bool,
  pub balance: Option<String>,
}

// ---------- Exec Command Source ----------

/// Source of an exec command
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ExecCommandSource {
  #[default]
  Agent,
  UserShell,
  UnifiedExecStartup,
  UnifiedExecInteraction,
}

/// Exec command status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecCommandStatus {
  Completed,
  Failed,
  Declined,
}

/// Turn abort reason
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TurnAbortReason {
  Interrupted,
  Replaced,
  ReviewEnded,
}

/// Elicitation action
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ElicitationAction {
  Accept,
  Decline,
  Cancel,
}

/// Review delivery mode
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReviewDelivery {
  Inline,
  Detached,
}
