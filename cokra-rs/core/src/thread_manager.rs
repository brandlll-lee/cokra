use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use chrono::Utc;
use tokio::sync::broadcast;

use cokra_protocol::ThreadId;

const THREAD_CREATED_CHANNEL_CAPACITY: usize = 128;

#[derive(Debug, Clone)]
pub struct ThreadInfo {
  pub thread_id: ThreadId,
  pub parent_thread_id: Option<ThreadId>,
  pub depth: usize,
  pub role: String,
  pub task: String,
  pub created_at: i64,
}

/// Shared state container used by `AgentControl`.
pub struct ThreadManagerState {
  threads: Mutex<HashMap<ThreadId, ThreadInfo>>,
  thread_created_tx: broadcast::Sender<ThreadId>,
}

impl ThreadManagerState {
  fn new(root_thread_id: ThreadId) -> Self {
    let (thread_created_tx, _) = broadcast::channel(THREAD_CREATED_CHANNEL_CAPACITY);
    let mut threads = HashMap::new();
    threads.insert(
      root_thread_id.clone(),
      ThreadInfo {
        thread_id: root_thread_id,
        parent_thread_id: None,
        depth: 0,
        role: "root".to_string(),
        task: "root session".to_string(),
        created_at: Utc::now().timestamp(),
      },
    );

    Self {
      threads: Mutex::new(threads),
      thread_created_tx,
    }
  }

  pub fn spawn_thread(
    &self,
    parent_thread_id: ThreadId,
    depth: usize,
    role: String,
    task: String,
  ) -> ThreadId {
    let thread_id = ThreadId::new();
    let info = ThreadInfo {
      thread_id: thread_id.clone(),
      parent_thread_id: Some(parent_thread_id),
      depth,
      role,
      task,
      created_at: Utc::now().timestamp(),
    };

    self
      .threads
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .insert(thread_id.clone(), info);
    thread_id
  }

  pub fn remove_thread(&self, thread_id: &ThreadId) -> bool {
    self
      .threads
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .remove(thread_id)
      .is_some()
  }

  pub fn get_thread(&self, thread_id: &ThreadId) -> Option<ThreadInfo> {
    self
      .threads
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .get(thread_id)
      .cloned()
  }

  pub fn list_thread_ids(&self) -> Vec<ThreadId> {
    self
      .threads
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .keys()
      .cloned()
      .collect()
  }

  pub fn subscribe_thread_created(&self) -> broadcast::Receiver<ThreadId> {
    self.thread_created_tx.subscribe()
  }

  pub fn notify_thread_created(&self, thread_id: ThreadId) {
    let _ = self.thread_created_tx.send(thread_id);
  }
}

/// Minimal thread registry for phase 1 multi-agent support.
pub struct ThreadManager {
  state: Arc<ThreadManagerState>,
}

impl ThreadManager {
  pub fn new(root_thread_id: ThreadId) -> Self {
    Self {
      state: Arc::new(ThreadManagerState::new(root_thread_id)),
    }
  }

  pub fn state(&self) -> Arc<ThreadManagerState> {
    Arc::clone(&self.state)
  }

  pub fn downgrade_state(&self) -> Weak<ThreadManagerState> {
    Arc::downgrade(&self.state)
  }

  pub fn subscribe_thread_created(&self) -> broadcast::Receiver<ThreadId> {
    self.state.subscribe_thread_created()
  }

  pub fn list_thread_ids(&self) -> Vec<ThreadId> {
    self.state.list_thread_ids()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn manager_registers_root_and_spawned_threads() {
    let root = ThreadId::new();
    let manager = ThreadManager::new(root.clone());
    assert!(manager.list_thread_ids().contains(&root));

    let child =
      manager
        .state()
        .spawn_thread(root, 1, "explorer".to_string(), "read files".to_string());

    let ids = manager.list_thread_ids();
    assert!(ids.contains(&child));
    assert_eq!(ids.len(), 2);
  }
}
