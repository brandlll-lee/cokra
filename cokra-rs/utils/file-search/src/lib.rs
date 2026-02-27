// Utils - File Search
// File search utilities

use ignore::Walk;
use anyhow::Result;

/// Search for files matching pattern
pub fn search_files(root: &str, pattern: &str) -> Result<Vec<String>> {
    let mut results = Vec::new();

    for entry in Walk::new(root) {
        let path = entry.path();
        if let Some(name) = path.file_name() {
            if name.to_string_lossy().contains(pattern) {
                results.push(path.to_string_lossy().to_string());
            }
        }
    }

    Ok(results)
}
