use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Serialize;
use tokio::process::Command;
use tokio::sync::Mutex;

use super::LspError;

pub(crate) const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 15_000;
pub(crate) const DEFAULT_DIAGNOSTICS_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Clone, Serialize)]
pub struct LspAuditEvent {
  pub timestamp: String,
  pub event: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub server_id: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub root: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LspManagerConfig {
  pub enabled: bool,
  pub auto_install: bool,
  pub request_timeout: Duration,
  pub diagnostics_timeout: Duration,
}

impl Default for LspManagerConfig {
  fn default() -> Self {
    Self {
      enabled: std::env::var("COKRA_DISABLE_LSP").as_deref() != Ok("1"),
      auto_install: std::env::var("COKRA_DISABLE_LSP_INSTALL").as_deref() != Ok("1"),
      request_timeout: Duration::from_millis(DEFAULT_REQUEST_TIMEOUT_MS),
      diagnostics_timeout: Duration::from_millis(DEFAULT_DIAGNOSTICS_TIMEOUT_MS),
    }
  }
}

#[derive(Debug, Clone, Copy)]
enum LspInstallStrategy {
  GoInstall(&'static str),
  NpmExec {
    package: &'static str,
    binary: &'static str,
  },
  PipInstall(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LspServerDefinition {
  pub id: &'static str,
  pub program: &'static str,
  pub args: &'static [&'static str],
  pub language_id: &'static str,
  pub extensions: &'static [&'static str],
  pub root_markers: &'static [&'static str],
  pub exclude_markers: &'static [&'static str],
  install: Option<LspInstallStrategy>,
}

pub(crate) const LSP_SERVERS: &[LspServerDefinition] = &[
  LspServerDefinition {
    id: "rust-analyzer",
    program: "rust-analyzer",
    args: &[],
    language_id: "rust",
    extensions: &["rs"],
    root_markers: &["Cargo.toml", "rust-project.json", ".git"],
    exclude_markers: &[],
    install: None,
  },
  LspServerDefinition {
    id: "typescript-language-server",
    program: "typescript-language-server",
    args: &["--stdio"],
    language_id: "typescript",
    extensions: &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"],
    root_markers: &[
      "tsconfig.json",
      "jsconfig.json",
      "package.json",
      "pnpm-workspace.yaml",
      "package-lock.json",
      "yarn.lock",
      "bun.lock",
      ".git",
    ],
    exclude_markers: &["deno.json", "deno.jsonc"],
    install: Some(LspInstallStrategy::NpmExec {
      package: "typescript-language-server",
      binary: "typescript-language-server",
    }),
  },
  LspServerDefinition {
    id: "pyright-langserver",
    program: "pyright-langserver",
    args: &["--stdio"],
    language_id: "python",
    extensions: &["py", "pyi"],
    root_markers: &[
      "pyproject.toml",
      "pyrightconfig.json",
      "setup.py",
      "setup.cfg",
      "requirements.txt",
      "Pipfile",
      ".git",
    ],
    exclude_markers: &[],
    install: Some(LspInstallStrategy::NpmExec {
      package: "pyright",
      binary: "pyright-langserver",
    }),
  },
  LspServerDefinition {
    id: "gopls",
    program: "gopls",
    args: &[],
    language_id: "go",
    extensions: &["go"],
    root_markers: &["go.work", "go.mod", "go.sum", ".git"],
    exclude_markers: &[],
    install: Some(LspInstallStrategy::GoInstall("golang.org/x/tools/gopls")),
  },
  LspServerDefinition {
    id: "lua-language-server",
    program: "lua-language-server",
    args: &[],
    language_id: "lua",
    extensions: &["lua"],
    root_markers: &[".luarc.json", ".luarc.jsonc", ".git"],
    exclude_markers: &[],
    install: None,
  },
  LspServerDefinition {
    id: "clangd",
    program: "clangd",
    args: &[],
    language_id: "cpp",
    extensions: &["c", "cc", "cpp", "cxx", "h", "hpp"],
    root_markers: &[
      "compile_commands.json",
      "compile_flags.txt",
      ".clangd",
      "CMakeLists.txt",
      "meson.build",
      ".git",
    ],
    exclude_markers: &[],
    install: None,
  },
  LspServerDefinition {
    id: "ruby-lsp",
    program: "ruby-lsp",
    args: &[],
    language_id: "ruby",
    extensions: &["rb", "rake", "ru", "gemspec"],
    root_markers: &["Gemfile", ".ruby-version", ".git"],
    exclude_markers: &[],
    install: None,
  },
];

#[derive(Debug, Clone)]
pub(crate) struct ResolvedServer {
  pub definition: &'static LspServerDefinition,
  pub root: PathBuf,
  pub program: PathBuf,
  pub args: Vec<String>,
}

pub(crate) fn path_to_uri(path: &Path) -> String {
  reqwest::Url::from_file_path(path)
    .map(|url| url.to_string())
    .unwrap_or_else(|_| format!("file://{}", path.display()))
}

pub(crate) fn uri_to_path(uri: &str) -> Option<PathBuf> {
  reqwest::Url::parse(uri).ok()?.to_file_path().ok()
}

pub(crate) async fn canonicalize_existing_path(path: &Path) -> Result<PathBuf, LspError> {
  if !path.exists() {
    return Err(LspError::FileNotFound(path.display().to_string()));
  }
  Ok(
    tokio::fs::canonicalize(path)
      .await
      .unwrap_or_else(|_| path.to_path_buf()),
  )
}

pub(crate) fn detect_root(path: &Path, server: &LspServerDefinition) -> PathBuf {
  let start = path.parent().unwrap_or(path).to_path_buf();
  if contains_marker_upwards(&start, server.exclude_markers).is_some() {
    return start;
  }
  find_marker_root(&start, server.root_markers).unwrap_or(start)
}

pub(crate) async fn resolve_server_for_path(
  path: &Path,
  manager_config: &LspManagerConfig,
) -> Result<ResolvedServer, LspError> {
  let extension = path
    .extension()
    .and_then(|ext| ext.to_str())
    .unwrap_or_default()
    .to_ascii_lowercase();
  let definition = LSP_SERVERS
    .iter()
    .find(|server| {
      server
        .extensions
        .iter()
        .any(|candidate| *candidate == extension)
    })
    .ok_or_else(|| LspError::UnsupportedFile(path.display().to_string()))?;
  let root = detect_root(path, definition);

  if let Ok(program) = which::which(definition.program) {
    return Ok(ResolvedServer {
      definition,
      root,
      program,
      args: definition
        .args
        .iter()
        .map(|value| value.to_string())
        .collect(),
    });
  }

  if !manager_config.auto_install {
    push_audit_event(
      "auto_install_blocked",
      Some(definition.id),
      Some(root.display().to_string()),
      Some(format!("{} missing from PATH", definition.program)),
    )
    .await;
    return Err(LspError::ServerUnavailable(format!(
      "LSP server '{}' is not on PATH and auto-install is disabled",
      definition.program
    )));
  }

  let install = definition.install.ok_or_else(|| {
    LspError::ServerUnavailable(format!(
      "LSP server '{}' is not on PATH and no auto-install strategy is configured",
      definition.program
    ))
  })?;

  match install {
    LspInstallStrategy::GoInstall(package) => {
      push_audit_event(
        "auto_install_started",
        Some(definition.id),
        Some(root.display().to_string()),
        Some(format!("go install {package}@latest")),
      )
      .await;
      let go = which::which("go").map_err(|_| {
        LspError::ServerUnavailable(format!("go is required to install {}", definition.program))
      })?;
      let bin_dir = cokra_bin_dir();
      let _ = tokio::fs::create_dir_all(&bin_dir).await;
      let status = Command::new(go)
        .args(["install", &format!("{package}@latest")])
        .env("GOBIN", &bin_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|err| {
          LspError::ServerUnavailable(format!("failed to install {}: {err}", definition.program))
        })?;
      if !status.success() {
        push_audit_event(
          "auto_install_failed",
          Some(definition.id),
          Some(root.display().to_string()),
          Some(format!("go install {package}@latest")),
        )
        .await;
        return Err(LspError::ServerUnavailable(format!(
          "failed to install {}",
          definition.program
        )));
      }
      Ok(ResolvedServer {
        definition,
        root,
        program: bin_dir.join(format!(
          "{}{}",
          definition.program,
          if cfg!(windows) { ".exe" } else { "" }
        )),
        args: definition
          .args
          .iter()
          .map(|value| value.to_string())
          .collect(),
      })
    }
    LspInstallStrategy::NpmExec { package, binary } => {
      push_audit_event(
        "launcher_resolved",
        Some(definition.id),
        Some(root.display().to_string()),
        Some(format!("npx exec --yes {package} -- {binary}")),
      )
      .await;
      let program = which::which("npx")
        .or_else(|_| which::which("npm"))
        .map_err(|_| {
          LspError::ServerUnavailable(format!(
            "npx or npm is required to launch {}",
            definition.program
          ))
        })?;
      let mut args = vec![
        "exec".to_string(),
        "--yes".to_string(),
        package.to_string(),
        "--".to_string(),
        binary.to_string(),
      ];
      args.extend(definition.args.iter().map(|value| value.to_string()));
      Ok(ResolvedServer {
        definition,
        root,
        program,
        args,
      })
    }
    LspInstallStrategy::PipInstall(package) => {
      push_audit_event(
        "auto_install_started",
        Some(definition.id),
        Some(root.display().to_string()),
        Some(format!("pip install --user --quiet {package}")),
      )
      .await;
      let pip = which::which("pip3")
        .or_else(|_| which::which("pip"))
        .map_err(|_| {
          LspError::ServerUnavailable(format!("pip is required to install {}", definition.program))
        })?;
      let status = Command::new(pip)
        .args(["install", "--user", "--quiet", package])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|err| {
          LspError::ServerUnavailable(format!("failed to install {}: {err}", definition.program))
        })?;
      if !status.success() {
        push_audit_event(
          "auto_install_failed",
          Some(definition.id),
          Some(root.display().to_string()),
          Some(format!("pip install --user --quiet {package}")),
        )
        .await;
        return Err(LspError::ServerUnavailable(format!(
          "failed to install {}",
          definition.program
        )));
      }
      Ok(ResolvedServer {
        definition,
        root,
        program: which::which(definition.program).map_err(|_| {
          LspError::ServerUnavailable(format!(
            "installed {}, but '{}' is still not on PATH",
            package, definition.program
          ))
        })?,
        args: definition
          .args
          .iter()
          .map(|value| value.to_string())
          .collect(),
      })
    }
  }
}

fn audit_log() -> &'static Mutex<Vec<LspAuditEvent>> {
  static LSP_AUDIT_LOG: OnceLock<Mutex<Vec<LspAuditEvent>>> = OnceLock::new();
  LSP_AUDIT_LOG.get_or_init(|| Mutex::new(Vec::new()))
}

pub(crate) async fn push_audit_event(
  event: &str,
  server_id: Option<&str>,
  root: Option<String>,
  detail: Option<String>,
) {
  let mut audit_log = audit_log().lock().await;
  audit_log.push(LspAuditEvent {
    timestamp: chrono::Utc::now().to_rfc3339(),
    event: event.to_string(),
    server_id: server_id.map(ToString::to_string),
    root,
    detail,
  });
  if audit_log.len() > 200 {
    let drain = audit_log.len() - 200;
    audit_log.drain(..drain);
  }
}

pub async fn recent_audit_events(limit: usize) -> Vec<LspAuditEvent> {
  let audit_log = audit_log().lock().await;
  let mut events = audit_log
    .iter()
    .rev()
    .take(limit.max(1))
    .cloned()
    .collect::<Vec<_>>();
  events.reverse();
  events
}

fn cokra_bin_dir() -> PathBuf {
  dirs::home_dir()
    .unwrap_or_else(std::env::temp_dir)
    .join(".cokra")
    .join("bin")
}

fn find_marker_root(start: &Path, markers: &[&str]) -> Option<PathBuf> {
  for directory in start.ancestors() {
    if markers.iter().any(|marker| directory.join(marker).exists()) {
      return Some(directory.to_path_buf());
    }
  }
  None
}

fn contains_marker_upwards(start: &Path, markers: &[&str]) -> Option<PathBuf> {
  for directory in start.ancestors() {
    if markers.iter().any(|marker| directory.join(marker).exists()) {
      return Some(directory.to_path_buf());
    }
  }
  None
}

#[cfg(test)]
mod tests {
  use super::*;
  use tempfile::tempdir;

  #[test]
  fn detect_root_prefers_nearest_marker() {
    let dir = tempdir().expect("tempdir");
    let workspace = dir.path().join("workspace");
    let nested = workspace.join("nested");
    std::fs::create_dir_all(&nested).expect("create nested");
    std::fs::write(workspace.join("Cargo.toml"), "[package]\nname = \"demo\"\n").expect("write");
    let file = nested.join("main.rs");
    std::fs::write(&file, "fn main() {}\n").expect("write");

    let server = LSP_SERVERS
      .iter()
      .find(|server| server.id == "rust-analyzer")
      .expect("rust analyzer");
    assert_eq!(detect_root(&file, server), workspace);
  }

  #[test]
  fn detect_root_falls_back_to_parent_directory() {
    let dir = tempdir().expect("tempdir");
    let file = dir.path().join("foo.py");
    std::fs::write(&file, "print('ok')\n").expect("write");

    let server = LspServerDefinition {
      id: "fallback-test",
      program: "fallback-test",
      args: &[],
      language_id: "python",
      extensions: &["py"],
      root_markers: &["__no_workspace_marker__"],
      exclude_markers: &[],
      install: None,
    };
    assert_eq!(detect_root(&file, &server), dir.path());
  }

  #[test]
  fn uri_round_trip_preserves_file_path() {
    let dir = tempdir().expect("tempdir");
    let file = dir.path().join("sample.rs");
    std::fs::write(&file, "fn main() {}\n").expect("write");

    let uri = path_to_uri(&file);
    assert_eq!(uri_to_path(&uri).as_deref(), Some(file.as_path()));
  }
}
