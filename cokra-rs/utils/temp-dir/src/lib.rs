// Utils - Temp Dir
// Temporary directory utilities

use anyhow::Result;
use std::path::PathBuf;

/// Create a temporary directory
pub fn temp_dir() -> Result<PathBuf> {
    let dir = std::env::temp_dir();
    Ok(dir)
}

/// Create a new temp directory with prefix
pub fn new_temp_dir(prefix: &str) -> Result<PathBuf> {
    let temp = std::env::temp_dir();
    let dir = temp.join(format!("{}-{}", prefix, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
