// Configuration Loader
// Layered configuration loading system

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use crate::layer_stack::ConfigLayerEntry;
use crate::layer_stack::ConfigLayerSource;
use crate::layer_stack::ConfigLayerStack;
use crate::layer_stack::TomlValue;
use crate::layer_stack::merge_toml_values;
use crate::types::Config;
use crate::types::TrustLevel;

const DEFAULT_PROJECT_ROOT_MARKERS: &[&str] = &[".git"];

#[derive(Debug, Clone)]
struct ProjectTrustContext {
  project_root: PathBuf,
  project_root_key: String,
  repo_root_key: Option<String>,
  projects_trust: HashMap<String, TrustLevel>,
  user_config_file: PathBuf,
}

#[derive(Debug, Clone)]
struct ProjectTrustDecision {
  trust_level: Option<TrustLevel>,
  trust_key: String,
}

impl ProjectTrustDecision {
  fn is_trusted(&self) -> bool {
    matches!(self.trust_level, Some(TrustLevel::Trusted))
  }
}

impl ProjectTrustContext {
  fn decision_for_dir(&self, dir: &Path) -> ProjectTrustDecision {
    let dir_key = dir.to_string_lossy().to_string();
    if let Some(level) = self.projects_trust.get(&dir_key).copied() {
      return ProjectTrustDecision {
        trust_level: Some(level),
        trust_key: dir_key,
      };
    }

    if let Some(level) = self.projects_trust.get(&self.project_root_key).copied() {
      return ProjectTrustDecision {
        trust_level: Some(level),
        trust_key: self.project_root_key.clone(),
      };
    }

    if let Some(repo_root_key) = self.repo_root_key.as_ref() {
      if let Some(level) = self.projects_trust.get(repo_root_key).copied() {
        return ProjectTrustDecision {
          trust_level: Some(level),
          trust_key: repo_root_key.clone(),
        };
      }
    }

    ProjectTrustDecision {
      trust_level: None,
      trust_key: self
        .repo_root_key
        .clone()
        .unwrap_or_else(|| self.project_root_key.clone()),
    }
  }

  fn disabled_reason_for_dir(&self, dir: &Path) -> Option<String> {
    let decision = self.decision_for_dir(dir);
    if decision.is_trusted() {
      return None;
    }

    let trust_key = decision.trust_key.as_str();
    let user_config_file = self.user_config_file.display();
    match decision.trust_level {
      Some(TrustLevel::Untrusted) => Some(format!(
        "{trust_key} is marked as untrusted in {user_config_file}. To load config.toml, mark it trusted."
      )),
      _ => Some(format!(
        "To load config.toml, add {trust_key} as a trusted project in {user_config_file}."
      )),
    }
  }
}

/// Configuration loader with layered support
pub struct ConfigLoader {
  /// Global config directory
  global_dir: PathBuf,
  /// Working directory override (session cwd).
  cwd: Option<PathBuf>,
}

impl ConfigLoader {
  /// Create a new configuration loader
  pub fn new() -> Self {
    let global_dir = dirs::home_dir()
      .unwrap_or_else(|| PathBuf::from("."))
      .join(".cokra");

    Self {
      global_dir,
      cwd: None,
    }
  }

  /// Set working directory override (alias).
  pub fn with_project_dir(mut self, dir: PathBuf) -> Self {
    self.cwd = Some(dir);
    self
  }

  /// Set working directory override.
  pub fn with_cwd(mut self, dir: PathBuf) -> Self {
    self.cwd = Some(dir);
    self
  }

  /// Load configuration with CLI overrides
  pub fn load_with_cli_overrides(&self, cli_overrides: Vec<(String, String)>) -> Result<Config> {
    let (mut config, stack) = self.load_with_cli_overrides_and_stack(cli_overrides)?;
    config.config_layer_stack = Some(stack);
    Ok(config)
  }

  /// Load configuration and return both the effective config and the full layer stack.
  pub fn load_with_cli_overrides_and_stack(
    &self,
    cli_overrides: Vec<(String, String)>,
  ) -> Result<(Config, ConfigLayerStack)> {
    let resolved_cwd = self.resolve_cwd()?;
    let system_file = system_config_toml_file()?;
    let user_file = self.global_dir.join("config.toml");

    let mut layers = Vec::<ConfigLayerEntry>::new();

    // 1) Defaults (lowest precedence)
    let default_cfg = TomlValue::try_from(Config::default())?;
    layers.push(ConfigLayerEntry::new(
      ConfigLayerSource::Default,
      default_cfg,
    ));

    // 2) System config layer (exists or empty)
    layers.push(load_required_toml_layer(&system_file, |cfg| {
      ConfigLayerEntry::new(
        ConfigLayerSource::System {
          file: system_file.clone(),
        },
        cfg,
      )
    })?);

    // 3) User config layer (exists or empty)
    layers.push(load_required_toml_layer(&user_file, |cfg| {
      ConfigLayerEntry::new(
        ConfigLayerSource::User {
          file: user_file.clone(),
        },
        cfg,
      )
    })?);

    // Session flags layer content (built early because it can affect root markers and trust context)
    let session_flags_layer = if cli_overrides.is_empty() {
      None
    } else {
      Some(build_cli_overrides_layer(&cli_overrides)?)
    };

    // Build merged config so far to derive project_root_markers and trust decisions.
    let mut merged_so_far = TomlValue::Table(toml::map::Map::new());
    for layer in &layers {
      merge_toml_values(&mut merged_so_far, &layer.config);
    }
    if let Some(flags) = session_flags_layer.as_ref() {
      merge_toml_values(&mut merged_so_far, flags);
    }

    let markers = project_root_markers_from_config(&merged_so_far)?
      .unwrap_or_else(default_project_root_markers);
    let project_trust_context =
      project_trust_context(&merged_so_far, &resolved_cwd, &markers, &user_file)?;

    // 4) Project layers between project_root and cwd (inclusive).
    let project_layers = load_project_layers(
      &resolved_cwd,
      &project_trust_context.project_root,
      &project_trust_context,
      &self.global_dir,
    )?;
    layers.extend(project_layers);

    // 5) Session flags layer (highest precedence of the supported set here).
    if let Some(flags) = session_flags_layer {
      layers.push(ConfigLayerEntry::new(
        ConfigLayerSource::SessionFlags,
        flags,
      ));
    }

    let stack = ConfigLayerStack::new(layers);
    let merged = stack.effective_config();
    let mut config: Config = merged.clone().try_into()?;
    config.cwd = resolved_cwd;
    Ok((config, stack))
  }

  fn resolve_cwd(&self) -> Result<PathBuf> {
    match self.cwd.as_ref() {
      Some(p) => Ok(std::fs::canonicalize(p)?),
      None => Ok(std::env::current_dir()?),
    }
  }
}

impl Default for ConfigLoader {
  fn default() -> Self {
    Self::new()
  }
}

#[cfg(unix)]
fn system_config_toml_file() -> Result<PathBuf> {
  Ok(PathBuf::from("/etc/cokra/config.toml"))
}

#[cfg(windows)]
fn system_config_toml_file() -> Result<PathBuf> {
  // Best-effort parity with Codex's ProgramData layout. If resolution fails, fall back to the
  // legacy default.
  Ok(PathBuf::from(r"C:\ProgramData\Cokra\config.toml"))
}

fn load_required_toml_layer(
  file: &Path,
  create: impl FnOnce(TomlValue) -> ConfigLayerEntry,
) -> Result<ConfigLayerEntry> {
  let cfg = match std::fs::read_to_string(file) {
    Ok(contents) => toml::from_str::<TomlValue>(&contents)
      .map_err(|e| anyhow::anyhow!("Error parsing config file {}: {e}", file.display()))?,
    Err(e) => {
      if e.kind() == std::io::ErrorKind::NotFound {
        TomlValue::Table(toml::map::Map::new())
      } else {
        return Err(anyhow::anyhow!(
          "Failed to read config file {}: {e}",
          file.display()
        ));
      }
    }
  };

  Ok(create(cfg))
}

fn build_cli_overrides_layer(cli_overrides: &[(String, String)]) -> Result<TomlValue> {
  let mut root = TomlValue::Table(toml::map::Map::new());
  for (key, raw) in cli_overrides {
    set_dotted_key(&mut root, key, parse_override_value(raw));
  }
  Ok(root)
}

fn parse_override_value(raw: &str) -> TomlValue {
  let trimmed = raw.trim();
  if trimmed.eq_ignore_ascii_case("true") {
    return TomlValue::Boolean(true);
  }
  if trimmed.eq_ignore_ascii_case("false") {
    return TomlValue::Boolean(false);
  }
  if let Ok(v) = trimmed.parse::<i64>() {
    return TomlValue::Integer(v);
  }
  if let Ok(v) = trimmed.parse::<f64>() {
    return TomlValue::Float(v);
  }
  TomlValue::String(trimmed.to_string())
}

fn set_dotted_key(root: &mut TomlValue, dotted: &str, value: TomlValue) {
  let segments: Vec<&str> = dotted.split('.').filter(|s| !s.is_empty()).collect();
  if segments.is_empty() {
    return;
  }

  let mut cur = root;
  for (idx, seg) in segments.iter().enumerate() {
    let is_last = idx + 1 == segments.len();
    if is_last {
      if let TomlValue::Table(tbl) = cur {
        tbl.insert((*seg).to_string(), value);
      }
      return;
    }

    // Ensure table at this key
    if let TomlValue::Table(tbl) = cur {
      if !tbl.contains_key(*seg) {
        tbl.insert((*seg).to_string(), TomlValue::Table(toml::map::Map::new()));
      }
      cur = tbl.get_mut(*seg).unwrap();
    } else {
      return;
    }
  }
}

fn project_root_markers_from_config(config: &TomlValue) -> Result<Option<Vec<String>>> {
  let Some(table) = config.as_table() else {
    return Ok(None);
  };
  let Some(markers_value) = table.get("project_root_markers") else {
    return Ok(None);
  };
  let TomlValue::Array(entries) = markers_value else {
    return Err(anyhow::anyhow!(
      "project_root_markers must be an array of strings"
    ));
  };
  if entries.is_empty() {
    return Ok(Some(Vec::new()));
  }
  let mut markers = Vec::new();
  for entry in entries {
    let Some(marker) = entry.as_str() else {
      return Err(anyhow::anyhow!(
        "project_root_markers must be an array of strings"
      ));
    };
    markers.push(marker.to_string());
  }
  Ok(Some(markers))
}

fn default_project_root_markers() -> Vec<String> {
  DEFAULT_PROJECT_ROOT_MARKERS
    .iter()
    .map(ToString::to_string)
    .collect()
}

fn find_project_root(cwd: &Path, project_root_markers: &[String]) -> Result<PathBuf> {
  if project_root_markers.is_empty() {
    return Ok(cwd.to_path_buf());
  }

  for ancestor in cwd.ancestors() {
    for marker in project_root_markers {
      let marker_path = ancestor.join(marker);
      if std::fs::metadata(&marker_path).is_ok() {
        return Ok(ancestor.to_path_buf());
      }
    }
  }

  Ok(cwd.to_path_buf())
}

fn resolve_root_git_project_for_trust(cwd: &Path) -> Option<PathBuf> {
  let base = if cwd.is_dir() { cwd } else { cwd.parent()? };

  let out = std::process::Command::new("git")
    .args(["rev-parse", "--git-common-dir"])
    .current_dir(base)
    .output()
    .ok()?;
  if !out.status.success() {
    return None;
  }
  let git_dir_s = String::from_utf8(out.stdout).ok()?.trim().to_string();
  if git_dir_s.is_empty() {
    return None;
  }

  let git_dir_path_raw = {
    let candidate = PathBuf::from(&git_dir_s);
    if candidate.is_absolute() {
      candidate
    } else {
      base.join(candidate)
    }
  };

  let git_dir_path = std::fs::canonicalize(&git_dir_path_raw).unwrap_or(git_dir_path_raw);
  git_dir_path.parent().map(Path::to_path_buf)
}

fn project_trust_context(
  merged_config: &TomlValue,
  cwd: &Path,
  project_root_markers: &[String],
  user_config_file: &Path,
) -> Result<ProjectTrustContext> {
  let cfg: Config = merged_config.clone().try_into()?;
  let project_root = find_project_root(cwd, project_root_markers)?;

  let project_root_key = project_root.to_string_lossy().to_string();
  let repo_root_key =
    resolve_root_git_project_for_trust(cwd).map(|p| p.to_string_lossy().to_string());

  let projects_trust = cfg
    .projects
    .into_iter()
    .filter_map(|(key, project)| project.trust_level.map(|level| (key, level)))
    .collect::<HashMap<_, _>>();

  Ok(ProjectTrustContext {
    project_root,
    project_root_key,
    repo_root_key,
    projects_trust,
    user_config_file: user_config_file.to_path_buf(),
  })
}

fn project_layer_entry(
  trust_context: &ProjectTrustContext,
  dot_cokra_folder: &Path,
  layer_dir: &Path,
  config: TomlValue,
  config_toml_exists: bool,
) -> ConfigLayerEntry {
  let source = ConfigLayerSource::Project {
    dot_cokra_folder: dot_cokra_folder.to_path_buf(),
  };

  if config_toml_exists {
    if let Some(reason) = trust_context.disabled_reason_for_dir(layer_dir) {
      return ConfigLayerEntry::new_disabled(source, config, reason);
    }
  }

  ConfigLayerEntry::new(source, config)
}

fn load_project_layers(
  cwd: &Path,
  project_root: &Path,
  trust_context: &ProjectTrustContext,
  cokra_home: &Path,
) -> Result<Vec<ConfigLayerEntry>> {
  let cokra_home_abs =
    std::fs::canonicalize(cokra_home).unwrap_or_else(|_| cokra_home.to_path_buf());

  // Collect ancestors from cwd up to project_root (inclusive), then reverse to apply increasing precedence.
  let mut dirs = Vec::<&Path>::new();
  for ancestor in cwd.ancestors() {
    dirs.push(ancestor);
    if ancestor == project_root {
      break;
    }
  }
  dirs.reverse();

  let mut layers = Vec::new();
  for dir in dirs {
    let dot_cokra = dir.join(".cokra");
    let is_dir = std::fs::metadata(&dot_cokra)
      .map(|m| m.is_dir())
      .unwrap_or(false);
    if !is_dir {
      continue;
    }

    // Skip if this .cokra folder is the same as the global cokra home folder.
    let dot_cokra_abs = std::fs::canonicalize(&dot_cokra).unwrap_or(dot_cokra.clone());
    if dot_cokra_abs == cokra_home_abs {
      continue;
    }

    let decision = trust_context.decision_for_dir(dir);
    let config_file = dot_cokra.join("config.toml");
    match std::fs::read_to_string(&config_file) {
      Ok(contents) => {
        let config: TomlValue = match toml::from_str(&contents) {
          Ok(v) => v,
          Err(e) => {
            if decision.is_trusted() {
              return Err(anyhow::anyhow!(
                "Error parsing project config file {}: {e}",
                config_file.display()
              ));
            }
            layers.push(project_layer_entry(
              trust_context,
              &dot_cokra,
              dir,
              TomlValue::Table(toml::map::Map::new()),
              true,
            ));
            continue;
          }
        };
        layers.push(project_layer_entry(
          trust_context,
          &dot_cokra,
          dir,
          config,
          true,
        ));
      }
      Err(err) => {
        if err.kind() == std::io::ErrorKind::NotFound {
          layers.push(project_layer_entry(
            trust_context,
            &dot_cokra,
            dir,
            TomlValue::Table(toml::map::Map::new()),
            false,
          ));
        } else {
          return Err(anyhow::anyhow!(
            "Failed to read project config file {}: {err}",
            config_file.display()
          ));
        }
      }
    }
  }

  Ok(layers)
}
