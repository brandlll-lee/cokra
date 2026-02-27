// Configuration Profile
// User profile configuration

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigProfile {
    /// Model to use
    pub model: Option<String>,
    /// Model provider
    pub model_provider: Option<String>,
    /// Approval policy
    pub approval_policy: Option<String>,
    /// Sandbox mode
    pub sandbox_mode: Option<String>,
    /// Reasoning effort
    pub model_reasoning_effort: Option<String>,
    /// Personality
    pub personality: Option<String>,
    /// Custom instructions file
    pub model_instructions_file: Option<PathBuf>,
    /// Features
    pub features: Option<FeaturesProfile>,
}

/// Features profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeaturesProfile {
    /// Enable MCP
    pub mcp: Option<bool>,
    /// Enable memories
    pub memories: Option<bool>,
    /// Enable web search
    pub web_search: Option<bool>,
}

impl Default for ConfigProfile {
    fn default() -> Self {
        Self {
            model: None,
            model_provider: None,
            approval_policy: None,
            sandbox_mode: None,
            model_reasoning_effort: None,
            personality: None,
            model_instructions_file: None,
            features: None,
        }
    }
}
