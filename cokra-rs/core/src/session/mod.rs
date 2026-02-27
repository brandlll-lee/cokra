// Session Module
pub mod manager;
pub mod task;
pub mod state;

pub use manager::Session;
pub use state::{SessionConfiguration, SessionState, ActiveTurn, TurnState};

use cokra_protocol::ThreadId;

/// Session source for tracking agent hierarchy
#[derive(Debug, Clone)]
pub enum SessionSource {
    /// Root session
    Root,
    /// Sub-agent session
    SubAgent {
        /// Depth of nesting
        depth: i32,
    },
}

impl SessionSource {
    /// Create a new root source
    pub fn root() -> Self {
        Self::Root
    }

    /// Create a new sub-agent source
    pub fn sub_agent(depth: i32) -> Self {
        Self::SubAgent { depth }
    }

    /// Get the depth of this source
    pub fn depth(&self) -> i32 {
        match self {
            Self::Root => 0,
            Self::SubAgent { depth } => *depth,
        }
    }
}

impl Default for SessionSource {
    fn default() -> Self {
        Self::Root
    }
}
