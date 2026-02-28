use serde::{Deserialize, Serialize};

/// Runtime status for an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
  PendingInit,
  Initializing,
  Ready,
  Busy,
  Error(String),
  Shutdown,
}

impl Default for AgentStatus {
  fn default() -> Self {
    Self::PendingInit
  }
}

impl AgentStatus {
  /// Validate state transition.
  pub fn can_transition_to(&self, new: &AgentStatus) -> bool {
    match (self, new) {
      (Self::PendingInit, Self::Initializing) => true,
      (Self::Initializing, Self::Ready) => true,
      (Self::Initializing, Self::Error(_)) => true,
      (Self::Ready, Self::Busy) => true,
      (Self::Ready, Self::Shutdown) => true,
      (Self::Busy, Self::Ready) => true,
      (Self::Busy, Self::Error(_)) => true,
      (Self::Busy, Self::Shutdown) => true,
      (Self::Error(_), Self::Ready) => true,
      (Self::Error(_), Self::Shutdown) => true,
      (Self::Shutdown, Self::Shutdown) => true,
      // allow idempotent transitions
      (a, b) if a == b => true,
      _ => false,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::AgentStatus;

  #[test]
  fn validates_transitions() {
    assert!(AgentStatus::PendingInit.can_transition_to(&AgentStatus::Initializing));
    assert!(AgentStatus::Busy.can_transition_to(&AgentStatus::Ready));
    assert!(!AgentStatus::Ready.can_transition_to(&AgentStatus::PendingInit));
    assert!(!AgentStatus::Shutdown.can_transition_to(&AgentStatus::Ready));
  }
}
