// Cokra Agent Module
// Agent system for multi-agent coordination

/// Agent control structure
pub struct AgentControl {
    max_depth: usize,
}

impl AgentControl {
    /// Create a new agent controller
    pub fn new() -> Self {
        Self {
            max_depth: 5,
        }
    }

    /// Get max spawn depth
    pub fn max_depth(&self) -> usize {
        self.max_depth
    }
}

impl Default for AgentControl {
    fn default() -> Self {
        Self::new()
    }
}
