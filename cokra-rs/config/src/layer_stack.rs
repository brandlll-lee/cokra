// Layer stack state and metadata (Codex-inspired).
//
// This module is intentionally independent of `types.rs` so it can be embedded
// into `Config` as debug metadata without creating circular dependencies.

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::PathBuf;

pub type TomlValue = toml::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ConfigLayerSource {
  Default,
  System { file: PathBuf },
  User { file: PathBuf },
  Project { dot_cokra_folder: PathBuf },
  SessionFlags,
}

impl ConfigLayerSource {
  pub fn precedence(&self) -> i16 {
    match self {
      ConfigLayerSource::Default => 0,
      ConfigLayerSource::System { .. } => 10,
      ConfigLayerSource::User { .. } => 20,
      ConfigLayerSource::Project { .. } => 25,
      ConfigLayerSource::SessionFlags => 30,
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigLayerEntry {
  pub source: ConfigLayerSource,
  pub config: TomlValue,
  pub disabled_reason: Option<String>,
  pub version: String,
}

impl ConfigLayerEntry {
  pub fn new(source: ConfigLayerSource, config: TomlValue) -> Self {
    let version = version_for_toml(&config);
    Self {
      source,
      config,
      disabled_reason: None,
      version,
    }
  }

  pub fn new_disabled(source: ConfigLayerSource, config: TomlValue, reason: String) -> Self {
    let version = version_for_toml(&config);
    Self {
      source,
      config,
      disabled_reason: Some(reason),
      version,
    }
  }

  pub fn is_enabled(&self) -> bool {
    self.disabled_reason.is_none()
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigLayerMetadata {
  pub source: ConfigLayerSource,
  pub version: String,
  pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigLayerStack {
  layers: Vec<ConfigLayerEntry>,
}

impl ConfigLayerStack {
  pub fn new(layers: Vec<ConfigLayerEntry>) -> Self {
    Self { layers }
  }

  pub fn layers_low_to_high(&self) -> &[ConfigLayerEntry] {
    &self.layers
  }

  pub fn layers_high_to_low(&self) -> Vec<ConfigLayerEntry> {
    let mut v = self.layers.clone();
    v.sort_by(|a, b| b.source.precedence().cmp(&a.source.precedence()));
    v
  }

  pub fn effective_config(&self) -> TomlValue {
    let mut merged = TomlValue::Table(toml::map::Map::new());
    for layer in &self.layers {
      if layer.is_enabled() {
        merge_toml_values(&mut merged, &layer.config);
      }
    }
    merged
  }

  /// Return per-key origins metadata for keys that appear in any enabled layer.
  ///
  /// Keys are dotted paths like `models.model` or `skills.paths`.
  pub fn origins(&self) -> HashMap<String, ConfigLayerMetadata> {
    let mut origins = HashMap::<String, ConfigLayerMetadata>::new();
    for layer in &self.layers {
      if !layer.is_enabled() {
        continue;
      }
      update_origins_for_value(
        &mut origins,
        &layer.config,
        "",
        ConfigLayerMetadata {
          source: layer.source.clone(),
          version: layer.version.clone(),
          disabled_reason: None,
        },
      );
    }
    origins
  }
}

pub fn merge_toml_values(base: &mut TomlValue, overlay: &TomlValue) {
  match (base, overlay) {
    (TomlValue::Table(base_tbl), TomlValue::Table(overlay_tbl)) => {
      for (k, v_overlay) in overlay_tbl {
        match base_tbl.get_mut(k) {
          Some(v_base) => merge_toml_values(v_base, v_overlay),
          None => {
            base_tbl.insert(k.clone(), v_overlay.clone());
          }
        }
      }
    }
    (base_any, overlay_any) => {
      *base_any = overlay_any.clone();
    }
  }
}

fn update_origins_for_value(
  origins: &mut HashMap<String, ConfigLayerMetadata>,
  value: &TomlValue,
  prefix: &str,
  meta: ConfigLayerMetadata,
) {
  match value {
    TomlValue::Table(tbl) => {
      for (k, v) in tbl {
        let next = if prefix.is_empty() {
          k.clone()
        } else {
          format!("{prefix}.{k}")
        };
        // Mark this key as originating from `meta` at its current shape.
        origins.insert(next.clone(), meta.clone());
        update_origins_for_value(origins, v, &next, meta.clone());
      }
    }
    TomlValue::Array(arr) => {
      // For arrays, treat the array key itself as the origin; do not explode indices.
      let key = prefix.to_string();
      if !key.is_empty() {
        origins.insert(key, meta);
      }
      let _ = arr;
    }
    _ => {
      let key = prefix.to_string();
      if !key.is_empty() {
        origins.insert(key, meta);
      }
    }
  }
}

pub fn version_for_toml(value: &TomlValue) -> String {
  let mut hasher = std::collections::hash_map::DefaultHasher::new();
  hash_toml_value(value, &mut hasher);
  format!("{:016x}", hasher.finish())
}

fn hash_toml_value(value: &TomlValue, state: &mut impl Hasher) {
  match value {
    TomlValue::String(s) => {
      0u8.hash(state);
      s.hash(state);
    }
    TomlValue::Integer(i) => {
      1u8.hash(state);
      i.hash(state);
    }
    TomlValue::Float(f) => {
      2u8.hash(state);
      f.to_bits().hash(state);
    }
    TomlValue::Boolean(b) => {
      3u8.hash(state);
      b.hash(state);
    }
    TomlValue::Datetime(dt) => {
      4u8.hash(state);
      dt.to_string().hash(state);
    }
    TomlValue::Array(arr) => {
      5u8.hash(state);
      arr.len().hash(state);
      for item in arr {
        hash_toml_value(item, state);
      }
    }
    TomlValue::Table(tbl) => {
      6u8.hash(state);
      let mut keys: Vec<&String> = tbl.keys().collect();
      keys.sort();
      for k in keys {
        k.hash(state);
        if let Some(v) = tbl.get(k) {
          hash_toml_value(v, state);
        }
      }
    }
  }
}
