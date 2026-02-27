// Session State
use std::collections::HashMap;
use tokio::sync::oneshot;

use crate::session::task::RunningTask;
use cokra_protocol::ReviewDecision;

/// Session configuration
#[derive(Debug, Clone, Default)]
pub struct SessionConfiguration {
    /// Model to use
    pub model: Option<String>,
    /// Approval policy
    pub approval_policy: Option<String>,
    /// Sandbox mode
    pub sandbox_mode: Option<String>,
}

/// Mutable session state
pub(crate) struct SessionState {
    /// Session configuration
    pub(crate) session_configuration: SessionConfiguration,
    /// Environment variables
    pub(crate) dependency_env: HashMap<String, String>,
    /// Initial context seeded flag
    pub(crate) initial_context_seeded: bool,
}

impl SessionState {
    /// Create new session state
    pub fn new(configuration: SessionConfiguration) -> Self {
        Self {
            session_configuration: configuration,
            dependency_env: HashMap::new(),
            initial_context_seeded: false,
        }
    }
}

/// Active turn (running task state)
pub(crate) struct ActiveTurn {
    /// Running tasks by ID
    pub(crate) tasks: indexmap::IndexMap<String, RunningTask>,
    /// Turn state
    pub(crate) turn_state: TurnState,
}

impl ActiveTurn {
    /// Create new active turn
    pub fn new() -> Self {
        Self {
            tasks: indexmap::IndexMap::new(),
            turn_state: TurnState::new(),
        }
    }

    /// Remove a task by ID
    pub fn remove_task(&mut self, task_id: &str) -> bool {
        self.tasks.remove(task_id).is_some()
    }
}

impl Default for ActiveTurn {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-turn mutable state
pub(crate) struct TurnState {
    /// Pending approvals
    pending_approvals: HashMap<String, oneshot::Sender<ReviewDecision>>,
    /// Pending user input requests
    pending_user_input: HashMap<String, oneshot::Sender<String>>,
    /// Pending input items
    pending_input: Vec<String>,
}

impl TurnState {
    /// Create new turn state
    pub fn new() -> Self {
        Self {
            pending_approvals: HashMap::new(),
            pending_user_input: HashMap::new(),
            pending_input: Vec::new(),
        }
    }

    /// Insert a pending approval
    pub fn insert_pending_approval(
        &mut self,
        id: String,
        sender: oneshot::Sender<ReviewDecision>,
    ) -> Option<oneshot::Sender<ReviewDecision>> {
        self.pending_approvals.insert(id, sender)
    }

    /// Remove a pending approval
    pub fn remove_pending_approval(&mut self, id: &str) -> Option<oneshot::Sender<ReviewDecision>> {
        self.pending_approvals.remove(id)
    }

    /// Insert a pending user input
    pub fn insert_pending_user_input(
        &mut self,
        id: String,
        sender: oneshot::Sender<String>,
    ) -> Option<oneshot::Sender<String>> {
        self.pending_user_input.insert(id, sender)
    }

    /// Remove a pending user input
    pub fn remove_pending_user_input(&mut self, id: &str) -> Option<oneshot::Sender<String>> {
        self.pending_user_input.remove(id)
    }

    /// Add pending input
    pub fn add_pending_input(&mut self, input: String) {
        self.pending_input.push(input);
    }

    /// Take all pending input
    pub fn take_pending_input(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_input)
    }
}

impl Default for TurnState {
    fn default() -> Self {
        Self::new()
    }
}
