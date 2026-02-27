// Configuration Loader
// Layered configuration loading system

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::types::Config;

/// Configuration loader with layered support
pub struct ConfigLoader {
    /// Global config directory
    global_dir: PathBuf,
    /// Project config directory
    project_dir: Option<PathBuf>,
}

impl ConfigLoader {
    /// Create a new configuration loader
    pub fn new() -> Self {
        let global_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cokra");

        Self {
            global_dir,
            project_dir: None,
        }
    }

    /// Set project directory
    pub fn with_project_dir(mut self, dir: PathBuf) -> Self {
        self.project_dir = Some(dir);
        self
    }

    /// Load configuration with CLI overrides
    pub fn load_with_cli_overrides(
        &self,
        cli_overrides: Vec<(String, String)>,
    ) -> Result<Config> {
        // Load layers in order:
        // 1. Built-in defaults
        // 2. Global config (~/.cokra/config.toml)
        // 3. Project config (.cokra/config.toml)
        // 4. CLI overrides

        let mut config = self.load_defaults()?;

        // Load global config
        if let Ok(global_config) = self.load_global_config() {
            config = self.merge_configs(config, global_config);
        }

        // Load project config
        if let Some(project_dir) = &self.project_dir {
            if let Ok(project_config) = self.load_project_config(project_dir) {
                config = self.merge_configs(config, project_config);
            }
        }

        // Apply CLI overrides
        for (key, value) in cli_overrides {
            config = self.apply_override(config, &key, &value)?;
        }

        Ok(config)
    }

    /// Load default configuration
    fn load_defaults(&self) -> Result<Config> {
        Ok(Config {
            approval: crate::types::ApprovalPolicy {
                policy: crate::types::ApprovalMode::Ask,
                shell: crate::types::ShellApproval::OnFailure,
                patch: crate::types::PatchApproval::OnRequest,
            },
            sandbox: crate::types::SandboxConfig {
                mode: crate::types::SandboxMode::Permissive,
                network_access: false,
            },
            personality: crate::types::PersonalityConfig {
                name: "default".to_string(),
                instructions: None,
            },
            features: crate::types::FeaturesConfig::default(),
            mcp: crate::types::McpConfig {
                servers: std::collections::HashMap::new(),
            },
            skills: crate::types::SkillsConfig {
                enabled: true,
                paths: vec![],
            },
            memories: crate::types::MemoriesConfig::default(),
            models: crate::types::ModelsConfig::default(),
            history: crate::types::HistoryConfig {
                persistence: crate::types::HistoryPersistence::SaveAll,
                max_bytes: None,
            },
            tui: crate::types::TuiConfig::default(),
            shell_environment: crate::types::ShellEnvironmentPolicy {
                inherit: crate::types::ShellEnvironmentPolicyInherit::Core,
                exclude: vec![],
                set: std::collections::HashMap::new(),
            },
            agents: crate::types::AgentConfig::default(),
        })
    }

    /// Load global configuration file
    fn load_global_config(&self) -> Result<Config> {
        let config_path = self.global_dir.join("config.toml");
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            anyhow::bail!("Global config not found")
        }
    }

    /// Load project configuration file
    fn load_project_config(&self, project_dir: &Path) -> Result<Config> {
        let config_path = project_dir.join(".cokra").join("config.toml");
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            anyhow::bail!("Project config not found")
        }
    }

    /// Merge two configurations
    fn merge_configs(&self, base: Config, override_config: Config) -> Config {
        // Simple merge - override config takes precedence
        override_config
    }

    /// Apply a single CLI override
    fn apply_override(&self, mut config: Config, key: &str, value: &str) -> Result<Config> {
        match key {
            "approval.policy" => {
                config.approval.policy = match value {
                    "ask" => crate::types::ApprovalMode::Ask,
                    "auto" => crate::types::ApprovalMode::Auto,
                    "never" => crate::types::ApprovalMode::Never,
                    _ => anyhow::bail!("Invalid approval policy: {}", value),
                };
            }
            "sandbox.mode" => {
                config.sandbox.mode = match value {
                    "strict" => crate::types::SandboxMode::Strict,
                    "permissive" => crate::types::SandboxMode::Permissive,
                    "danger_full_access" => crate::types::SandboxMode::DangerFullAccess,
                    _ => anyhow::bail!("Invalid sandbox mode: {}", value),
                };
            }
            "models.model" => {
                config.models.model = value.to_string();
            }
            _ => {
                anyhow::bail!("Unknown config key: {}", key);
            }
        }
        Ok(config)
    }
}

impl Default for ConfigLoader {
    fn default() -> Self {
        Self::new()
    }
}
