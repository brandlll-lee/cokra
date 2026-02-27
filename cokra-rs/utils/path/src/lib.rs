// Utils - Path
// Path utilities

use std::path::Path;

/// Join path components
pub fn join(path1: impl AsRef<Path>, path2: impl AsRef<Path>) -> std::path::PathBuf {
    path1.as_ref().join(path2)
}

/// Get parent directory
pub fn parent(path: impl AsRef<Path>) -> Option<std::path::PathBuf> {
    path.as_ref().parent().map(|p| p.to_path_buf())
}
