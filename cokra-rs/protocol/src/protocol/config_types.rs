// Configuration Types for Protocol
// Types used in protocol for configuration

use serde::{Deserialize, Serialize};
use crate::ModeKind;

/// Ask for approval policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AskForApproval {
    /// Only auto-approve known safe read-only commands
    UnlessTrusted,
    /// Auto-approve in sandbox, escalate on failure (DEPRECATED)
    OnFailure,
    /// Model decides when to ask (default)
    OnRequest,
    /// Never ask - fail immediately on errors
    Never,
}

/// Sandbox policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SandboxPolicy {
    /// Full access to system (dangerous)
    DangerFullAccess,
    /// Read-only access
    ReadOnly {
        access: ReadOnlyAccess,
    },
    /// External sandbox with network control
    ExternalSandbox {
        network_access: NetworkAccess,
    },
    /// Workspace write with restrictions
    WorkspaceWrite {
        writable_roots: Vec<String>,
        read_only_access: ReadOnlyAccess,
        network_access: bool,
        exclude_tmpdir_env_var: bool,
        exclude_slash_tmp: bool,
    },
}

/// Read-only access level
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReadOnlyAccess {
    /// Restricted access with specific readable roots
    Restricted {
        include_platform_defaults: bool,
        readable_roots: Vec<String>,
    },
    /// Full read access
    FullAccess,
}

/// Network access level
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkAccess {
    /// Full network access
    Full,
    /// No network access
    None,
}

/// Review decision for approvals
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReviewDecision {
    /// Approved for execution
    Approved,
    /// Denied
    Denied,
    /// Approved and always auto-approve in future
    Always,
}

/// Collaboration mode
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaborationMode {
    pub mode: ModeKind,
    pub settings: Settings,
}

/// Settings for collaboration mode
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub developer_instructions: Option<String>,
}

/// Reasoning effort level
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
}

/// Reasoning effort config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningEffortConfig {
    pub effort: ReasoningEffort,
}

/// Reasoning summary config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReasoningSummaryConfig {
    Auto,
    Always,
    Never,
}

/// Personality configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Personality {
    pub name: String,
    pub instructions: Option<String>,
}

/// Windows sandbox level
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WindowsSandboxLevel {
    /// Full access
    FullAccess,
    /// Restricted
    Restricted,
}
