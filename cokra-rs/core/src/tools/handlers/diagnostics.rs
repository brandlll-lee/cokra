//! diagnostics tool handler — get LSP diagnostics for a file.
//!
//! Architecture (1:1 opencode lsp.ts pattern, Rust adaptation):
//! - Spawns a language server process based on file extension (language detection).
//! - Sends `initialize` → `initialized` → `textDocument/didOpen` via JSON-RPC stdio.
//! - Collects `textDocument/publishDiagnostics` notifications (debounced 3s).
//! - Returns formatted diagnostics: ERROR/WARN/INFO/HINT [line:col] message.
//!
//! Supported language servers (auto-detected by extension, must be on PATH):
//!   .rs  → rust-analyzer
//!   .ts/.tsx/.js/.jsx → typescript-language-server --stdio
//!   .py  → pylsp / pyright-langserver --stdio
//!   .go  → gopls
//!   .lua → lua-language-server
//!
//! If no LSP is found for the file type, returns a clear error.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::time::timeout;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct DiagnosticsHandler;

const DIAGNOSTICS_TIMEOUT_SECS: u64 = 30;
const NOTIFICATION_COLLECT_SECS: u64 = 5;

// ── LSP JSON-RPC types ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct LspRequest {
  jsonrpc: &'static str,
  id: u32,
  method: String,
  params: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct LspNotification {
  jsonrpc: &'static str,
  method: String,
  params: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct LspMessage {
  method: Option<String>,
  params: Option<serde_json::Value>,
  id: Option<serde_json::Value>,
  result: Option<serde_json::Value>,
  error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct PublishDiagnosticsParams {
  uri: String,
  diagnostics: Vec<LspDiagnostic>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LspDiagnostic {
  pub message: String,
  pub range: LspRange,
  pub severity: Option<u8>,
  pub code: Option<serde_json::Value>,
  pub source: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LspRange {
  pub start: LspPosition,
  pub end: LspPosition,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LspPosition {
  pub line: u32,
  pub character: u32,
}

// ── Tool args ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct DiagnosticsArgs {
  /// Absolute or cwd-relative path to the file.
  path: String,
  /// Maximum number of diagnostics to return (default: 50).
  #[serde(default = "default_max_diagnostics")]
  max_diagnostics: usize,
}

fn default_max_diagnostics() -> usize {
  50
}

// ── Language server detection ─────────────────────────────────────────────────

struct LspConfig {
  /// Primary LSP binary name.
  program: &'static str,
  /// Arguments passed to the LSP binary on spawn.
  args: &'static [&'static str],
  /// Optional auto-install strategy when the binary is not on PATH.
  install: Option<LspInstall>,
}

/// Describes how to auto-install an LSP server (mirrors opencode server.ts tier-2).
#[allow(dead_code)]
enum LspInstall {
  /// `go install <pkg>@latest` with GOBIN pointing to cokra's bin dir.
  GoInstall(&'static str),
  /// `npx --yes <pkg> -- <args>` — downloads on first call, no permanent install.
  NpmExec(&'static str),
  /// `pip install --user <pkg>` then retry PATH lookup.
  PipInstall(&'static str),
}

fn lsp_for_extension(ext: &str) -> Option<LspConfig> {
  match ext {
    "rs" => Some(LspConfig {
      program: "rust-analyzer",
      args: &[],
      install: None,
    }),
    "ts" | "tsx" | "js" | "jsx" | "mts" | "cts" | "mjs" | "cjs" => Some(LspConfig {
      program: "typescript-language-server",
      args: &["--stdio"],
      install: Some(LspInstall::NpmExec("typescript-language-server")),
    }),
    "py" | "pyi" => Some(LspConfig {
      program: "pyright-langserver",
      args: &["--stdio"],
      install: Some(LspInstall::NpmExec("pyright")),
    }),
    "go" => Some(LspConfig {
      program: "gopls",
      args: &[],
      install: Some(LspInstall::GoInstall("golang.org/x/tools/gopls")),
    }),
    "lua" => Some(LspConfig {
      program: "lua-language-server",
      args: &[],
      install: None,
    }),
    "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" => Some(LspConfig {
      program: "clangd",
      args: &[],
      install: None,
    }),
    "rb" | "rake" => Some(LspConfig {
      program: "ruby-lsp",
      args: &[],
      install: None,
    }),
    _ => None,
  }
}

/// Resolve the LSP binary path using three tiers (mirrors opencode server.ts):
///
///   Tier 1 — binary already on PATH → return it immediately.
///   Tier 2 — binary absent but auto-install strategy available → install silently.
///   Tier 3 — nothing works → return None (caller skips diagnostics gracefully).
///
/// Set `COKRA_DISABLE_LSP_INSTALL=1` to disable tier-2 (opt-out like opencode's flag).
async fn resolve_lsp_binary(config: &LspConfig) -> Option<std::path::PathBuf> {
  // Tier 1: already on PATH
  if let Ok(p) = which::which(config.program) {
    return Some(p);
  }

  // Honour opt-out env var
  if std::env::var("COKRA_DISABLE_LSP_INSTALL").as_deref() == Ok("1") {
    return None;
  }

  let install = config.install.as_ref()?;

  match install {
    LspInstall::GoInstall(pkg) => {
      // Require `go` runtime — don't force-install Go itself.
      if which::which("go").is_err() {
        return None;
      }
      let gobin = cokra_bin_dir();
      let _ = std::fs::create_dir_all(&gobin);
      let ok = tokio::process::Command::new("go")
        .args(["install", &format!("{pkg}@latest")])
        .env("GOBIN", &gobin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);
      if !ok {
        return None;
      }
      let bin_name = config.program.to_string() + if cfg!(windows) { ".exe" } else { "" };
      let installed = gobin.join(bin_name);
      installed.exists().then_some(installed)
    }

    LspInstall::NpmExec(pkg) => {
      // npx downloads the package on first run without permanently installing it.
      // If `npx` is available we use it; otherwise fall back to `npm exec`.
      let npx = which::which("npx").or_else(|_| which::which("npm")).ok()?;
      let version_ok = tokio::process::Command::new(&npx)
        .args(["exec", "--yes", pkg, "--", "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);
      // Return the npx binary path — run_lsp_session will prepend "exec --yes <pkg>"
      version_ok.then_some(npx)
    }

    LspInstall::PipInstall(pkg) => {
      let pip = which::which("pip3").or_else(|_| which::which("pip")).ok()?;
      let ok = tokio::process::Command::new(&pip)
        .args(["install", "--user", "--quiet", pkg])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);
      if !ok {
        return None;
      }
      which::which(config.program).ok()
    }
  }
}

/// Returns `~/.cokra/bin` as the private auto-install directory.
fn cokra_bin_dir() -> std::path::PathBuf {
  dirs::home_dir()
    .unwrap_or_else(std::env::temp_dir)
    .join(".cokra")
    .join("bin")
}

// ── Handler ───────────────────────────────────────────────────────────────────

#[async_trait]
impl ToolHandler for DiagnosticsHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _: &ToolInvocation) -> bool {
    false
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let id = invocation.id.clone();
    let args: DiagnosticsArgs = invocation.parse_arguments()?;

    let file_path = invocation.resolve_path(Some(&args.path));
    let file_path = tokio::fs::canonicalize(&file_path)
      .await
      .unwrap_or(file_path);

    if !file_path.exists() {
      return Err(FunctionCallError::RespondToModel(format!(
        "file not found: {}",
        file_path.display()
      )));
    }

    let ext = file_path
      .extension()
      .and_then(|e| e.to_str())
      .unwrap_or("")
      .to_lowercase();

    let lsp_config = lsp_for_extension(&ext).ok_or_else(|| {
      FunctionCallError::RespondToModel(format!("no LSP server known for .{ext} files"))
    })?;

    let lsp_binary = resolve_lsp_binary(&lsp_config).await.ok_or_else(|| {
      FunctionCallError::RespondToModel(format!(
        "LSP server '{}' not found and could not be auto-installed for .{ext} files. \
         Install it manually or set COKRA_DISABLE_LSP_INSTALL=0.",
        lsp_config.program
      ))
    })?;

    let root_uri = workspace_uri(&file_path);
    let file_uri = path_to_uri(&file_path);

    let file_content = tokio::fs::read_to_string(&file_path)
      .await
      .map_err(|e| FunctionCallError::Execution(format!("failed to read file: {e}")))?;

    let diagnostics = timeout(
      Duration::from_secs(DIAGNOSTICS_TIMEOUT_SECS),
      run_lsp_session(lsp_config, lsp_binary, &root_uri, &file_uri, &file_content),
    )
    .await
    .map_err(|_| {
      FunctionCallError::RespondToModel(format!(
        "LSP diagnostics timed out after {DIAGNOSTICS_TIMEOUT_SECS}s"
      ))
    })??;

    if diagnostics.is_empty() {
      return Ok(
        ToolOutput::success(format!("No diagnostics for {}", file_path.display())).with_id(id),
      );
    }

    let limited: Vec<_> = diagnostics.into_iter().take(args.max_diagnostics).collect();

    let output = format_diagnostics(&file_path, &limited);
    Ok(ToolOutput::success(output).with_id(id))
  }
}

// ── LSP session ───────────────────────────────────────────────────────────────

async fn run_lsp_session(
  config: LspConfig,
  binary: std::path::PathBuf,
  root_uri: &str,
  file_uri: &str,
  file_content: &str,
) -> Result<Vec<LspDiagnostic>, FunctionCallError> {
  use tokio::process::Command;

  // NpmExec: binary is the npx/npm path; prepend "exec --yes <pkg> --" before config.args.
  // Direct: binary is the LSP binary itself; args are passed as-is.
  let is_npm_exec = matches!(config.install, Some(LspInstall::NpmExec(_)));
  let mut child = if is_npm_exec {
    let pkg = match &config.install {
      Some(LspInstall::NpmExec(p)) => *p,
      _ => unreachable!(),
    };
    let mut cmd = Command::new(&binary);
    cmd.args(["exec", "--yes", pkg, "--"]);
    cmd.args(config.args);
    cmd
      .stdin(std::process::Stdio::piped())
      .stdout(std::process::Stdio::piped())
      .stderr(std::process::Stdio::null())
      .kill_on_drop(true)
      .spawn()
      .map_err(|e| FunctionCallError::Execution(format!("failed to spawn npx {pkg}: {e}")))?
  } else {
    Command::new(&binary)
      .args(config.args)
      .stdin(std::process::Stdio::piped())
      .stdout(std::process::Stdio::piped())
      .stderr(std::process::Stdio::null())
      .kill_on_drop(true)
      .spawn()
      .map_err(|e| {
        FunctionCallError::Execution(format!("failed to spawn {}: {e}", binary.display()))
      })?
  };

  let mut stdin = child
    .stdin
    .take()
    .ok_or_else(|| FunctionCallError::Execution("failed to get LSP stdin".to_string()))?;

  let stdout = child
    .stdout
    .take()
    .ok_or_else(|| FunctionCallError::Execution("failed to get LSP stdout".to_string()))?;

  // Send initialize request
  let init_id = 1u32;
  send_request(
    &mut stdin,
    init_id,
    "initialize",
    serde_json::json!({
      "processId": std::process::id(),
      "rootUri": root_uri,
      "capabilities": {
        "textDocument": {
          "publishDiagnostics": {
            "relatedInformation": true,
            "versionSupport": false
          }
        },
        "workspace": {
          "configuration": false,
          "didChangeWatchedFiles": { "dynamicRegistration": false }
        }
      },
      "clientInfo": { "name": "cokra", "version": env!("CARGO_PKG_VERSION") }
    }),
  )
  .await?;

  let mut reader = BufReader::new(stdout);
  let mut id_counter = init_id + 1;

  // Wait for initialize response
  let _init_result = wait_for_response(&mut reader, init_id).await?;

  // Send initialized notification
  send_notification(&mut stdin, "initialized", serde_json::json!({})).await?;

  // Send textDocument/didOpen
  let language_id = language_id_for_uri(file_uri);
  send_notification(
    &mut stdin,
    "textDocument/didOpen",
    serde_json::json!({
      "textDocument": {
        "uri": file_uri,
        "languageId": language_id,
        "version": 1,
        "text": file_content
      }
    }),
  )
  .await?;

  // Collect publishDiagnostics notifications for a bounded window
  let diagnostics = collect_diagnostics(&mut reader, file_uri, id_counter).await;
  id_counter += 1;

  // Shutdown gracefully (best-effort, ignore errors)
  let _ = send_request(&mut stdin, id_counter, "shutdown", serde_json::json!(null)).await;
  let _ = send_notification(&mut stdin, "exit", serde_json::json!(null)).await;
  let _ = child.kill().await;

  Ok(diagnostics)
}

async fn collect_diagnostics(
  reader: &mut BufReader<tokio::process::ChildStdout>,
  file_uri: &str,
  _id: u32,
) -> Vec<LspDiagnostic> {
  let mut result: Vec<LspDiagnostic> = Vec::new();

  // Collect for up to NOTIFICATION_COLLECT_SECS seconds
  let _ = timeout(Duration::from_secs(NOTIFICATION_COLLECT_SECS), async {
    loop {
      match read_lsp_message(reader).await {
        Ok(msg) => {
          if msg.method.as_deref() == Some("textDocument/publishDiagnostics") {
            if let Some(params) = msg.params {
              if let Ok(diag_params) = serde_json::from_value::<PublishDiagnosticsParams>(params) {
                if diag_params.uri == file_uri {
                  result = diag_params.diagnostics;
                }
              }
            }
          }
          // Keep reading until timeout
        }
        Err(_) => break,
      }
    }
  })
  .await;

  result
}

// ── LSP I/O helpers ───────────────────────────────────────────────────────────

async fn send_request(
  stdin: &mut tokio::process::ChildStdin,
  id: u32,
  method: &str,
  params: serde_json::Value,
) -> Result<(), FunctionCallError> {
  let req = LspRequest {
    jsonrpc: "2.0",
    id,
    method: method.to_string(),
    params,
  };
  let body = serde_json::to_string(&req)
    .map_err(|e| FunctionCallError::Execution(format!("LSP serialise error: {e}")))?;
  let msg = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
  stdin
    .write_all(msg.as_bytes())
    .await
    .map_err(|e| FunctionCallError::Execution(format!("LSP write error: {e}")))
}

async fn send_notification(
  stdin: &mut tokio::process::ChildStdin,
  method: &str,
  params: serde_json::Value,
) -> Result<(), FunctionCallError> {
  let notif = LspNotification {
    jsonrpc: "2.0",
    method: method.to_string(),
    params,
  };
  let body = serde_json::to_string(&notif)
    .map_err(|e| FunctionCallError::Execution(format!("LSP serialise error: {e}")))?;
  let msg = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
  stdin
    .write_all(msg.as_bytes())
    .await
    .map_err(|e| FunctionCallError::Execution(format!("LSP write error: {e}")))
}

async fn read_lsp_message(
  reader: &mut BufReader<tokio::process::ChildStdout>,
) -> Result<LspMessage, FunctionCallError> {
  // Read headers until blank line
  let mut content_length: Option<usize> = None;
  loop {
    let mut line = String::new();
    let n = reader
      .read_line(&mut line)
      .await
      .map_err(|e| FunctionCallError::Execution(format!("LSP read header error: {e}")))?;
    if n == 0 {
      return Err(FunctionCallError::Execution("LSP EOF".to_string()));
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
      break;
    }
    if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
      content_length = rest.trim().parse().ok();
    }
  }

  let len = content_length.ok_or_else(|| {
    FunctionCallError::Execution("missing Content-Length in LSP message".to_string())
  })?;

  let mut buf = vec![0u8; len];
  tokio::io::AsyncReadExt::read_exact(reader, &mut buf)
    .await
    .map_err(|e| FunctionCallError::Execution(format!("LSP read body error: {e}")))?;

  serde_json::from_slice::<LspMessage>(&buf)
    .map_err(|e| FunctionCallError::Execution(format!("LSP parse error: {e}")))
}

async fn wait_for_response(
  reader: &mut BufReader<tokio::process::ChildStdout>,
  id: u32,
) -> Result<serde_json::Value, FunctionCallError> {
  loop {
    let msg = read_lsp_message(reader).await?;
    if let Some(msg_id) = &msg.id {
      if msg_id == &serde_json::json!(id) {
        if let Some(err) = msg.error {
          return Err(FunctionCallError::Execution(format!(
            "LSP error response: {err}"
          )));
        }
        return Ok(msg.result.unwrap_or(serde_json::Value::Null));
      }
    }
    // Skip notifications and other messages while waiting
  }
}

// ── Formatting ────────────────────────────────────────────────────────────────

fn format_diagnostics(file_path: &Path, diagnostics: &[LspDiagnostic]) -> String {
  let mut lines = vec![format!(
    "Diagnostics for {} ({} found):\n",
    file_path.display(),
    diagnostics.len()
  )];

  for d in diagnostics {
    let severity = match d.severity {
      Some(1) => "ERROR",
      Some(2) => "WARN",
      Some(3) => "INFO",
      Some(4) => "HINT",
      _ => "ERROR",
    };
    let line = d.range.start.line + 1;
    let col = d.range.start.character + 1;
    let source = d
      .source
      .as_deref()
      .map(|s| format!("[{s}] "))
      .unwrap_or_default();
    lines.push(format!("{severity} [{line}:{col}] {source}{}", d.message));
  }

  lines.join("\n")
}

// ── URI helpers ───────────────────────────────────────────────────────────────

fn path_to_uri(path: &Path) -> String {
  let path_str = path.to_string_lossy();
  if cfg!(windows) {
    // Windows: /C:/path/to/file
    format!("file:///{}", path_str.replace('\\', "/"))
  } else {
    format!("file://{path_str}")
  }
}

fn workspace_uri(file_path: &Path) -> String {
  let root = file_path.parent().unwrap_or(file_path);
  path_to_uri(root)
}

fn language_id_for_uri(uri: &str) -> &'static str {
  if uri.ends_with(".rs") {
    "rust"
  } else if uri.ends_with(".ts")
    || uri.ends_with(".tsx")
    || uri.ends_with(".mts")
    || uri.ends_with(".cts")
  {
    "typescript"
  } else if uri.ends_with(".js")
    || uri.ends_with(".jsx")
    || uri.ends_with(".mjs")
    || uri.ends_with(".cjs")
  {
    "javascript"
  } else if uri.ends_with(".py") {
    "python"
  } else if uri.ends_with(".go") {
    "go"
  } else if uri.ends_with(".lua") {
    "lua"
  } else if uri.ends_with(".c") || uri.ends_with(".h") {
    "c"
  } else if uri.ends_with(".cpp")
    || uri.ends_with(".cc")
    || uri.ends_with(".cxx")
    || uri.ends_with(".hpp")
  {
    "cpp"
  } else {
    "plaintext"
  }
}

// ── Public helper for edit_file / write_file ──────────────────────────────────

/// Run LSP diagnostics on `path` after a write operation.
/// Returns a formatted suffix string to append to the tool output.
/// Returns an empty string if:
///   - no LSP is configured for the file type
///   - the LSP binary is not on PATH
///   - diagnostics timed out or failed (silent)
pub async fn collect_file_diagnostics(path: &std::path::Path) -> String {
  let ext = path
    .extension()
    .and_then(|e| e.to_str())
    .unwrap_or("")
    .to_lowercase();

  let Some(lsp_config) = lsp_for_extension(&ext) else {
    return String::new();
  };

  let Some(binary) = resolve_lsp_binary(&lsp_config).await else {
    return String::new();
  };

  let file_content = match tokio::fs::read_to_string(path).await {
    Ok(c) => c,
    Err(_) => return String::new(),
  };

  let root_uri = workspace_uri(path);
  let file_uri = path_to_uri(path);

  let result = tokio::time::timeout(
    std::time::Duration::from_secs(NOTIFICATION_COLLECT_SECS + 5),
    run_lsp_session(lsp_config, binary, &root_uri, &file_uri, &file_content),
  )
  .await;

  match result {
    Ok(Ok(diags)) if !diags.is_empty() => {
      let formatted = format_diagnostics(path, &diags);
      format!("\n\n{formatted}")
    }
    _ => String::new(),
  }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn path_to_uri_unix_style() {
    if !cfg!(windows) {
      let p = PathBuf::from("/home/user/file.rs");
      assert_eq!(path_to_uri(&p), "file:///home/user/file.rs");
    }
  }

  #[test]
  fn language_id_for_rust() {
    assert_eq!(language_id_for_uri("file:///foo/bar.rs"), "rust");
  }

  #[test]
  fn language_id_for_typescript() {
    assert_eq!(language_id_for_uri("file:///foo/bar.ts"), "typescript");
    assert_eq!(language_id_for_uri("file:///foo/bar.tsx"), "typescript");
  }

  #[test]
  fn language_id_for_python() {
    assert_eq!(language_id_for_uri("file:///foo/bar.py"), "python");
  }

  #[test]
  fn language_id_for_go() {
    assert_eq!(language_id_for_uri("file:///foo/bar.go"), "go");
  }

  #[test]
  fn lsp_for_rs_extension() {
    let config = lsp_for_extension("rs").unwrap();
    assert_eq!(config.program, "rust-analyzer");
  }

  #[test]
  fn lsp_for_ts_extension() {
    let config = lsp_for_extension("ts").unwrap();
    assert_eq!(config.program, "typescript-language-server");
  }

  #[test]
  fn lsp_unknown_extension_returns_none() {
    assert!(lsp_for_extension("xyz123").is_none());
  }

  #[test]
  fn format_diagnostics_empty() {
    let path = PathBuf::from("/foo/bar.rs");
    let result = format_diagnostics(&path, &[]);
    assert!(result.contains("0 found"));
  }

  #[test]
  fn format_diagnostics_with_entries() {
    let path = PathBuf::from("/foo/bar.rs");
    let diags = vec![
      LspDiagnostic {
        message: "unused variable".to_string(),
        range: LspRange {
          start: LspPosition {
            line: 4,
            character: 7,
          },
          end: LspPosition {
            line: 4,
            character: 12,
          },
        },
        severity: Some(2),
        code: None,
        source: Some("rustc".to_string()),
      },
      LspDiagnostic {
        message: "type mismatch".to_string(),
        range: LspRange {
          start: LspPosition {
            line: 9,
            character: 0,
          },
          end: LspPosition {
            line: 9,
            character: 5,
          },
        },
        severity: Some(1),
        code: None,
        source: None,
      },
    ];
    let result = format_diagnostics(&path, &diags);
    assert!(result.contains("WARN [5:8] [rustc] unused variable"));
    assert!(result.contains("ERROR [10:1] type mismatch"));
  }

  #[test]
  fn default_max_diagnostics_is_50() {
    let args: DiagnosticsArgs = serde_json::from_str(r#"{"path":"/foo/bar.rs"}"#).unwrap();
    assert_eq!(args.max_diagnostics, 50);
  }

  #[tokio::test]
  async fn rejects_nonexistent_file() {
    use crate::tools::context::ToolPayload;
    let inv = ToolInvocation {
      id: "t1".to_string(),
      name: "diagnostics".to_string(),
      payload: ToolPayload::Function {
        arguments: r#"{"path":"/nonexistent/file.rs"}"#.to_string(),
      },
      cwd: std::env::temp_dir(),
      runtime: None,
    };
    let err = DiagnosticsHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("not found"));
  }

  #[tokio::test]
  async fn rejects_unknown_extension() {
    use crate::tools::context::ToolPayload;
    use tempfile::NamedTempFile;
    let tmp = NamedTempFile::with_suffix(".unknown123").unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    let inv = ToolInvocation {
      id: "t2".to_string(),
      name: "diagnostics".to_string(),
      payload: ToolPayload::Function {
        arguments: serde_json::json!({"path": path}).to_string(),
      },
      cwd: std::env::temp_dir(),
      runtime: None,
    };
    let err = DiagnosticsHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("no LSP server"));
  }
}
