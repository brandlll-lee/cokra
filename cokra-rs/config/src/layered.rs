// Layered Configuration
// Support for layered configuration with precedence

use serde::{Deserialize, Serialize};

/// Layered configuration wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayeredConfig {
  /// Configuration layers
  layers: Vec<ConfigLayer>,
}

/// Configuration layer with source tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigLayer {
  /// Layer source
  pub source: ConfigLayerSource,
  /// Configuration values
  pub values: toml::Value,
}

/// Configuration layer source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConfigLayerSource {
  /// Built-in defaults
  Default,
  /// Global user config
  GlobalConfig,
  /// Project-specific config
  ProjectConfig,
  /// CLI override
  CliOverride,
  /// Remote config
  RemoteConfig,
}

impl LayeredConfig {
  /// Create a new layered configuration
  pub fn new() -> Self {
    Self { layers: Vec::new() }
  }

  /// Add a layer
  pub fn add_layer(&mut self, layer: ConfigLayer) {
    self.layers.push(layer);
  }

  /// Get merged configuration
  pub fn merge(&self) -> toml::Value {
    let mut merged = toml::Value::Table(toml::map::Map::new());

    for layer in &self.layers {
      if let toml::Value::Table(ref table) = layer.values {
        for (key, value) in table {
          if let toml::Value::Table(ref merged_table) = merged {
            let mut new_table = merged_table.clone();
            Self::merge_values(&mut new_table, key, value.clone());
            merged = toml::Value::Table(new_table);
          }
        }
      }
    }

    merged
  }

  fn merge_values(table: &mut toml::map::Map<String, toml::Value>, key: &str, value: toml::Value) {
    table.insert(key.to_string(), value);
  }
}

impl Default for LayeredConfig {
  fn default() -> Self {
    Self::new()
  }
}
