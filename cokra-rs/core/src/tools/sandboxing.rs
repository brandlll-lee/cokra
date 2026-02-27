// Sandboxing and Approvals
// Core approval and sandbox orchestration

use serde::Serialize;
use std::collections::HashMap;

/// Approval store with caching
#[derive(Clone, Default, Debug)]
pub struct ApprovalStore {
    map: HashMap<String, ReviewDecision>,
}

impl ApprovalStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get<K>(&self, key: &K) -> Option<ReviewDecision>
    where
        K: Serialize,
    {
        let key_str = serde_json::to_string(key).ok()?;
        self.map.get(&key_str).copied()
    }

    pub fn put<K>(&mut self, key: K, value: ReviewDecision)
    where
        K: Serialize,
    {
        if let Ok(key_str) = serde_json::to_string(&key) {
            self.map.insert(key_str, value);
        }
    }
}

/// Review decision
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewDecision {
    Approved,
    Denied,
    Always,
}

/// Approval requirement
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalRequirement {
    /// Skip approval
    Skip {
        bypass_sandbox: bool,
    },
    /// Needs approval
    NeedsApproval {
        reason: Option<String>,
    },
    /// Forbidden
    Forbidden {
        reason: String,
    },
}

/// Sandbox override mode
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxOverride {
    NoOverride,
    BypassSandboxFirstAttempt,
}

/// Approval context
pub struct ApprovalCtx {
    pub session_id: String,
    pub turn_id: String,
    pub call_id: String,
    pub tool_name: String,
}

/// Tool error
#[derive(Debug)]
pub enum ToolError {
    Rejected(String),
    Execution(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::Rejected(msg) => write!(f, "Rejected: {}", msg),
            ToolError::Execution(msg) => write!(f, "Execution error: {}", msg),
        }
    }
}

impl std::error::Error for ToolError {}
