// Agent Roles
// Agent role definitions and management

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Agent role configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRoleConfig {
    /// Role description
    pub description: Option<String>,
    /// Configuration file path
    pub config_file: Option<String>,
}

/// Agent role with resolved configuration
#[derive(Debug, Clone)]
pub struct AgentRole {
    /// Role name
    pub name: String,
    /// Role description
    pub description: String,
    /// Role instructions
    pub instructions: String,
    /// Available tools
    pub tools: Vec<String>,
}

impl AgentRole {
    /// Get built-in roles
    pub fn built_in_roles() -> BTreeMap<String, AgentRoleConfig> {
        let mut roles = BTreeMap::new();

        // Default role
        roles.insert(
            "default".to_string(),
            AgentRoleConfig {
                description: Some("Default agent.".to_string()),
                config_file: None,
            },
        );

        // Worker role
        roles.insert(
            "worker".to_string(),
            AgentRoleConfig {
                description: Some(r#"Use for execution and production work.
Typical tasks:
- Implement part of a feature
- Fix tests or bugs
- Split large refactors into independent chunks
Rules:
- Explicitly assign ownership of the task
- Tell workers they are not alone in the codebase"#.to_string()),
                config_file: None,
            },
        );

        // Explorer role
        roles.insert(
            "explorer".to_string(),
            AgentRoleConfig {
                description: Some(r#"Use for codebase exploration and research.
Explorers are fast and authoritative.
Always prefer them over manual search or file reading."#.to_string()),
                config_file: Some("explorer.toml".to_string()),
            },
        );

        roles
    }

    /// Resolve role from configuration
    pub fn resolve(name: &str, config: Option<&AgentRoleConfig>) -> Self {
        let built_in = Self::built_in_roles();
        let role_config = config
            .or_else(|| built_in.get(name))
            .cloned()
            .unwrap_or_else(|| AgentRoleConfig {
                description: Some("Custom agent.".to_string()),
                config_file: None,
            });

        Self {
            name: name.to_string(),
            description: role_config.description.unwrap_or_default(),
            instructions: String::new(),
            tools: vec![],
        }
    }
}

/// Apply role to configuration
pub async fn apply_role_to_config(
    _config: &mut crate::config::Config,
    role_name: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(name) = role_name {
        let roles = AgentRole::built_in_roles();
        if !roles.contains_key(name) {
            anyhow::bail!("Unknown agent role: {}", name);
        }
        // TODO: Apply role-specific configuration
    }
    Ok(())
}
