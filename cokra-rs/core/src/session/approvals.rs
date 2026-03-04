use std::collections::HashMap;

use tokio::sync::Mutex;
use tokio::sync::oneshot;

use cokra_protocol::ReviewDecision;

#[derive(Default)]
pub struct PendingApprovals {
  state: Mutex<PendingApprovalsState>,
}

#[derive(Default)]
struct PendingApprovalsState {
  by_id: HashMap<String, oneshot::Sender<ReviewDecision>>,
  ids_by_turn: HashMap<String, Vec<String>>,
}

impl PendingApprovals {
  pub async fn insert(
    &self,
    approval_id: String,
    turn_id: String,
    tx: oneshot::Sender<ReviewDecision>,
  ) -> Option<oneshot::Sender<ReviewDecision>> {
    let mut state = self.state.lock().await;
    let previous = state.by_id.insert(approval_id.clone(), tx);

    if previous.is_some() {
      for ids in state.ids_by_turn.values_mut() {
        ids.retain(|id| id != &approval_id);
      }
    }

    state
      .ids_by_turn
      .entry(turn_id)
      .or_default()
      .push(approval_id);
    previous
  }

  pub async fn remove(&self, approval_id: &str) -> Option<oneshot::Sender<ReviewDecision>> {
    let mut state = self.state.lock().await;
    let removed = state.by_id.remove(approval_id);

    removed.as_ref()?;

    let mut empty_turns = Vec::new();
    for (turn_id, ids) in &mut state.ids_by_turn {
      ids.retain(|id| id != approval_id);
      if ids.is_empty() {
        empty_turns.push(turn_id.clone());
      }
    }
    for turn_id in empty_turns {
      state.ids_by_turn.remove(&turn_id);
    }

    removed
  }

  pub async fn clear_turn(&self, turn_id: &str) -> Vec<oneshot::Sender<ReviewDecision>> {
    let mut state = self.state.lock().await;
    let Some(ids) = state.ids_by_turn.remove(turn_id) else {
      return Vec::new();
    };

    let mut removed = Vec::new();
    for id in ids {
      if let Some(tx) = state.by_id.remove(&id) {
        removed.push(tx);
      }
    }
    removed
  }
}
