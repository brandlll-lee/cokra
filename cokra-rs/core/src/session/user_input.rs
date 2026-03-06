use std::collections::HashMap;

use tokio::sync::Mutex;
use tokio::sync::oneshot;

use cokra_protocol::user_input::RequestUserInputResponse;

#[derive(Default)]
pub struct PendingUserInputs {
  state: Mutex<PendingUserInputsState>,
}

#[derive(Default)]
struct PendingUserInputsState {
  by_id: HashMap<String, oneshot::Sender<RequestUserInputResponse>>,
  ids_by_turn: HashMap<String, Vec<String>>,
}

impl PendingUserInputs {
  pub async fn insert(
    &self,
    request_id: String,
    turn_id: String,
    tx: oneshot::Sender<RequestUserInputResponse>,
  ) -> Option<oneshot::Sender<RequestUserInputResponse>> {
    let mut state = self.state.lock().await;
    let previous = state.by_id.insert(request_id.clone(), tx);

    if previous.is_some() {
      for ids in state.ids_by_turn.values_mut() {
        ids.retain(|id| id != &request_id);
      }
    }

    state
      .ids_by_turn
      .entry(turn_id)
      .or_default()
      .push(request_id);
    previous
  }

  pub async fn remove(
    &self,
    request_id: &str,
  ) -> Option<oneshot::Sender<RequestUserInputResponse>> {
    let mut state = self.state.lock().await;
    let removed = state.by_id.remove(request_id);

    removed.as_ref()?;

    let mut empty_turns = Vec::new();
    for (turn_id, ids) in &mut state.ids_by_turn {
      ids.retain(|id| id != request_id);
      if ids.is_empty() {
        empty_turns.push(turn_id.clone());
      }
    }
    for turn_id in empty_turns {
      state.ids_by_turn.remove(&turn_id);
    }

    removed
  }

  pub async fn clear_turn(&self, turn_id: &str) -> Vec<oneshot::Sender<RequestUserInputResponse>> {
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
