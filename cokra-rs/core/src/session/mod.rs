mod approvals;
mod user_input;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::compaction::estimate_message_tokens;
use crate::compaction::find_safe_tail_start_index;
use crate::compaction::first_non_system_index;
use crate::model::Message;
use crate::model::Usage;
use crate::shell::Shell;
use crate::turn::response_items::ResponseItem;
use approvals::PendingApprovals;
use cokra_protocol::EventMsg;
use cokra_protocol::ExecApprovalRequestEvent;
use cokra_protocol::RequestUserInputEvent;
use cokra_protocol::ReviewDecision;
use cokra_protocol::TurnId;
use cokra_protocol::UserInput;
use cokra_protocol::user_input::RequestUserInputResponse;
use user_input::PendingUserInputs;

/// Runtime session state for one conversation thread.
///
/// Spec 3.2: Session caches the resolved user shell so that
/// `default_user_shell()` is called once at init, not per-tool-call.
pub struct Session {
  session_id: String,
  thread_id: cokra_protocol::ThreadId,
  history: Arc<RwLock<Vec<Message>>>,
  response_history: Arc<RwLock<Vec<ResponseItem>>>,
  event_tx: broadcast::Sender<cokra_protocol::EventMsg>,
  pending_approvals: Arc<PendingApprovals>,
  pending_user_inputs: Arc<PendingUserInputs>,
  active_turn_state: Arc<RwLock<ActiveTurnState>>,
  /// Spec 3.2: cached user shell, resolved once at session creation.
  cached_shell: Arc<RwLock<Shell>>,
  token_usage: Arc<RwLock<TokenUsageState>>,
  model_switch_state: Arc<RwLock<ModelSwitchState>>,
}

#[derive(Debug, Clone, Default)]
pub struct TokenUsageState {
  pub input_tokens: u64,
  pub output_tokens: u64,
  pub total_tokens: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelSwitchState {
  pub current_model: Option<String>,
  pub previous_model: Option<String>,
  pub switched_at: Option<i64>,
}

#[derive(Debug, Default)]
struct ActiveTurnState {
  turn_id: Option<TurnId>,
  pending_inputs: VecDeque<Vec<UserInput>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SteerInputError {
  NoActiveTurn,
  TurnMismatch {
    expected_turn_id: TurnId,
    active_turn_id: TurnId,
  },
}

impl Session {
  pub fn new() -> Self {
    Self::new_with_thread_id(cokra_protocol::ThreadId::new())
  }

  pub fn new_with_thread_id(thread_id: cokra_protocol::ThreadId) -> Self {
    let (event_tx, _event_rx) = broadcast::channel(512);
    let shell = crate::shell::default_user_shell();
    Self {
      session_id: uuid::Uuid::new_v4().to_string(),
      thread_id,
      history: Arc::new(RwLock::new(Vec::new())),
      response_history: Arc::new(RwLock::new(Vec::new())),
      event_tx,
      pending_approvals: Arc::new(PendingApprovals::default()),
      pending_user_inputs: Arc::new(PendingUserInputs::default()),
      active_turn_state: Arc::new(RwLock::new(ActiveTurnState::default())),
      cached_shell: Arc::new(RwLock::new(shell)),
      token_usage: Arc::new(RwLock::new(TokenUsageState::default())),
      model_switch_state: Arc::new(RwLock::new(ModelSwitchState::default())),
    }
  }

  /// Spec 3.2: get the session-cached user shell.
  pub async fn user_shell(&self) -> Shell {
    self.cached_shell.read().await.clone()
  }

  /// Spec 3.2: reset the cached shell (e.g. after config/model change).
  pub async fn reset_user_shell(&self) {
    let new_shell = crate::shell::default_user_shell();
    *self.cached_shell.write().await = new_shell;
  }

  pub fn id(&self) -> Option<String> {
    Some(self.session_id.clone())
  }

  pub async fn get_history(&self, limit: usize) -> Vec<Message> {
    let history = self.history.read().await;
    if history.len() <= limit {
      return history.clone();
    }
    history[history.len() - limit..].to_vec()
  }

  /// Return history constrained by an estimated token budget.
  ///
  /// Strategy: always keep all `System` messages, then walk non-system messages
  /// from newest to oldest until budget is exhausted.
  pub async fn get_history_for_prompt(&self, max_tokens: usize) -> Vec<Message> {
    let history = self.history.read().await;
    if max_tokens == 0 {
      return history
        .iter()
        .filter(|msg| matches!(msg, Message::System(_)))
        .cloned()
        .collect();
    }

    let mut systems = history
      .iter()
      .filter(|msg| matches!(msg, Message::System(_)))
      .cloned()
      .collect::<Vec<_>>();
    let Some(boundary_start) = first_non_system_index(&history) else {
      return systems;
    };
    let first_kept_index = find_safe_tail_start_index(&history, boundary_start, max_tokens, true)
      .unwrap_or(boundary_start);

    systems.extend(
      history[first_kept_index..]
        .iter()
        .filter(|msg| !matches!(msg, Message::System(_)))
        .cloned(),
    );
    systems
  }

  pub async fn append_message(&self, msg: Message) {
    self.history.write().await.push(msg);
  }

  pub async fn append_messages(&self, msgs: Vec<Message>) {
    self.history.write().await.extend(msgs);
  }

  pub async fn append_response_item(&self, item: ResponseItem) {
    self.response_history.write().await.push(item);
  }

  pub async fn append_response_items(&self, items: Vec<ResponseItem>) {
    self.response_history.write().await.extend(items);
  }

  pub async fn clone_response_history(&self) -> Vec<ResponseItem> {
    self.response_history.read().await.clone()
  }

  pub async fn clone_history(&self) -> Vec<Message> {
    self.history.read().await.clone()
  }

  pub async fn replace_history(&self, messages: Vec<Message>) {
    *self.history.write().await = messages;
  }

  pub(crate) async fn begin_turn(&self, turn_id: TurnId) {
    let mut active_turn = self.active_turn_state.write().await;
    if active_turn.turn_id.as_deref() == Some(turn_id.as_str()) {
      return;
    }
    active_turn.turn_id = Some(turn_id);
    active_turn.pending_inputs.clear();
  }

  pub(crate) async fn end_turn(&self, turn_id: &str) {
    let mut active_turn = self.active_turn_state.write().await;
    if active_turn.turn_id.as_deref() == Some(turn_id) {
      active_turn.turn_id = None;
      active_turn.pending_inputs.clear();
    }
  }

  pub(crate) async fn active_turn_id(&self) -> Option<TurnId> {
    self.active_turn_state.read().await.turn_id.clone()
  }

  pub(crate) async fn steer_input(
    &self,
    expected_turn_id: Option<&str>,
    items: Vec<UserInput>,
  ) -> Result<(), SteerInputError> {
    let mut active_turn = self.active_turn_state.write().await;
    let Some(active_turn_id) = active_turn.turn_id.clone() else {
      return Err(SteerInputError::NoActiveTurn);
    };
    if let Some(expected_turn_id) = expected_turn_id
      && expected_turn_id != active_turn_id.as_str()
    {
      return Err(SteerInputError::TurnMismatch {
        expected_turn_id: expected_turn_id.to_string(),
        active_turn_id,
      });
    }
    active_turn.pending_inputs.push_back(items);
    Ok(())
  }

  pub(crate) async fn take_pending_inputs(&self) -> Vec<Vec<UserInput>> {
    let mut active_turn = self.active_turn_state.write().await;
    active_turn.pending_inputs.drain(..).collect()
  }

  pub async fn update_token_usage(&self, usage: &Usage) {
    let mut token_usage = self.token_usage.write().await;
    token_usage.input_tokens += usage.input_tokens as u64;
    token_usage.output_tokens += usage.output_tokens as u64;
    token_usage.total_tokens += usage.total_tokens as u64;
  }

  pub async fn set_token_usage(&self, usage: &Usage) {
    let mut token_usage = self.token_usage.write().await;
    token_usage.input_tokens = usage.input_tokens as u64;
    token_usage.output_tokens = usage.output_tokens as u64;
    token_usage.total_tokens = usage.total_tokens as u64;
  }

  pub async fn get_total_token_usage(&self) -> u64 {
    self.token_usage.read().await.total_tokens
  }

  pub async fn track_model_selection(&self, model: impl Into<String>) -> ModelSwitchState {
    let model = model.into();
    let mut state = self.model_switch_state.write().await;
    match state.current_model.as_deref() {
      None => {
        state.current_model = Some(model);
        state.previous_model = None;
        state.switched_at = None;
      }
      Some(current) if current == model => {}
      Some(_) => {
        state.previous_model = state.current_model.clone();
        state.current_model = Some(model);
        state.switched_at = Some(chrono::Utc::now().timestamp());
      }
    }
    state.clone()
  }

  pub async fn model_switch_state(&self) -> ModelSwitchState {
    self.model_switch_state.read().await.clone()
  }

  /// Drop oldest non-system messages until usage is below `target_total_tokens`.
  pub async fn compact_history_to_token_target(&self, target_total_tokens: usize) {
    let mut history = self.history.write().await;
    let system_tokens = history
      .iter()
      .filter(|msg| matches!(msg, Message::System(_)))
      .map(estimate_message_tokens)
      .sum::<usize>();
    let Some(boundary_start) = first_non_system_index(&history) else {
      return;
    };

    let allowed_non_system_tokens = target_total_tokens.saturating_sub(system_tokens);
    if allowed_non_system_tokens == 0 {
      history.retain(|msg| matches!(msg, Message::System(_)));
      return;
    }

    let Some(first_kept_index) =
      find_safe_tail_start_index(&history, boundary_start, allowed_non_system_tokens, true)
    else {
      return;
    };
    if first_kept_index <= boundary_start {
      return;
    }

    let systems = history
      .iter()
      .filter(|msg| matches!(msg, Message::System(_)))
      .cloned()
      .collect::<Vec<_>>();
    let kept = history[first_kept_index..]
      .iter()
      .filter(|msg| !matches!(msg, Message::System(_)))
      .cloned()
      .collect::<Vec<_>>();
    history.clear();
    history.extend(systems);
    history.extend(kept);
  }

  pub fn subscribe_events(&self) -> broadcast::Receiver<cokra_protocol::EventMsg> {
    self.event_tx.subscribe()
  }

  pub fn emit_event(&self, event: cokra_protocol::EventMsg) {
    let _ = self.event_tx.send(event);
  }

  pub async fn insert_pending_approval(
    &self,
    approval_id: String,
    turn_id: String,
    tx: oneshot::Sender<ReviewDecision>,
  ) -> Option<oneshot::Sender<ReviewDecision>> {
    self
      .pending_approvals
      .insert(approval_id, turn_id, tx)
      .await
  }

  pub async fn remove_pending_approval(
    &self,
    approval_id: &str,
  ) -> Option<oneshot::Sender<ReviewDecision>> {
    self.pending_approvals.remove(approval_id).await
  }

  pub async fn clear_pending_approvals_for_turn(&self, turn_id: &str) -> usize {
    let pending = self.pending_approvals.clear_turn(turn_id).await;
    let total = pending.len();
    for tx in pending {
      let _ = tx.send(ReviewDecision::Denied);
    }
    total
  }

  pub async fn insert_pending_user_input(
    &self,
    request_id: String,
    turn_id: String,
    tx: oneshot::Sender<RequestUserInputResponse>,
  ) -> Option<oneshot::Sender<RequestUserInputResponse>> {
    self
      .pending_user_inputs
      .insert(request_id, turn_id, tx)
      .await
  }

  pub async fn remove_pending_user_input(
    &self,
    request_id: &str,
  ) -> Option<oneshot::Sender<RequestUserInputResponse>> {
    self.pending_user_inputs.remove(request_id).await
  }

  pub async fn clear_pending_user_inputs_for_turn(&self, turn_id: &str) -> usize {
    self.pending_user_inputs.clear_turn(turn_id).await.len()
  }

  pub async fn emit_exec_approval_request(
    &self,
    thread_id: String,
    turn_id: String,
    approval_id: String,
    tool_name: String,
    command: String,
    cwd: PathBuf,
    tx_event: Option<mpsc::Sender<EventMsg>>,
  ) {
    let event = EventMsg::ExecApprovalRequest(ExecApprovalRequestEvent {
      thread_id,
      turn_id,
      id: approval_id,
      tool_name,
      command,
      cwd,
    });
    self.emit_event(event.clone());
    if let Some(tx_event) = tx_event {
      let _ = tx_event.send(event).await;
    }
  }

  pub async fn emit_request_user_input(
    &self,
    thread_id: String,
    turn_id: String,
    call_id: String,
    questions: Vec<cokra_protocol::RequestUserInputQuestion>,
    tx_event: Option<mpsc::Sender<EventMsg>>,
  ) {
    let event = EventMsg::RequestUserInput(RequestUserInputEvent {
      thread_id,
      turn_id,
      call_id,
      questions,
    });
    self.emit_event(event.clone());
    if let Some(tx_event) = tx_event {
      let _ = tx_event.send(event).await;
    }
  }

  pub async fn request_exec_approval(
    &self,
    thread_id: String,
    turn_id: String,
    approval_id: String,
    tool_name: String,
    command: String,
    cwd: PathBuf,
    tx_event: Option<mpsc::Sender<EventMsg>>,
  ) -> ReviewDecision {
    let (tx, rx) = oneshot::channel();
    let previous = self
      .insert_pending_approval(approval_id.clone(), turn_id.clone(), tx)
      .await;
    if let Some(previous) = previous {
      tracing::warn!("overwriting existing pending approval for id: {approval_id}");
      let _ = previous.send(ReviewDecision::Denied);
    }

    self
      .emit_exec_approval_request(
        thread_id,
        turn_id,
        approval_id,
        tool_name,
        command,
        cwd,
        tx_event,
      )
      .await;

    match rx.await {
      Ok(decision) => decision,
      Err(_) => ReviewDecision::Denied,
    }
  }

  pub async fn notify_exec_approval(&self, approval_id: &str, decision: ReviewDecision) -> bool {
    let Some(tx) = self.remove_pending_approval(approval_id).await else {
      return false;
    };
    let _ = tx.send(decision);
    true
  }

  pub async fn request_user_input(
    &self,
    thread_id: String,
    turn_id: String,
    request_id: String,
    call_id: String,
    questions: Vec<cokra_protocol::RequestUserInputQuestion>,
    tx_event: Option<mpsc::Sender<EventMsg>>,
  ) -> Option<RequestUserInputResponse> {
    let (tx, rx) = oneshot::channel();
    let previous = self
      .insert_pending_user_input(request_id.clone(), turn_id.clone(), tx)
      .await;
    if previous.is_some() {
      tracing::warn!("overwriting existing pending user input for id: {request_id}");
    }

    self
      .emit_request_user_input(thread_id, turn_id, call_id, questions, tx_event)
      .await;

    rx.await.ok()
  }

  pub async fn notify_user_input(
    &self,
    request_id: &str,
    response: RequestUserInputResponse,
  ) -> bool {
    let Some(tx) = self.remove_pending_user_input(request_id).await else {
      return false;
    };
    let _ = tx.send(response);
    true
  }

  pub fn thread_id(&self) -> Option<&cokra_protocol::ThreadId> {
    Some(&self.thread_id)
  }

  pub async fn shutdown(&self) -> anyhow::Result<()> {
    self.emit_event(cokra_protocol::EventMsg::ShutdownComplete);
    Ok(())
  }
}

impl Default for Session {
  fn default() -> Self {
    Self::new()
  }
}

#[cfg(test)]
mod tests {
  use tokio::sync::oneshot;

  use super::Session;
  use super::SteerInputError;
  use crate::model::Message;
  use cokra_protocol::ReviewDecision;
  use cokra_protocol::UserInput;
  use cokra_protocol::user_input::RequestUserInputResponse;

  #[tokio::test]
  async fn pending_approval_insert_notify_round_trip() {
    let session = Session::new();
    let (tx, rx) = oneshot::channel();
    let previous = session
      .insert_pending_approval("approval-1".to_string(), "turn-1".to_string(), tx)
      .await;
    assert!(previous.is_none());

    let notified = session
      .notify_exec_approval("approval-1", ReviewDecision::Approved)
      .await;
    assert!(notified);
    assert!(matches!(rx.await, Ok(ReviewDecision::Approved)));
  }

  #[tokio::test]
  async fn notifying_missing_approval_returns_false() {
    let session = Session::new();
    let notified = session
      .notify_exec_approval("does-not-exist", ReviewDecision::Denied)
      .await;
    assert!(!notified);
  }

  #[tokio::test]
  async fn pending_user_input_insert_notify_round_trip() {
    let session = Session::new();
    let (tx, rx) = oneshot::channel();
    let previous = session
      .insert_pending_user_input("input-1".to_string(), "turn-1".to_string(), tx)
      .await;
    assert!(previous.is_none());

    let notified = session
      .notify_user_input(
        "input-1",
        RequestUserInputResponse {
          answers: std::collections::HashMap::from([(
            "q1".to_string(),
            cokra_protocol::RequestUserInputAnswer {
              answers: vec!["hello".to_string()],
            },
          )]),
        },
      )
      .await;
    assert!(notified);
    assert!(matches!(
      rx.await,
      Ok(RequestUserInputResponse { answers })
        if answers.get("q1").is_some_and(|answer| answer.answers == vec!["hello".to_string()])
    ));
  }

  #[tokio::test]
  async fn clear_turn_denies_pending_waiters() {
    let session = Session::new();
    let (tx1, rx1) = oneshot::channel();
    let (tx2, rx2) = oneshot::channel();
    let (tx3, rx3) = oneshot::channel();

    session
      .insert_pending_approval("a1".to_string(), "turn-a".to_string(), tx1)
      .await;
    session
      .insert_pending_approval("a2".to_string(), "turn-a".to_string(), tx2)
      .await;
    session
      .insert_pending_approval("b1".to_string(), "turn-b".to_string(), tx3)
      .await;

    let cleared = session.clear_pending_approvals_for_turn("turn-a").await;
    assert_eq!(cleared, 2);
    assert!(matches!(rx1.await, Ok(ReviewDecision::Denied)));
    assert!(matches!(rx2.await, Ok(ReviewDecision::Denied)));

    let notified = session
      .notify_exec_approval("b1", ReviewDecision::Approved)
      .await;
    assert!(notified);
    assert!(matches!(rx3.await, Ok(ReviewDecision::Approved)));
  }

  #[tokio::test]
  async fn clear_turn_drops_pending_user_inputs() {
    let session = Session::new();
    let (tx1, rx1) = oneshot::channel();
    let (tx2, rx2) = oneshot::channel();
    let (tx3, rx3) = oneshot::channel();

    session
      .insert_pending_user_input("u1".to_string(), "turn-a".to_string(), tx1)
      .await;
    session
      .insert_pending_user_input("u2".to_string(), "turn-a".to_string(), tx2)
      .await;
    session
      .insert_pending_user_input("u3".to_string(), "turn-b".to_string(), tx3)
      .await;

    let cleared = session.clear_pending_user_inputs_for_turn("turn-a").await;
    assert_eq!(cleared, 2);
    assert!(rx1.await.is_err());
    assert!(rx2.await.is_err());

    let notified = session
      .notify_user_input(
        "u3",
        RequestUserInputResponse {
          answers: std::collections::HashMap::from([(
            "q1".to_string(),
            cokra_protocol::RequestUserInputAnswer {
              answers: vec!["kept".to_string()],
            },
          )]),
        },
      )
      .await;
    assert!(notified);
    assert!(matches!(
      rx3.await,
      Ok(RequestUserInputResponse { answers })
        if answers.get("q1").is_some_and(|answer| answer.answers == vec!["kept".to_string()])
    ));
  }

  #[tokio::test]
  async fn steer_input_round_trip_uses_active_turn_queue() {
    let session = Session::new();
    let queued_input = UserInput::Text {
      text: "follow-up".to_string(),
      text_elements: Vec::new(),
    };

    session.begin_turn("turn-1".to_string()).await;
    session
      .steer_input(None, vec![queued_input.clone()])
      .await
      .expect("steer input should queue while turn is active");

    assert_eq!(session.active_turn_id().await, Some("turn-1".to_string()));
    assert_eq!(
      session.take_pending_inputs().await,
      vec![vec![queued_input]]
    );

    session.end_turn("turn-1").await;
    assert_eq!(session.active_turn_id().await, None);
    assert!(session.take_pending_inputs().await.is_empty());
  }

  #[tokio::test]
  async fn steer_input_rejects_turn_mismatch() {
    let session = Session::new();
    session.begin_turn("turn-1".to_string()).await;

    let result = session
      .steer_input(
        Some("turn-2"),
        vec![UserInput::Text {
          text: "ignored".to_string(),
          text_elements: Vec::new(),
        }],
      )
      .await;

    assert_eq!(
      result,
      Err(SteerInputError::TurnMismatch {
        expected_turn_id: "turn-2".to_string(),
        active_turn_id: "turn-1".to_string(),
      })
    );
    assert!(session.take_pending_inputs().await.is_empty());
  }

  #[tokio::test]
  async fn get_history_for_prompt_keeps_system_and_recent_messages() {
    let session = Session::new();
    session
      .append_messages(vec![
        Message::System("sys".to_string()),
        Message::User("old message that should drop".to_string()),
        Message::Assistant {
          content: Some("recent".to_string()),
          tool_calls: None,
        },
      ])
      .await;

    let selected = session.get_history_for_prompt(3).await;
    assert!(matches!(selected.first(), Some(Message::System(_))));
    assert!(selected.iter().any(|m| match m {
      Message::Assistant { content, .. } => content.as_deref() == Some("recent"),
      _ => false,
    }));
    assert!(!selected.iter().any(|m| match m {
      Message::User(text) => text == "old message that should drop",
      _ => false,
    }));
  }

  #[tokio::test]
  async fn compact_history_to_token_target_drops_oldest_non_system() {
    let session = Session::new();
    session
      .append_messages(vec![
        Message::System("sys".to_string()),
        Message::User("1111111111".to_string()),
        Message::User("2222222222".to_string()),
      ])
      .await;

    session.compact_history_to_token_target(4).await;
    let history = session.get_history(10).await;

    assert!(matches!(history.first(), Some(Message::System(_))));
    assert_eq!(
      history
        .iter()
        .filter(|m| matches!(m, Message::User(_)))
        .count(),
      1
    );
  }

  #[tokio::test]
  async fn get_history_for_prompt_never_starts_with_orphaned_tool_result() {
    let session = Session::new();
    session
      .append_messages(vec![
        Message::Assistant {
          content: Some("calling tool".to_string()),
          tool_calls: Some(vec![crate::model::ToolCall {
            id: "call-1".to_string(),
            call_type: "function".to_string(),
            function: crate::model::ToolCallFunction {
              name: "read_file".to_string(),
              arguments: r#"{"file_path":"src/lib.rs"}"#.to_string(),
            },
            provider_meta: None,
          }]),
        },
        Message::Tool {
          tool_call_id: "call-1".to_string(),
          content: "content".to_string(),
        },
        Message::Assistant {
          content: Some("done".to_string()),
          tool_calls: None,
        },
      ])
      .await;

    let selected = session.get_history_for_prompt(2).await;
    assert!(!matches!(selected.first(), Some(Message::Tool { .. })));
    for (index, message) in selected.iter().enumerate() {
      let Message::Tool { tool_call_id, .. } = message else {
        continue;
      };
      assert!(matches!(
        selected.get(index.saturating_sub(1)),
        Some(Message::Assistant {
          tool_calls: Some(tool_calls),
          ..
        }) if index > 0 && tool_calls.iter().any(|call| call.id == *tool_call_id)
      ));
    }
  }
}
