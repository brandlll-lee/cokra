// Cargo bin utility
use std::path::PathBuf;

pub fn cargo_bin() -> Result<PathBuf, String> {
    Ok(PathBuf::from("cargo"))
}
