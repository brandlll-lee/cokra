// Cokra Core Library
// Main orchestrator for AI agent team system

pub mod config;
pub mod agent;
pub mod tools;
pub mod mcp;
pub mod session;

pub use config::Config;
pub use agent::AgentControl;
pub use tools::ToolsRegistry;
pub use mcp::McpConnectionManager;
pub use session::SessionManager;

use std::sync::Arc;
use anyhow::Result;

/// Cokra - Main orchestrator structure
pub struct Cokra {
    /// Configuration
    config: Arc<Config>,
    /// Agent control
    agent_control: AgentControl,
    /// Tools registry
    tools: ToolsRegistry,
    /// MCP connection manager
    mcp_manager: McpConnectionManager,
    /// Session manager
    session_manager: SessionManager,
}

impl Cokra {
    /// Create a new Cokra instance
    pub fn new(config: Config) -> Result<Self> {
        let config = Arc::new(config);

        let agent_control = AgentControl::new(config.clone());
        let tools = ToolsRegistry::new();
        let mcp_manager = McpConnectionManager::new();
        let session_manager = SessionManager::new(config.clone());

        Ok(Self {
            config,
            agent_control,
            tools,
            mcp_manager,
            session_manager,
        })
    }

    /// Get configuration
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Get agent control
    pub fn agent_control(&self) -> &AgentControl {
        &self.agent_control
    }

    /// Get tools registry
    pub fn tools(&self) -> &ToolsRegistry {
        &self.tools
    }

    /// Get MCP manager
    pub fn mcp_manager(&self) -> &McpConnectionManager {
        &self.mcp_manager
    }

    /// Get session manager
    pub fn session_manager(&self) -> &SessionManager {
        &self.session_manager
    }

    /// Shutdown Cokra
    pub async fn shutdown(self) -> Result<()> {
        // TODO: Implement graceful shutdown
        Ok(())
    }
}
