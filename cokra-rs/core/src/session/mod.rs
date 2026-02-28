use std::sync::Arc;

use tokio::sync::{RwLock, broadcast};

use crate::model::Message;

/// Runtime session state for one conversation thread.
pub struct Session {
  session_id: String,
  thread_id: cokra_protocol::ThreadId,
  history: Arc<RwLock<Vec<Message>>>,
  event_tx: broadcast::Sender<cokra_protocol::EventMsg>,
}

impl Session {
  pub fn new() -> Self {
    let (event_tx, _event_rx) = broadcast::channel(512);
    Self {
      session_id: uuid::Uuid::new_v4().to_string(),
      thread_id: cokra_protocol::ThreadId::new(),
      history: Arc::new(RwLock::new(Vec::new())),
      event_tx,
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
