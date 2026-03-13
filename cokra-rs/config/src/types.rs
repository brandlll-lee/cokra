// Configuration Types
// All configuration type definitions

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::layer_stack::ConfigLayerStack;

fn default_cwd() -> PathBuf {
  std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn default_true() -> bool {
  true
}

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Config {
  /// Approval policy settings
  #[serde(default)]
  pub approval: ApprovalPolicy,
  /// Sandbox configuration
  #[serde(default)]
  pub sandbox: SandboxConfig,
  /// Personality settings
  #[serde(default)]
  pub personality: PersonalityConfig,
  /// Feature flags
  #[serde(default)]
  pub features: FeaturesConfig,
  /// MCP server configurations
  #[serde(default)]
  pub mcp: McpConfig,
  /// Skills configuration
  #[serde(default)]
  pub skills: SkillsConfig,
  /// Memory settings
  #[serde(default)]
  pub memories: MemoriesConfig,
  /// Model configuration
  #[serde(default)]
  pub models: ModelsConfig,
  /// History settings
  #[serde(default)]
  pub history: HistoryConfig,
  /// TUI settings
  #[serde(default)]
  pub tui: TuiConfig,
  /// Shell environment policy
  #[serde(default)]
  pub shell_environment: ShellEnvironmentPolicy,
  /// Agent configuration
  #[serde(default)]
  pub agents: AgentConfig,
  /// Tool configuration
  #[serde(default)]
  pub tools: ToolsConfig,

  /// Projects trust map keyed by canonical path string.
  ///
  /// Mirrors Codex: `[projects."..."] trust_level = "trusted" | "untrusted"`.
  #[serde(default)]
  pub projects: HashMap<String, ProjectConfig>,

  /// Markers used to detect the project root when searching parent directories
  /// for `.cokra` folders. Defaults to [".git"] when unset.
  #[serde(default)]
  pub project_root_markers: Option<Vec<String>>,

  /// Session working directory (runtime override, not persisted in config.toml).
  #[serde(default = "default_cwd", skip_serializing)]
  #[schemars(skip)]
  pub cwd: PathBuf,

  /// Debug-only metadata: resolved config layers for this session cwd.
  #[serde(skip)]
  #[schemars(skip)]
  pub config_layer_stack: Option<ConfigLayerStack>,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      approval: ApprovalPolicy::default(),
      sandbox: SandboxConfig::default(),
      personality: PersonalityConfig::default(),
      features: FeaturesConfig::default(),
      mcp: McpConfig::default(),
      skills: SkillsConfig::default(),
      memories: MemoriesConfig::default(),
      models: ModelsConfig::default(),
      history: HistoryConfig::default(),
      tui: TuiConfig::default(),
      shell_environment: ShellEnvironmentPolicy::default(),
      agents: AgentConfig::default(),
      tools: ToolsConfig::default(),
      projects: HashMap::new(),
      project_root_markers: None,
      cwd: default_cwd(),
      config_layer_stack: None,
    }
  }
}

// ============================================================================
// APPROVAL POLICY
// ============================================================================

/// Approval policy settings
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalPolicy {
  /// Overall approval mode
  pub policy: ApprovalMode,
  /// Shell command approval
  pub shell: ShellApproval,
  /// Patch approval
  pub patch: PatchApproval,
}

impl Default for ApprovalPolicy {
  fn default() -> Self {
    Self {
      policy: ApprovalMode::Ask,
      shell: ShellApproval::OnFailure,
      patch: PatchApproval::OnRequest,
    }
  }
}

/// Approval modes
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalMode {
  Ask,
  Auto,
  Never,
}

impl Default for ApprovalMode {
  fn default() -> Self {
    Self::Ask
  }
}

/// Shell approval modes
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ShellApproval {
  Always,
  OnFailure,
  UnlessTrusted,
  Never,
}

impl Default for ShellApproval {
  fn default() -> Self {
    Self::OnFailure
  }
}

/// Patch approval modes
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PatchApproval {
  Auto,
  OnRequest,
  Never,
}

impl Default for PatchApproval {
  fn default() -> Self {
    Self::OnRequest
  }
}

// ============================================================================
// SANDBOX CONFIGURATION
// ============================================================================

/// Sandbox configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SandboxConfig {
  /// Sandbox mode
  pub mode: SandboxMode,
  /// Network access
  pub network_access: bool,
}

impl Default for SandboxConfig {
  fn default() -> Self {
    Self {
      mode: SandboxMode::Permissive,
      network_access: false,
    }
  }
}

/// Sandbox modes
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
  Strict,
  Permissive,
  DangerFullAccess,
}

impl Default for SandboxMode {
  fn default() -> Self {
    Self::Permissive
  }
}

// ============================================================================
// PERSONALITY CONFIGURATION
// ============================================================================

/// Personality configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PersonalityConfig {
  /// Personality name
  pub name: String,
  /// Custom instructions
  pub instructions: Option<String>,
}

impl Default for PersonalityConfig {
  fn default() -> Self {
    Self {
      name: "default".to_string(),
      instructions: None,
    }
  }
}

// ============================================================================
// FEATURES CONFIGURATION
// ============================================================================

/// Feature flags configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
// TOOLS CONFIGURATION
// ============================================================================

/// Tool-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct ToolsConfig {
  /// Exec tool configuration.
  #[serde(default)]
  pub exec: ExecToolsConfig,
}

/// Exec tool surface and backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecToolsConfig {
  /// Which exec tool name to expose to the model.
  #[serde(default)]
  pub public_surface: ExecPublicSurface,
  /// Which implementation path should back shell-family execution.
  #[serde(default)]
  pub backend: ExecBackend,
}

impl Default for ExecToolsConfig {
  fn default() -> Self {
    Self {
      public_surface: ExecPublicSurface::default(),
      backend: ExecBackend::default(),
    }
  }
}

/// Which exec tool should be shown to the model.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecPublicSurface {
  #[default]
  Auto,
  Shell,
  UnifiedExec,
}

/// Which backend style should execute shell-family tool calls internally.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecBackend {
  #[default]
  Auto,
  ShellCommand,
  UnifiedExec,
}

// ============================================================================
// MCP CONFIGURATION
// ============================================================================

/// MCP configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct McpConfig {
  /// MCP server configurations
  #[serde(default)]
  pub servers: HashMap<String, McpServerConfig>,
}

/// MCP server transport configuration.
///
/// Discriminated by the presence of fields (codex-style flat schema):
///   - `command` present → Stdio transport
///   - `url` present     → StreamableHttp transport
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub enum McpServerTransportConfig {
  Stdio {
    command: String,
    args: Vec<String>,
    env: Option<HashMap<String, String>>,
    cwd: Option<PathBuf>,
  },
  Http {
    url: String,
    bearer_token: Option<String>,
    headers: Option<HashMap<String, String>>,
  },
}

/// Flat (un-nested) deserialization shape — mirrors codex RawMcpServerConfig.
/// Users write all fields directly under `[mcp.servers.<name>]`, no sub-table.
#[derive(serde::Deserialize)]
struct RawMcpServerConfig {
  // ── Stdio fields ──────────────────────────────────────────────────────────
  command: Option<String>,
  #[serde(default)]
  args: Vec<String>,
  #[serde(default)]
  env: Option<HashMap<String, String>>,
  #[serde(default)]
  cwd: Option<PathBuf>,

  // ── HTTP fields ───────────────────────────────────────────────────────────
  url: Option<String>,
  #[serde(default)]
  bearer_token: Option<String>,
  #[serde(default)]
  headers: Option<HashMap<String, String>>,

  // ── Shared fields ─────────────────────────────────────────────────────────
  #[serde(default = "default_true")]
  enabled: bool,
  #[serde(default)]
  required: bool,
  #[serde(default)]
  startup_timeout_sec: Option<u64>,
  #[serde(default)]
  tool_timeout_sec: Option<u64>,
  #[serde(default)]
  enabled_tools: Option<Vec<String>>,
  #[serde(default)]
  disabled_tools: Option<Vec<String>>,
}

/// MCP server configuration.
///
/// **TOML format** (flat, no nested `[transport]` sub-table):
///
/// ```toml
/// # Stdio
/// [mcp.servers.my-server]
/// command = "npx"
/// args = ["-y", "some-mcp-package"]
/// env = { API_KEY = "..." }
///
/// # HTTP
/// [mcp.servers.my-http-server]
/// url = "https://mcp.example.com/mcp/"
/// bearer_token = "..."
/// ```
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct McpServerConfig {
  /// Transport configuration (derived from flat fields).
  #[serde(flatten)]
  pub transport: McpServerTransportConfig,
  #[serde(default = "default_true")]
  pub enabled: bool,
  #[serde(default)]
  pub required: bool,
  pub startup_timeout_sec: Option<u64>,
  pub tool_timeout_sec: Option<u64>,
  pub enabled_tools: Option<Vec<String>>,
  pub disabled_tools: Option<Vec<String>>,
}

impl<'de> serde::Deserialize<'de> for McpServerConfig {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: serde::Deserializer<'de>,
  {
    let raw = RawMcpServerConfig::deserialize(deserializer)?;

    let transport = if let Some(command) = raw.command {
      McpServerTransportConfig::Stdio {
        command,
        args: raw.args,
        env: raw.env,
        cwd: raw.cwd,
      }
    } else if let Some(url) = raw.url {
      McpServerTransportConfig::Http {
        url,
        bearer_token: raw.bearer_token,
        headers: raw.headers,
      }
    } else {
      return Err(serde::de::Error::custom(
        "MCP server config must have either `command` (stdio) or `url` (http)",
      ));
    };

    Ok(McpServerConfig {
      transport,
      enabled: raw.enabled,
      required: raw.required,
      startup_timeout_sec: raw.startup_timeout_sec,
      tool_timeout_sec: raw.tool_timeout_sec,
      enabled_tools: raw.enabled_tools,
      disabled_tools: raw.disabled_tools,
    })
  }
}

// ============================================================================
// SKILLS CONFIGURATION
// ============================================================================

/// Skills configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillsConfig {
  /// Whether skills system is enabled
  pub enabled: bool,
  /// Local skill paths
  pub paths: Vec<PathBuf>,
}

impl Default for SkillsConfig {
  fn default() -> Self {
    Self {
      enabled: true,
      paths: Vec::new(),
    }
  }
}

// ============================================================================
// MEMORIES CONFIGURATION
// ============================================================================

/// Memories configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelsConfig {
  /// Model provider
  pub provider: String,
  /// Model name
  pub model: String,
  /// Base URL for API
  pub base_url: Option<String>,
  /// API key (optional — falls back to environment variable)
  pub api_key: Option<String>,
}

impl Default for ModelsConfig {
  fn default() -> Self {
    Self {
      provider: "openai".to_string(),
      model: "gpt-5.2-codex".to_string(),
      base_url: None,
      api_key: None,
    }
  }
}

// ============================================================================
// HISTORY CONFIGURATION
// ============================================================================

/// History configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HistoryConfig {
  /// Persistence mode
  pub persistence: HistoryPersistence,
  /// Max bytes to store
  pub max_bytes: Option<usize>,
}

impl Default for HistoryConfig {
  fn default() -> Self {
    Self {
      persistence: HistoryPersistence::SaveAll,
      max_bytes: None,
    }
  }
}

/// History persistence modes
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HistoryPersistence {
  SaveAll,
  None,
}

impl Default for HistoryPersistence {
  fn default() -> Self {
    Self::SaveAll
  }
}

// ============================================================================
// TUI CONFIGURATION
// ============================================================================

/// TUI configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ShellEnvironmentPolicy {
  /// Inheritance mode
  pub inherit: ShellEnvironmentPolicyInherit,
  /// Exclude patterns
  pub exclude: Vec<String>,
  /// Set environment variables
  pub set: HashMap<String, String>,
}

impl Default for ShellEnvironmentPolicy {
  fn default() -> Self {
    Self {
      inherit: ShellEnvironmentPolicyInherit::Core,
      exclude: Vec::new(),
      set: HashMap::new(),
    }
  }
}

/// Shell environment inheritance modes
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ShellEnvironmentPolicyInherit {
  Core,
  All,
  None,
}

impl Default for ShellEnvironmentPolicyInherit {
  fn default() -> Self {
    Self::Core
  }
}

// ============================================================================
// AGENT CONFIGURATION
// ============================================================================

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentRoleConfig {
  /// Role description
  pub description: Option<String>,
  /// Config file path
  pub config_file: Option<String>,
}

// ============================================================================
// PROJECT TRUST CONFIGURATION
// ============================================================================

/// Project trust configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct ProjectConfig {
  pub trust_level: Option<TrustLevel>,
}

impl ProjectConfig {
  pub fn is_trusted(&self) -> bool {
    matches!(self.trust_level, Some(TrustLevel::Trusted))
  }

  pub fn is_untrusted(&self) -> bool {
    matches!(self.trust_level, Some(TrustLevel::Untrusted))
  }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TrustLevel {
  Trusted,
  Untrusted,
}
