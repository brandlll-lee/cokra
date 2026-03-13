use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use schemars::schema_for;

fn main() -> anyhow::Result<()> {
  let schema = schema_for!(cokra_config::Config);
  let json = serde_json::to_string_pretty(&schema).context("serialize config schema")?;

  let output_path = PathBuf::from("core").join("config.schema.json");
  if let Some(parent) = output_path.parent() {
    fs::create_dir_all(parent)
      .with_context(|| format!("create schema directory {}", parent.display()))?;
  }

  fs::write(&output_path, json)
    .with_context(|| format!("write config schema to {}", output_path.display()))?;
  println!("{}", output_path.display());
  Ok(())
}
