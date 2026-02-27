// Utils - Cargo Bin
// Cargo binary utilities

use std::path::Path;
use anyhow::Result;

/// Find cargo binary path
pub fn cargo_bin(bin_name: &str) -> Result<std::path::PathBuf> {
    let output = std::process::Command::new("cargo")
        .args(["which", bin_name, "--quiet"])
        .output()?;

    let path = std::str::from_utf8(&output.stdout)
        .trim()
        .to_string();

    Ok(PathBuf::from(path))
}
