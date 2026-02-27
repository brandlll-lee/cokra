// Cokra State
// SQLite-based state persistence

pub mod database;

/// State database handle
pub struct StateDb {
    /// Database path
    pub path: String,
}

impl StateDb {
    /// Create new state database
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
        }
    }
}
