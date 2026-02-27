// Cokra Tools Module
// Tool system for extensible command execution

/// Tool registry
pub struct ToolsRegistry;

impl ToolsRegistry {
    /// Create a new tool registry
    pub fn new() -> Self {
        Self
    }
}

impl Default for ToolsRegistry {
    fn default() -> Self {
        Self::new()
    }
}
