// Configuration Types
// All configuration type definitions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
  /// Approval policy settings
  pub approval: ApprovalPolicy,
  /// Sandbox configuration
  pub sandbox: SandboxConfig,
  /// Personality settings
  pub personality: PersonalityConfig,
  /// Feature flags
  pub features: FeaturesConfig,
  /// MCP server configurations
  pub mcp: McpConfig,
  /// Skills configuration
  pub skills: SkillsConfig,
  /// Memory settings
  pub memories: MemoriesConfig,
  /// Model configuration
  pub models: ModelsConfig,
  /// History settings
  pub history: HistoryConfig,
  /// TUI settings
  pub tui: TuiConfig,
  /// Shell environment policy
  pub shell_environment: ShellEnvironmentPolicy,
  /// Agent configuration
  pub agents: AgentConfig,
}

// ============================================================================
// APPROVAL POLICY
// ============================================================================

/// Approval policy settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalPolicy {
  /// Overall approval mode
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

// ============================================================================
// SANDBOX CONFIGURATION
// ============================================================================

/// Sandbox configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
  /// Sandbox mode
  pub mode: SandboxMode,
  /// Network access
  pub network_access: bool,
}

/// Sandbox modes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
  Strict,
  Permissive,
  DangerFullAccess,
}

// ============================================================================
// PERSONALITY CONFIGURATION
// ============================================================================

/// Personality configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalityConfig {
  /// Personality name
  pub name: String,
  /// Custom instructions
  pub instructions: Option<String>,
}

// ============================================================================
// FEATURES CONFIGURATION
// ============================================================================

/// Feature flags configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeaturesConfig {
  /// Enable MCP
  pub mcp: bool,
  /// Enable memories
  pub memories: bool,
  /// Enable web search
  pub web_search: bool,
  /// Enable JS REPL
  pub js_repl: bool,
  /// Enable cloud tasks
  pub cloud_tasks: bool,
}

impl Default for FeaturesConfig {
  fn default() -> Self {
    Self {
      mcp: true,
      memories: false,
      web_search: false,
      js_repl: false,
      cloud_tasks: false,
    }
  }
}

// ============================================================================
// MCP CONFIGURATION
// ============================================================================

/// MCP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
  /// MCP server configurations
  pub servers: HashMap<String, McpServerConfig>,
}

/// MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
  /// Transport configuration
  pub transport: McpServerTransportConfig,
  /// Whether server is enabled
  pub enabled: bool,
  /// Whether server is required
  pub required: bool,
  /// Startup timeout in seconds
  pub startup_timeout_sec: Option<u64>,
  /// Tool timeout in seconds
  pub tool_timeout_sec: Option<u64>,
  /// Enabled tools filter
  pub enabled_tools: Option<Vec<String>>,
  /// Disabled tools filter
  pub disabled_tools: Option<Vec<String>>,
}

/// MCP server transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpServerTransportConfig {
  /// stdio transport
  Stdio {
    command: String,
    args: Vec<String>,
    env: Option<HashMap<String, String>>,
    cwd: Option<PathBuf>,
  },
  /// HTTP transport
  Http {
    url: String,
    bearer_token: Option<String>,
    headers: Option<HashMap<String, String>>,
  },
}

// ============================================================================
// SKILLS CONFIGURATION
// ============================================================================

/// Skills configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsConfig {
  /// Whether skills system is enabled
  pub enabled: bool,
  /// Local skill paths
  pub paths: Vec<PathBuf>,
}

// ============================================================================
// MEMORIES CONFIGURATION
// ============================================================================

/// Memories configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoriesConfig {
  /// Max raw memories for global
  pub max_raw_memories_for_global: usize,
  /// Max rollout age in days
  pub max_rollout_age_days: i64,
  /// Max rollouts per startup
  pub max_rollouts_per_startup: usize,
  /// Min rollout idle hours
  pub min_rollout_idle_hours: i64,
}

impl Default for MemoriesConfig {
  fn default() -> Self {
    Self {
      max_raw_memories_for_global: 100,
      max_rollout_age_days: 30,
      max_rollouts_per_startup: 10,
      min_rollout_idle_hours: 1,
    }
  }
}

// ============================================================================
// MODELS CONFIGURATION
// ============================================================================

/// Models configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
  /// Model provider
  pub provider: String,
  /// Model name
  pub model: String,
  /// Base URL for API
  pub base_url: Option<String>,
}

impl Default for ModelsConfig {
  fn default() -> Self {
    Self {
      provider: "openai".to_string(),
      model: "gpt-5.2-codex".to_string(),
      base_url: None,
    }
  }
}

// ============================================================================
// HISTORY CONFIGURATION
// ============================================================================

/// History configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryConfig {
  /// Persistence mode
  pub persistence: HistoryPersistence,
  /// Max bytes to store
  pub max_bytes: Option<usize>,
}

/// History persistence modes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryPersistence {
  SaveAll,
  None,
}

// ============================================================================
// TUI CONFIGURATION
// ============================================================================

/// TUI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
  /// Notifications enabled
  pub notifications: bool,
  /// Animation enabled
  pub animations: bool,
  /// Show tooltips
  pub show_tooltips: bool,
  /// Alternate screen mode
  pub alternate_screen: bool,
}

impl Default for TuiConfig {
  fn default() -> Self {
    Self {
      notifications: true,
      animations: true,
      show_tooltips: true,
      alternate_screen: true,
    }
  }
}

// ============================================================================
// SHELL ENVIRONMENT POLICY
// ============================================================================

/// Shell environment policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellEnvironmentPolicy {
  /// Inheritance mode
  pub inherit: ShellEnvironmentPolicyInherit,
  /// Exclude patterns
  pub exclude: Vec<String>,
  /// Set environment variables
  pub set: HashMap<String, String>,
}

/// Shell environment inheritance modes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellEnvironmentPolicyInherit {
  Core,
  All,
  None,
}

// ============================================================================
// AGENT CONFIGURATION
// ============================================================================

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
  /// Max concurrent threads
  pub max_threads: usize,
  /// Agent roles
  pub roles: HashMap<String, AgentRoleConfig>,
}

impl Default for AgentConfig {
  fn default() -> Self {
    Self {
      max_threads: 10,
      roles: HashMap::new(),
    }
  }
}

/// Agent role configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRoleConfig {
  /// Role description
  pub description: Option<String>,
  /// Config file path
  pub config_file: Option<String>,
}
