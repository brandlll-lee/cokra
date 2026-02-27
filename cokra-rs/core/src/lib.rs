// Cokra Core Library
// Main orchestrator for AI agent team system

pub mod config;
pub mod agent;
pub mod tools;
pub mod mcp;
pub mod session;

/// Cokra - Main orchestrator structure
pub struct Cokra {
    config: std::sync::Arc<config::Config>,
}

impl Cokra {
    /// Create a new Cokra instance
    pub fn new(config: config::Config) -> Result<Self, anyhow::Error> {
        Ok(Self {
            config: std::sync::Arc::new(config),
        })
    }

    /// Get configuration
    pub fn config(&self) -> &config::Config {
        &self.config
    }
}
