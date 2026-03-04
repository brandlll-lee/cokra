//! 1:1 codex: Unified command execution layer.
//!
//! All external process spawning MUST go through this module.
//! No handler should directly call `Command::new(...).spawn()`.
//!
//! Pipeline: ExecParams → SandboxManager::transform() → execute_command()

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::truncate::TruncationPolicy;
use crate::truncate::formatted_truncate_text;

// ---------------------------------------------------------------------------
// Constants (1:1 codex)
// ---------------------------------------------------------------------------

/// 1:1 codex: EXEC_OUTPUT_MAX_BYTES = 1 MiB
pub const EXEC_OUTPUT_MAX_BYTES: usize = 1024 * 1024;

/// 1:1 codex: timeout exit code convention
pub const EXEC_TIMEOUT_EXIT_CODE: i32 = 124;

/// 1:1 codex: DEFAULT_EXEC_COMMAND_TIMEOUT_MS (10 seconds)
pub const DEFAULT_EXEC_COMMAND_TIMEOUT_MS: u64 = 10_000;

/// 1:1 codex: IO drain timeout after process kill (2 seconds)
const IO_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// Sandbox permissions (minimal viable enum, Spec 1.1)
// ---------------------------------------------------------------------------

/// Sandbox permission level for a command execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SandboxPermissions {
  /// Normal sandbox restrictions apply.
  #[default]
  Default,
  /// Escalated permissions (e.g. after user approval on sandbox denial).
  RequireEscalated,
}

/// Format exec output into codex-style structured payload for model consumption.
pub fn format_exec_output_for_model_structured(
  exec_output: &ExecToolCallOutput,
  truncation_policy: TruncationPolicy,
) -> String {
  #[derive(Serialize)]
  struct ExecMetadata {
    exit_code: i32,
    duration_seconds: f32,
  }

  #[derive(Serialize)]
  struct ExecOutput<'a> {
    output: &'a str,
    metadata: ExecMetadata,
  }

  let duration_seconds = (exec_output.duration.as_secs_f32() * 10.0).round() / 10.0;
  let content = build_content_with_timeout(exec_output);
  let output = formatted_truncate_text(&content, truncation_policy);

  let payload = ExecOutput {
    output: &output,
    metadata: ExecMetadata {
      exit_code: exec_output.exit_code,
      duration_seconds,
    },
  };

  serde_json::to_string(&payload).unwrap_or_else(|_| {
        format!(
            "{{\"output\":\"serialization_error\",\"metadata\":{{\"exit_code\":{},\"duration_seconds\":{}}}}}",
            exec_output.exit_code, duration_seconds
        )
    })
}

/// Windows sandbox level placeholder (fixed Disabled for now).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowsSandboxLevel {
  #[default]
  Disabled,
}

// ---------------------------------------------------------------------------
// ExecExpiration (1:1 codex)
// ---------------------------------------------------------------------------

/// How a command execution should be terminated.
#[derive(Debug, Clone, Default)]
pub enum ExecExpiration {
  /// Hard timeout after the given duration.
  Timeout(Duration),
  /// Use `DEFAULT_EXEC_COMMAND_TIMEOUT_MS`.
  #[default]
  DefaultTimeout,
  /// Cancel via a `CancellationToken`.
  Cancellation(CancellationToken),
}

impl ExecExpiration {
  /// Resolve to a concrete `Duration`.
  pub fn as_duration(&self) -> Duration {
    match self {
      ExecExpiration::Timeout(d) => *d,
      ExecExpiration::DefaultTimeout => Duration::from_millis(DEFAULT_EXEC_COMMAND_TIMEOUT_MS),
      ExecExpiration::Cancellation(_) => Duration::from_secs(3600),
    }
  }
}

// ---------------------------------------------------------------------------
// ExecParams (1:1 codex fields)
// ---------------------------------------------------------------------------

/// Unified parameters for spawning an external command.
///
/// Every tool that needs to run an external process MUST construct an
/// `ExecParams` and pass it through `execute_command()`.
#[derive(Debug, Clone)]
pub struct ExecParams {
  /// Full argv (program + arguments).
  pub command: Vec<String>,
  /// Working directory for the child process.
  pub cwd: PathBuf,
  /// Expiration / timeout strategy.
  pub expiration: ExecExpiration,
  /// Extra environment variables merged into the child.
  pub env: HashMap<String, String>,
  /// Network proxy placeholder (reserved for future sandbox integration).
  pub network: Option<()>,
  /// Network approval attempt id (for managed network requirements).
  pub network_attempt_id: Option<String>,
  /// Sandbox permission level.
  pub sandbox_permissions: SandboxPermissions,
  /// Windows sandbox level (fixed Disabled for now).
  pub windows_sandbox_level: WindowsSandboxLevel,
  /// Human-readable justification for why the command is being run.
  pub justification: Option<String>,
  /// Override argv[0] display name.
  pub arg0: Option<String>,
}

impl Default for ExecParams {
  fn default() -> Self {
    Self {
      command: Vec::new(),
      cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
      expiration: ExecExpiration::DefaultTimeout,
      env: HashMap::new(),
      network: None,
      network_attempt_id: None,
      sandbox_permissions: SandboxPermissions::Default,
      windows_sandbox_level: WindowsSandboxLevel::Disabled,
      justification: None,
      arg0: None,
    }
  }
}

// ---------------------------------------------------------------------------
// StreamOutput (1:1 codex)
// ---------------------------------------------------------------------------

/// Capped string output with truncation tracking.
#[derive(Debug, Clone, Default)]
pub struct StreamOutput {
  pub text: String,
  pub truncated: bool,
}

// ---------------------------------------------------------------------------
// ExecToolCallOutput (1:1 codex)
// ---------------------------------------------------------------------------

/// Result of executing an external command.
#[derive(Debug, Clone)]
pub struct ExecToolCallOutput {
  pub exit_code: i32,
  pub stdout: StreamOutput,
  pub stderr: StreamOutput,
  pub aggregated_output: StreamOutput,
  pub duration: Duration,
  pub timed_out: bool,
}

// ---------------------------------------------------------------------------
// ExecError
// ---------------------------------------------------------------------------

/// Errors produced by the exec layer.
///
/// Distinguishes "spawn failure" from "command ran but returned non-zero".
#[derive(Debug, Clone)]
pub enum ExecError {
  /// The process could not be spawned at all (e.g. binary not found).
  SpawnFailed {
    message: String,
    os_error: Option<i32>,
  },
  /// The command executed but the sandbox denied it.
  SandboxDenied { output: String },
  /// General execution error.
  Other(String),
}

impl std::fmt::Display for ExecError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      ExecError::SpawnFailed { message, .. } => write!(f, "{message}"),
      ExecError::SandboxDenied { output } => write!(f, "{output}"),
      ExecError::Other(msg) => write!(f, "{msg}"),
    }
  }
}

impl std::error::Error for ExecError {}

// ---------------------------------------------------------------------------
// execute_command() — the single entry point for all process spawning
// ---------------------------------------------------------------------------

/// Execute a command described by `ExecParams`.
///
/// This is the **only** function in cokra that should call
/// `tokio::process::Command::spawn()`. All tools must go through here.
///
/// Behavior (1:1 codex):
/// - stdin is always null (prevent hanging on stdin read)
/// - stdout/stderr are piped and drained concurrently
/// - output is capped at `EXEC_OUTPUT_MAX_BYTES`
/// - timeout → kill process group + exit code 124
/// - IO drain has a secondary 2s timeout after kill (prevent hang)
/// - kill_on_drop ensures child is terminated if cokra exits
pub async fn execute_command(params: &ExecParams) -> Result<ExecToolCallOutput, ExecError> {
  let (program, prog_args) = params
    .command
    .split_first()
    .ok_or_else(|| ExecError::Other("empty command".to_string()))?;

  // 1:1 codex: sanitize program and args — strip NUL bytes that would
  // cause Command::spawn() to fail with "nul byte found in provided data".
  let program_clean = program.replace('\0', "");
  let args_clean: Vec<String> = prog_args.iter().map(|a| a.replace('\0', "")).collect();

  // Also sanitize cwd — current_dir internally converts to CString.
  let cwd_clean = PathBuf::from(params.cwd.to_string_lossy().replace('\0', ""));

  let mut cmd = Command::new(&program_clean);
  cmd.args(&args_clean);
  cmd.current_dir(&cwd_clean);

  // 1:1 codex: stdin null to prevent commands from hanging on stdin read.
  cmd.stdin(std::process::Stdio::null());
  cmd.stdout(std::process::Stdio::piped());
  cmd.stderr(std::process::Stdio::piped());

  // 1:1 codex: kill_on_drop ensures child is terminated if Cokra exits.
  cmd.kill_on_drop(true);

  // NOTE: codex does env_clear() + cmd.envs(env) because its upper layer
  // (create_env + ShellEnvironmentPolicy) builds a complete env HashMap.
  // cokra doesn't have that infrastructure yet, so we inherit the parent
  // process env and only merge ExecParams.env on top. NUL bytes in the
  // extra env are stripped to prevent spawn failures.
  for (k, v) in &params.env {
    cmd.env(k.replace('\0', ""), v.replace('\0', ""));
  }

  let start = Instant::now();

  let mut child = cmd.spawn().map_err(|e| {
    let os_error = e.raw_os_error();
    ExecError::SpawnFailed {
      message: format!("command failed to start: {e} (program: {program})"),
      os_error,
    }
  })?;

  let timeout = params.expiration.as_duration();

  // 1:1 codex: take ownership of stdout/stderr handles before the select.
  let stdout_handle = child.stdout.take();
  let stderr_handle = child.stderr.take();

  // 1:1 codex: concurrent stdout+stderr drain with timeout via tokio::select!
  let drain_result = tokio::select! {
      result = drain_and_wait(&mut child, stdout_handle, stderr_handle) => {
          Ok(result)
      }
      _ = expiration_future(&params.expiration, timeout) => {
          // 1:1 codex: kill on timeout
          kill_child(&mut child).await;
          Err(())
      }
  };

  let duration = start.elapsed();

  match drain_result {
    Err(()) => {
      // Timed out — return exit_code 124
      let timeout_msg = format!("Command timed out after {}ms", timeout.as_millis());
      Ok(ExecToolCallOutput {
        exit_code: EXEC_TIMEOUT_EXIT_CODE,
        stdout: StreamOutput::default(),
        stderr: StreamOutput {
          text: timeout_msg.clone(),
          truncated: false,
        },
        aggregated_output: StreamOutput {
          text: timeout_msg,
          truncated: false,
        },
        duration,
        timed_out: true,
      })
    }
    Ok((status_result, stdout_bytes, stderr_bytes)) => {
      let exit_code = match status_result {
        Ok(status) => status.code().unwrap_or(-1),
        Err(e) => {
          return Err(ExecError::Other(format!(
            "failed to wait on child process: {e}"
          )));
        }
      };

      let stdout_out = cap_stream_output(&stdout_bytes);
      let stderr_out = cap_stream_output(&stderr_bytes);
      let aggregated_output = build_aggregated_output(&stdout_bytes, &stderr_bytes);

      Ok(ExecToolCallOutput {
        exit_code,
        stdout: stdout_out,
        stderr: stderr_out,
        aggregated_output,
        duration,
        timed_out: false,
      })
    }
  }
}

// ---------------------------------------------------------------------------
// Formatting helpers (1:1 codex output format)
// ---------------------------------------------------------------------------

/// Format an `ExecToolCallOutput` into the standard tool output string.
///
/// Format:
/// ```text
/// exit_code: <code>
/// stdout:
/// <stdout text>
/// stderr:
/// <stderr text>
/// ```
pub fn format_exec_output(output: &ExecToolCallOutput) -> String {
  let mut content = format!("exit_code: {}\n", output.exit_code);
  if !output.stdout.text.is_empty() {
    content.push_str("stdout:\n");
    content.push_str(&output.stdout.text);
    if !content.ends_with('\n') {
      content.push('\n');
    }
  }
  if !output.stderr.text.is_empty() {
    content.push_str("stderr:\n");
    content.push_str(&output.stderr.text);
    if !content.ends_with('\n') {
      content.push('\n');
    }
  }
  content
}

/// Format an `ExecError` into the standard error string.
pub fn format_exec_error(error: &ExecError) -> String {
  match error {
    ExecError::SpawnFailed { message, .. } => {
      // Note: message already contains the os error from std::io::Error Display,
      // so we don't append os_error again to avoid duplication.
      message.clone()
    }
    ExecError::SandboxDenied { output } => output.clone(),
    ExecError::Other(msg) => msg.clone(),
  }
}

fn build_content_with_timeout(exec_output: &ExecToolCallOutput) -> String {
  if exec_output.timed_out {
    format!(
      "command timed out after {} milliseconds\n{}",
      exec_output.duration.as_millis(),
      exec_output.aggregated_output.text
    )
  } else {
    exec_output.aggregated_output.text.clone()
  }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Drain stdout + stderr concurrently, then wait for child exit.
async fn drain_and_wait(
  child: &mut tokio::process::Child,
  stdout_handle: Option<tokio::process::ChildStdout>,
  stderr_handle: Option<tokio::process::ChildStderr>,
) -> (
  Result<std::process::ExitStatus, std::io::Error>,
  Vec<u8>,
  Vec<u8>,
) {
  let mut out_bytes = Vec::new();
  let mut err_bytes = Vec::new();

  let drain_out = async {
    if let Some(mut reader) = stdout_handle {
      let mut buf = vec![0u8; 8192];
      loop {
        match reader.read(&mut buf).await {
          Ok(0) => break,
          Ok(n) => {
            out_bytes.extend_from_slice(&buf[..n]);
            if out_bytes.len() >= EXEC_OUTPUT_MAX_BYTES {
              break;
            }
          }
          Err(_) => break,
        }
      }
    }
  };

  let drain_err = async {
    if let Some(mut reader) = stderr_handle {
      let mut buf = vec![0u8; 8192];
      loop {
        match reader.read(&mut buf).await {
          Ok(0) => break,
          Ok(n) => {
            err_bytes.extend_from_slice(&buf[..n]);
            if err_bytes.len() >= EXEC_OUTPUT_MAX_BYTES {
              break;
            }
          }
          Err(_) => break,
        }
      }
    }
  };

  tokio::join!(drain_out, drain_err);

  let status = child.wait().await;
  (status, out_bytes, err_bytes)
}

/// Future that resolves when the expiration triggers.
async fn expiration_future(expiration: &ExecExpiration, timeout: Duration) {
  match expiration {
    ExecExpiration::Cancellation(token) => {
      token.cancelled().await;
    }
    _ => {
      tokio::time::sleep(timeout).await;
    }
  }
}

/// Kill a child process. On Unix, attempt to kill the process group first.
async fn kill_child(child: &mut tokio::process::Child) {
  #[cfg(unix)]
  {
    // 1:1 codex: kill process group to prevent orphaned grandchildren
    if let Some(pid) = child.id() {
      unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
      }
    }
  }

  let _ = child.start_kill();

  // 1:1 codex: secondary IO drain timeout (2s) to prevent hang
  let _ = tokio::time::timeout(IO_DRAIN_TIMEOUT, child.wait()).await;
}

/// Cap raw bytes to `EXEC_OUTPUT_MAX_BYTES` and convert to `StreamOutput`.
fn cap_stream_output(bytes: &[u8]) -> StreamOutput {
  if bytes.len() > EXEC_OUTPUT_MAX_BYTES {
    StreamOutput {
      text: String::from_utf8_lossy(&bytes[..EXEC_OUTPUT_MAX_BYTES]).into_owned(),
      truncated: true,
    }
  } else {
    StreamOutput {
      text: String::from_utf8_lossy(bytes).into_owned(),
      truncated: false,
    }
  }
}

/// 1:1 codex: build aggregated output with stdout 1/3, stderr 2/3 rebalancing.
fn build_aggregated_output(stdout_bytes: &[u8], stderr_bytes: &[u8]) -> StreamOutput {
  let total = stdout_bytes.len() + stderr_bytes.len();
  if total <= EXEC_OUTPUT_MAX_BYTES {
    let mut text = String::new();
    if !stdout_bytes.is_empty() {
      text.push_str(&String::from_utf8_lossy(stdout_bytes));
    }
    if !stderr_bytes.is_empty() {
      if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
      }
      text.push_str(&String::from_utf8_lossy(stderr_bytes));
    }
    StreamOutput {
      text,
      truncated: false,
    }
  } else {
    // 1:1 codex: stdout gets 1/3, stderr gets 2/3
    let stdout_budget = EXEC_OUTPUT_MAX_BYTES / 3;
    let stderr_budget = EXEC_OUTPUT_MAX_BYTES - stdout_budget;

    let stdout_slice = &stdout_bytes[..stdout_bytes.len().min(stdout_budget)];
    let stderr_slice = &stderr_bytes[..stderr_bytes.len().min(stderr_budget)];

    let mut text = String::new();
    if !stdout_slice.is_empty() {
      text.push_str(&String::from_utf8_lossy(stdout_slice));
    }
    if !stderr_slice.is_empty() {
      if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
      }
      text.push_str(&String::from_utf8_lossy(stderr_slice));
    }
    StreamOutput {
      text,
      truncated: true,
    }
  }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn exec_expiration_default_resolves() {
    let exp = ExecExpiration::DefaultTimeout;
    assert_eq!(
      exp.as_duration(),
      Duration::from_millis(DEFAULT_EXEC_COMMAND_TIMEOUT_MS)
    );
  }

  #[test]
  fn exec_expiration_timeout_resolves() {
    let exp = ExecExpiration::Timeout(Duration::from_secs(5));
    assert_eq!(exp.as_duration(), Duration::from_secs(5));
  }

  #[test]
  fn cap_stream_output_no_truncation() {
    let bytes = b"hello world";
    let out = cap_stream_output(bytes);
    assert_eq!(out.text, "hello world");
    assert!(!out.truncated);
  }

  #[test]
  fn cap_stream_output_truncation() {
    let bytes = vec![b'A'; EXEC_OUTPUT_MAX_BYTES + 100];
    let out = cap_stream_output(&bytes);
    assert_eq!(out.text.len(), EXEC_OUTPUT_MAX_BYTES);
    assert!(out.truncated);
  }

  #[test]
  fn aggregated_output_small() {
    let stdout = b"out";
    let stderr = b"err";
    let agg = build_aggregated_output(stdout, stderr);
    assert!(agg.text.contains("out"));
    assert!(agg.text.contains("err"));
    assert!(!agg.truncated);
  }

  #[test]
  fn aggregated_output_large_rebalances() {
    let stdout = vec![b'O'; EXEC_OUTPUT_MAX_BYTES];
    let stderr = vec![b'E'; EXEC_OUTPUT_MAX_BYTES];
    let agg = build_aggregated_output(&stdout, &stderr);
    assert!(agg.truncated);
    // stdout gets ~1/3, stderr gets ~2/3
    let stdout_portion = agg.text.matches('O').count();
    let stderr_portion = agg.text.matches('E').count();
    assert!(stdout_portion <= EXEC_OUTPUT_MAX_BYTES / 3 + 1);
    assert!(stderr_portion <= (EXEC_OUTPUT_MAX_BYTES * 2 / 3) + 1);
  }

  #[test]
  fn format_exec_output_basic() {
    let output = ExecToolCallOutput {
      exit_code: 0,
      stdout: StreamOutput {
        text: "hello\n".to_string(),
        truncated: false,
      },
      stderr: StreamOutput::default(),
      aggregated_output: StreamOutput::default(),
      duration: Duration::from_millis(100),
      timed_out: false,
    };
    let formatted = format_exec_output(&output);
    assert!(formatted.starts_with("exit_code: 0\n"));
    assert!(formatted.contains("stdout:\nhello\n"));
  }

  #[test]
  fn format_exec_error_spawn() {
    // message from execute_command already includes os error via std::io::Error Display
    let err = ExecError::SpawnFailed {
      message:
        "command failed to start: No such file or directory (os error 2) (program: /bin/bash)"
          .to_string(),
      os_error: Some(2),
    };
    let formatted = format_exec_error(&err);
    assert!(formatted.contains("No such file or directory"));
    assert!(formatted.contains("(program: /bin/bash)"));
    // os_error is NOT appended again — it's already in the message
    assert_eq!(formatted.matches("os error 2").count(), 1);
  }

  #[test]
  fn format_exec_output_for_model_structured_contains_metadata() {
    let output = ExecToolCallOutput {
      exit_code: 7,
      stdout: StreamOutput::default(),
      stderr: StreamOutput::default(),
      aggregated_output: StreamOutput {
        text: "sample output".to_string(),
        truncated: false,
      },
      duration: Duration::from_millis(1520),
      timed_out: false,
    };

    let formatted = format_exec_output_for_model_structured(&output, TruncationPolicy::Tokens(100));
    let parsed: serde_json::Value = serde_json::from_str(&formatted).expect("valid json");

    assert_eq!(parsed["metadata"]["exit_code"], 7);
    assert_eq!(parsed["metadata"]["duration_seconds"], 1.5);
    assert_eq!(parsed["output"], "sample output");
  }
}
