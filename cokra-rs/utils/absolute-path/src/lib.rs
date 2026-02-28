// Utils - Absolute Path
// Absolute path utilities

use anyhow::Result as anyhow_result;
use std::path::{Path, PathBuf};

/// Get absolute path
pub fn absolute_path(path: impl AsRef<Path>) -> Result<PathBuf, anyhow::Error> {
  std::fs::canonicalize(path.as_ref())
    .map_err(|e| anyhow::anyhow!("Failed to get absolute path: {}", e))
}
