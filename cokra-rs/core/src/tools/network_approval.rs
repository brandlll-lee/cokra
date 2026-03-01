use std::collections::HashMap;
use std::sync::OnceLock;

use tokio::sync::Mutex;
use uuid::Uuid;

use crate::session::Session;
use crate::tools::sandboxing::ToolError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkApprovalMode {
  Immediate,
  Deferred,
}

#[derive(Clone, Debug)]
pub struct NetworkApprovalSpec {
  pub mode: NetworkApprovalMode,
}

#[derive(Clone, Debug)]
pub struct DeferredNetworkApproval {
  attempt_id: String,
}

impl DeferredNetworkApproval {
  pub fn attempt_id(&self) -> &str {
    &self.attempt_id
  }
}

#[derive(Debug)]
pub struct ActiveNetworkApproval {
  attempt_id: Option<String>,
  mode: NetworkApprovalMode,
}

impl ActiveNetworkApproval {
  pub fn attempt_id(&self) -> Option<&str> {
    self.attempt_id.as_deref()
  }

  pub fn mode(&self) -> NetworkApprovalMode {
    self.mode
  }

  pub fn into_deferred(self) -> Option<DeferredNetworkApproval> {
    match (self.mode, self.attempt_id) {
      (NetworkApprovalMode::Deferred, Some(attempt_id)) => {
        Some(DeferredNetworkApproval { attempt_id })
      }
      _ => None,
    }
  }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NetworkApprovalOutcome {
  DeniedByUser,
  DeniedByPolicy(String),
}

#[derive(Default)]
struct NetworkApprovalService {
  attempts: Mutex<HashMap<String, Option<NetworkApprovalOutcome>>>,
}

impl NetworkApprovalService {
  async fn register_attempt(&self, attempt_id: String) {
    self.attempts.lock().await.insert(attempt_id, None);
  }

  async fn unregister_attempt(&self, attempt_id: &str) {
    self.attempts.lock().await.remove(attempt_id);
  }

  async fn set_outcome(&self, attempt_id: &str, outcome: NetworkApprovalOutcome) {
    if let Some(slot) = self.attempts.lock().await.get_mut(attempt_id) {
      *slot = Some(outcome);
    }
  }

  async fn take_outcome(&self, attempt_id: &str) -> Option<NetworkApprovalOutcome> {
    let mut attempts = self.attempts.lock().await;
    attempts.get_mut(attempt_id).and_then(Option::take)
  }
}

fn service() -> &'static NetworkApprovalService {
  static SERVICE: OnceLock<NetworkApprovalService> = OnceLock::new();
  SERVICE.get_or_init(NetworkApprovalService::default)
}

pub async fn begin_network_approval(
  _session: &Session,
  _turn_id: &str,
  _call_id: &str,
  has_managed_network_requirements: bool,
  spec: Option<NetworkApprovalSpec>,
) -> Option<ActiveNetworkApproval> {
  let spec = spec?;
  if !has_managed_network_requirements {
    return None;
  }

  let attempt_id = Uuid::new_v4().to_string();
  service().register_attempt(attempt_id.clone()).await;

  Some(ActiveNetworkApproval {
    attempt_id: Some(attempt_id),
    mode: spec.mode,
  })
}

pub async fn record_network_approval_outcome(attempt_id: &str, outcome: NetworkApprovalOutcome) {
  service().set_outcome(attempt_id, outcome).await;
}

pub async fn is_network_approval_attempt_active(attempt_id: &str) -> bool {
  service().attempts.lock().await.contains_key(attempt_id)
}

pub async fn finish_immediate_network_approval(
  _session: &Session,
  active: ActiveNetworkApproval,
) -> Result<(), ToolError> {
  let Some(attempt_id) = active.attempt_id() else {
    return Ok(());
  };

  let outcome = service().take_outcome(attempt_id).await;
  service().unregister_attempt(attempt_id).await;

  match outcome {
    Some(NetworkApprovalOutcome::DeniedByUser) => {
      Err(ToolError::Rejected("rejected by user".to_string()))
    }
    Some(NetworkApprovalOutcome::DeniedByPolicy(message)) => Err(ToolError::Rejected(message)),
    None => Ok(()),
  }
}

pub async fn finish_deferred_network_approval(
  _session: &Session,
  deferred: Option<DeferredNetworkApproval>,
) {
  let Some(deferred) = deferred else {
    return;
  };
  service().unregister_attempt(deferred.attempt_id()).await;
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn immediate_finalize_propagates_user_denial() {
    let session = Session::new();
    let active = begin_network_approval(
      &session,
      "turn-1",
      "call-1",
      true,
      Some(NetworkApprovalSpec {
        mode: NetworkApprovalMode::Immediate,
      }),
    )
    .await
    .expect("active approval");

    let attempt_id = active.attempt_id().expect("attempt id").to_string();
    record_network_approval_outcome(&attempt_id, NetworkApprovalOutcome::DeniedByUser).await;

    let result = finish_immediate_network_approval(&session, active).await;
    assert!(matches!(result, Err(ToolError::Rejected(_))));
  }

  #[tokio::test]
  async fn deferred_finalize_unregisters_attempt() {
    let session = Session::new();
    let active = begin_network_approval(
      &session,
      "turn-1",
      "call-1",
      true,
      Some(NetworkApprovalSpec {
        mode: NetworkApprovalMode::Deferred,
      }),
    )
    .await
    .expect("active approval");

    let deferred = active.into_deferred();
    let attempt_id = deferred
      .as_ref()
      .expect("deferred handle")
      .attempt_id()
      .to_string();

    finish_deferred_network_approval(&session, deferred).await;
    assert!(service().take_outcome(&attempt_id).await.is_none());
  }
}
