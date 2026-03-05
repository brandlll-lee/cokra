//! Command display utilities - 1:1 port from codex-rs/tui/src/exec_command.rs
//!
//! This module provides command formatting functions for display in the TUI.

use shlex::try_join;

/// Escape a command for display, joining arguments with proper shell escaping.
///
/// 1:1 port from codex-rs/tui/src/exec_command.rs
pub(crate) fn escape_command(command: &[String]) -> String {
  try_join(command.iter().map(String::as_str)).unwrap_or_else(|_| command.join(" "))
}

/// Strip `bash -lc "..."` or `zsh -lc "..."` wrapper from command for display.
///
/// This extracts the actual script from a shell invocation like:
/// - `["bash", "-lc", "echo hello"]` -> `"echo hello"`
/// - `["/bin/bash", "-c", "ls -la"]` -> `"ls -la"`
///
/// 1:1 port from codex-rs/tui/src/exec_command.rs
pub(crate) fn strip_bash_lc_and_escape(command: &[String]) -> String {
  if let Some((_, script)) = extract_shell_command(command) {
    return script.to_string();
  }
  escape_command(command)
}

/// Extract the shell script from a `bash -lc "..."` or `zsh -lc "..."` invocation.
///
/// Returns `Some((shell, script))` if the command matches the pattern,
/// otherwise returns `None`.
fn extract_shell_command(command: &[String]) -> Option<(&str, &str)> {
  let [shell, flag, script] = command else {
    return None;
  };

  // Check if flag is -lc or -c
  if !matches!(flag.as_str(), "-lc" | "-c") {
    return None;
  }

  // Check if shell is bash, zsh, sh, or a path to one of them
  let shell_name = if shell.contains('/') {
    shell.rsplit('/').next()?
  } else {
    shell.as_str()
  };

  if !matches!(shell_name, "bash" | "zsh" | "sh") {
    return None;
  }

  Some((shell.as_str(), script.as_str()))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_escape_command() {
    let args = vec!["foo".into(), "bar baz".into(), "weird&stuff".into()];
    let cmdline = escape_command(&args);
    assert_eq!(cmdline, "foo 'bar baz' 'weird&stuff'");
  }

  #[test]
  fn test_strip_bash_lc_and_escape() {
    // Test bash
    let args = vec!["bash".into(), "-lc".into(), "echo hello".into()];
    let cmdline = strip_bash_lc_and_escape(&args);
    assert_eq!(cmdline, "echo hello");

    // Test zsh
    let args = vec!["zsh".into(), "-lc".into(), "echo hello".into()];
    let cmdline = strip_bash_lc_and_escape(&args);
    assert_eq!(cmdline, "echo hello");

    // Test absolute path to zsh
    let args = vec!["/usr/bin/zsh".into(), "-lc".into(), "echo hello".into()];
    let cmdline = strip_bash_lc_and_escape(&args);
    assert_eq!(cmdline, "echo hello");

    // Test absolute path to bash
    let args = vec!["/bin/bash".into(), "-lc".into(), "echo hello".into()];
    let cmdline = strip_bash_lc_and_escape(&args);
    assert_eq!(cmdline, "echo hello");

    // Test -c flag (without l)
    let args = vec!["bash".into(), "-c".into(), "ls -la".into()];
    let cmdline = strip_bash_lc_and_escape(&args);
    assert_eq!(cmdline, "ls -la");

    // Test non-shell command (returns escaped)
    let args = vec!["echo".into(), "hello world".into()];
    let cmdline = strip_bash_lc_and_escape(&args);
    assert_eq!(cmdline, "echo 'hello world'");

    // Test wrong flag (returns escaped)
    let args = vec!["bash".into(), "-l".into(), "echo hello".into()];
    let cmdline = strip_bash_lc_and_escape(&args);
    assert_eq!(cmdline, "bash -l 'echo hello'");
  }
}
