// Cokra Configuration Module
// Layered configuration system

use serde::{Deserialize, Serialize};

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Approval policy
    pub approval: ApprovalPolicy,
    /// Sandbox configuration
    pub sandbox: SandboxConfig,
    /// Personality settings
    pub personality: PersonalityConfig,
}

/// Approval policy settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalPolicy {
    /// Overall policy
    pub policy: ApprovalMode,
    /// Shell command approval
    pub shell: ShellApproval,
    /// Patch approval
    pub patch: PatchApproval,
}

/// Approval modes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalMode {
    Ask,
    Auto,
    Never,
}

/// Shell approval modes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellApproval {
    Always,
    OnFailure,
    UnlessTrusted,
    Never,
}

/// Patch approval modes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PatchApproval {
    Auto,
    OnRequest,
    Never,
}

/// Sandbox configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Sandbox mode
    pub mode: SandboxMode,
}

/// Sandbox modes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    Strict,
    Permissive,
    DangerFullAccess,
}

/// Personality configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalityConfig {
    /// Personality name
    pub name: String,
}
