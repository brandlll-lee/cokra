// Cokra Session Module
// Session management and turn execution

/// Session manager
pub struct SessionManager {
    config: std::sync::Arc<crate::config::Config>,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new(config: std::sync::Arc<crate::config::Config>) -> Self {
        Self { config }
    }
}
