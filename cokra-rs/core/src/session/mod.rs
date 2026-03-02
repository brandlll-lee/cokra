mod approvals;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{RwLock, broadcast, mpsc, oneshot};

use crate::model::Message;
use approvals::PendingApprovals;
use cokra_protocol::{EventMsg, ExecApprovalRequestEvent, ReviewDecision};

/// Runtime session state for one conversation thread.
pub struct Session {
  session_id: String,
  thread_id: cokra_protocol::ThreadId,
  history: Arc<RwLock<Vec<Message>>>,
  event_tx: broadcast::Sender<cokra_protocol::EventMsg>,
  pending_approvals: Arc<PendingApprovals>,
}

impl Session {
  pub fn new() -> Self {
    let (event_tx, _event_rx) = broadcast::channel(512);
    Self {
      session_id: uuid::Uuid::new_v4().to_string(),
      thread_id: cokra_protocol::ThreadId::new(),
      history: Arc::new(RwLock::new(Vec::new())),
      event_tx,
      pending_approvals: Arc::new(PendingApprovals::default()),
    }
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

  pub async fn append_message(&self, msg: Message) {
    self.history.write().await.push(msg);
  }

  pub async fn append_messages(&self, msgs: Vec<Message>) {
    self.history.write().await.extend(msgs);
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

  pub async fn emit_exec_approval_request(
    &self,
    thread_id: String,
    turn_id: String,
    approval_id: String,
    command: String,
    cwd: PathBuf,
    tx_event: Option<mpsc::Sender<EventMsg>>,
  ) {
    let event = EventMsg::ExecApprovalRequest(ExecApprovalRequestEvent {
      thread_id,
      turn_id,
      id: approval_id,
      command,
      cwd,
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
      .emit_exec_approval_request(thread_id, turn_id, approval_id, command, cwd, tx_event)
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
  use cokra_protocol::ReviewDecision;

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
}
