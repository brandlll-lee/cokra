// Utils - FS Err
// Filesystem error utilities

use std::path::Path;

/// Check if path exists
pub fn exists(path: impl AsRef<Path>) -> bool {
  path.as_ref().exists()
}

/// Create directory if not exists
pub fn ensure_dir(path: impl AsRef<Path>) -> anyhow::Result<()> {
  std::fs::create_dir_all(path)?;
  Ok(())
}
