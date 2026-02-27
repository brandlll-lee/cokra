// Agent Status
// Status tracking for agents

use serde::{Deserialize, Serialize};

/// Agent status enum
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentStatus {
    /// Waiting for initialization
    PendingInit,
    /// Currently executing
    Running,
    /// Done with optional final message
    Completed(Option<String>),
    /// Encountered error
    Errored(String),
    /// Shut down
    Shutdown,
    /// Agent not found
    NotFound,
}

impl AgentStatus {
    /// Check if status is final (no more updates)
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            AgentStatus::Completed(_) |
            AgentStatus::Errored(_) |
            AgentStatus::Shutdown |
            AgentStatus::NotFound
        )
    }
}

/// Check if status is final
pub fn is_final(status: &AgentStatus) -> bool {
    status.is_final()
}

/// Derive agent status from event
pub fn agent_status_from_event(msg: &cokra_protocol::EventMsg) -> Option<AgentStatus> {
    use cokra_protocol::EventMsg;

    match msg {
        EventMsg::TurnStarted(_) => Some(AgentStatus::Running),
        EventMsg::TurnComplete(e) => {
            // Extract final message from completion
            Some(AgentStatus::Completed(None))
        }
        EventMsg::TurnAborted(e) => Some(AgentStatus::Errored(e.reason.clone())),
        EventMsg::Error(e) => Some(AgentStatus::Errored(e.error.clone())),
        EventMsg::ShutdownComplete => Some(AgentStatus::Shutdown),
        _ => None,
    }
}
