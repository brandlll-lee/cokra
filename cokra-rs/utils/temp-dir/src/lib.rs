// Temp dir utility
use std::path::PathBuf;

pub fn create_temp_dir(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(prefix)
}
