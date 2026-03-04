//! 1:1 codex: Shell detection and command argument derivation.
//!
//! Dynamically detects the user's default shell and provides cross-platform
//! command argument construction. Replaces hardcoded `bash` invocations.

use std::collections::HashMap;
use std::path::PathBuf;

/// Supported shell types.
///
/// 1:1 codex: `codex-rs/core/src/shell.rs` ShellType enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellType {
  Zsh,
  Bash,
  Sh,
  PowerShell,
  Cmd,
}

/// A resolved shell binary with its type and absolute path.
///
/// 1:1 codex: `codex-rs/core/src/shell.rs` Shell struct (without shell_snapshot).
#[derive(Debug, Clone)]
pub struct Shell {
  pub shell_type: ShellType,
  pub shell_path: PathBuf,
}

/// Environment inheritance policy for shell command execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InheritMode {
  All,
  None,
  Selected(Vec<String>),
}

/// Shell environment policy used to build model/tool command env maps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellEnvironmentPolicy {
  pub inherit: InheritMode,
  pub set: HashMap<String, String>,
  pub unset: Vec<String>,
}

impl Default for ShellEnvironmentPolicy {
  fn default() -> Self {
    Self {
      inherit: InheritMode::All,
      set: HashMap::new(),
      unset: Vec::new(),
    }
  }
}

/// Build command environment according to shell policy.
///
/// Inserts `COKRA_THREAD_ID` when provided.
pub fn create_env(
  policy: &ShellEnvironmentPolicy,
  thread_id: Option<&str>,
) -> HashMap<String, String> {
  let mut env = match &policy.inherit {
    InheritMode::All => std::env::vars().collect::<HashMap<_, _>>(),
    InheritMode::None => HashMap::new(),
    InheritMode::Selected(keys) => {
      let mut selected = HashMap::new();
      for key in keys {
        if let Ok(value) = std::env::var(key) {
          selected.insert(key.clone(), value);
        }
      }
      selected
    }
  };

  for key in &policy.unset {
    env.remove(key);
  }

  env.extend(policy.set.clone());

  if let Some(thread_id) = thread_id {
    env.insert("COKRA_THREAD_ID".to_string(), thread_id.to_string());
  }

  env
}

impl Shell {
  /// Shell display name.
  pub fn name(&self) -> &'static str {
    match self.shell_type {
      ShellType::Zsh => "zsh",
      ShellType::Bash => "bash",
      ShellType::PowerShell => "powershell",
      ShellType::Sh => "sh",
      ShellType::Cmd => "cmd",
    }
  }

  /// 1:1 codex: Shell::derive_exec_args()
  ///
  /// Takes a command string and returns the full argument list for spawning
  /// a child process that executes the command under this shell.
  pub fn derive_exec_args(&self, command: &str, use_login_shell: bool) -> Vec<String> {
    match self.shell_type {
      ShellType::Zsh | ShellType::Bash | ShellType::Sh => {
        let flag = if use_login_shell { "-lc" } else { "-c" };
        vec![
          self.shell_path.to_string_lossy().to_string(),
          flag.to_string(),
          command.to_string(),
        ]
      }
      ShellType::PowerShell => {
        let mut args = vec![self.shell_path.to_string_lossy().to_string()];
        if !use_login_shell {
          args.push("-NoProfile".to_string());
        }
        args.push("-Command".to_string());
        args.push(command.to_string());
        args
      }
      ShellType::Cmd => {
        vec![
          self.shell_path.to_string_lossy().to_string(),
          "/c".to_string(),
          command.to_string(),
        ]
      }
    }
  }
}

/// 1:1 codex: detect_shell_type() — infer ShellType from a path's file stem.
fn detect_shell_type(path: &PathBuf) -> Option<ShellType> {
  let stem = path.file_stem()?.to_string_lossy().to_lowercase();
  match stem.as_str() {
    "zsh" => Some(ShellType::Zsh),
    "bash" => Some(ShellType::Bash),
    "sh" => Some(ShellType::Sh),
    "pwsh" | "powershell" => Some(ShellType::PowerShell),
    "cmd" => Some(ShellType::Cmd),
    _ => {
      let name = path.file_name()?.to_string_lossy().to_lowercase();
      if name.contains("powershell") || name.contains("pwsh") {
        Some(ShellType::PowerShell)
      } else {
        None
      }
    }
  }
}

/// 1:1 codex: get_user_shell_path() — read default shell from /etc/passwd via getpwuid.
#[cfg(unix)]
fn get_user_shell_path() -> Option<PathBuf> {
  use std::ffi::CStr;

  unsafe {
    let uid = libc::getuid();
    let pw = libc::getpwuid(uid);
    if pw.is_null() {
      return None;
    }
    let shell = CStr::from_ptr((*pw).pw_shell)
      .to_string_lossy()
      .into_owned();
    Some(PathBuf::from(shell))
  }
}

#[cfg(not(unix))]
fn get_user_shell_path() -> Option<PathBuf> {
  None
}

/// 1:1 codex: file_exists validation helper.
fn file_exists(path: &PathBuf) -> bool {
  std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

/// 1:1 codex: get_shell_path() — four-step fallback resolution.
///
/// 1. Explicit provided path
/// 2. User's default shell (if it matches requested type)
/// 3. `which` lookup by binary name
/// 4. Hardcoded fallback paths
fn get_shell_path(
  shell_type: ShellType,
  provided_path: Option<&PathBuf>,
  binary_name: &str,
  fallback_paths: &[&str],
) -> Option<PathBuf> {
  // Step 1: explicit path
  if let Some(p) = provided_path
    && file_exists(p)
  {
    return Some(p.clone());
  }

  // Step 2: match against user's default shell
  if let Some(default) = get_user_shell_path()
    && detect_shell_type(&default) == Some(shell_type.clone())
    && file_exists(&default)
  {
    return Some(default);
  }

  // Step 3: which lookup
  if !binary_name.is_empty()
    && let Ok(p) = which::which(binary_name)
  {
    return Some(p);
  }

  // Step 4: hardcoded fallback paths
  for &path in fallback_paths {
    let p = PathBuf::from(path);
    if file_exists(&p) {
      return Some(p);
    }
  }

  None
}

/// 1:1 codex: get_shell() — resolve a specific shell type with correct binary_name and fallbacks.
pub fn get_shell(shell_type: ShellType, path: Option<&PathBuf>) -> Option<Shell> {
  match shell_type {
    ShellType::Zsh => get_shell_path(ShellType::Zsh, path, "zsh", &["/bin/zsh"]).map(|p| Shell {
      shell_type: ShellType::Zsh,
      shell_path: p,
    }),
    ShellType::Bash => {
      get_shell_path(ShellType::Bash, path, "bash", &["/bin/bash"]).map(|p| Shell {
        shell_type: ShellType::Bash,
        shell_path: p,
      })
    }
    ShellType::Sh => get_shell_path(ShellType::Sh, path, "sh", &["/bin/sh"]).map(|p| Shell {
      shell_type: ShellType::Sh,
      shell_path: p,
    }),
    ShellType::PowerShell => get_shell_path(
      ShellType::PowerShell,
      path,
      "pwsh",
      &["/usr/local/bin/pwsh"],
    )
    .or_else(|| get_shell_path(ShellType::PowerShell, path, "powershell", &[]))
    .map(|p| Shell {
      shell_type: ShellType::PowerShell,
      shell_path: p,
    }),
    ShellType::Cmd => get_shell_path(ShellType::Cmd, path, "cmd", &[]).map(|p| Shell {
      shell_type: ShellType::Cmd,
      shell_path: p,
    }),
  }
}

/// 1:1 codex: ultimate_fallback_shell() — absolute last resort.
fn ultimate_fallback_shell() -> Shell {
  if cfg!(windows) {
    Shell {
      shell_type: ShellType::Cmd,
      shell_path: PathBuf::from("cmd.exe"),
    }
  } else {
    Shell {
      shell_type: ShellType::Sh,
      shell_path: PathBuf::from("/bin/sh"),
    }
  }
}

/// 1:1 codex: default_user_shell() — main entry point for shell detection.
///
/// Windows: PowerShell (pwsh → powershell) → cmd.exe
/// macOS: user default → zsh → bash → sh
/// Linux: user default → bash → zsh → sh
pub fn default_user_shell() -> Shell {
  default_user_shell_from_path(get_user_shell_path())
}

/// 1:1 codex: default_user_shell_from_path() — testable inner implementation.
fn default_user_shell_from_path(user_shell_path: Option<PathBuf>) -> Shell {
  if cfg!(windows) {
    // Windows: prefer pwsh, fallback powershell, fallback cmd
    if let Some(path) = get_shell_path(ShellType::PowerShell, None, "pwsh", &[]) {
      return Shell {
        shell_type: ShellType::PowerShell,
        shell_path: path,
      };
    }
    if let Some(path) = get_shell_path(ShellType::PowerShell, None, "powershell", &[]) {
      return Shell {
        shell_type: ShellType::PowerShell,
        shell_path: path,
      };
    }
    return ultimate_fallback_shell();
  }

  // Unix: resolve from user's default shell
  // 1:1 codex: detect type from user's shell path, then resolve via get_shell()
  // which uses correct binary_name + fallback_paths for each shell type.
  let user_default = user_shell_path
    .as_ref()
    .and_then(detect_shell_type)
    .and_then(|t| get_shell(t, None));

  if let Some(shell) = user_default {
    return shell;
  }

  // 1:1 codex: macOS prefers zsh → bash; Linux prefers bash → zsh
  let shell_with_fallback = if cfg!(target_os = "macos") {
    get_shell(ShellType::Zsh, None).or_else(|| get_shell(ShellType::Bash, None))
  } else {
    get_shell(ShellType::Bash, None).or_else(|| get_shell(ShellType::Zsh, None))
  };

  shell_with_fallback.unwrap_or(ultimate_fallback_shell())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn detect_shell_type_from_common_paths() {
    assert_eq!(
      detect_shell_type(&PathBuf::from("/bin/bash")),
      Some(ShellType::Bash)
    );
    assert_eq!(
      detect_shell_type(&PathBuf::from("/bin/zsh")),
      Some(ShellType::Zsh)
    );
    assert_eq!(
      detect_shell_type(&PathBuf::from("/bin/sh")),
      Some(ShellType::Sh)
    );
    assert_eq!(
      detect_shell_type(&PathBuf::from("pwsh.exe")),
      Some(ShellType::PowerShell)
    );
    assert_eq!(
      detect_shell_type(&PathBuf::from("powershell.exe")),
      Some(ShellType::PowerShell)
    );
    assert_eq!(
      detect_shell_type(&PathBuf::from("cmd.exe")),
      Some(ShellType::Cmd)
    );
    assert_eq!(detect_shell_type(&PathBuf::from("fish")), None);
    assert_eq!(detect_shell_type(&PathBuf::from("nu")), None);
  }

  #[test]
  fn derive_exec_args_bash_login() {
    let shell = Shell {
      shell_type: ShellType::Bash,
      shell_path: PathBuf::from("/bin/bash"),
    };
    assert_eq!(
      shell.derive_exec_args("echo hello", true),
      vec!["/bin/bash", "-lc", "echo hello"]
    );
  }

  #[test]
  fn derive_exec_args_bash_no_login() {
    let shell = Shell {
      shell_type: ShellType::Bash,
      shell_path: PathBuf::from("/bin/bash"),
    };
    assert_eq!(
      shell.derive_exec_args("echo hello", false),
      vec!["/bin/bash", "-c", "echo hello"]
    );
  }

  #[test]
  fn derive_exec_args_powershell_login() {
    let shell = Shell {
      shell_type: ShellType::PowerShell,
      shell_path: PathBuf::from("pwsh.exe"),
    };
    assert_eq!(
      shell.derive_exec_args("echo hello", true),
      vec!["pwsh.exe", "-Command", "echo hello"]
    );
  }

  #[test]
  fn derive_exec_args_powershell_no_login() {
    let shell = Shell {
      shell_type: ShellType::PowerShell,
      shell_path: PathBuf::from("pwsh.exe"),
    };
    assert_eq!(
      shell.derive_exec_args("echo hello", false),
      vec!["pwsh.exe", "-NoProfile", "-Command", "echo hello"]
    );
  }

  #[test]
  fn derive_exec_args_cmd() {
    let shell = Shell {
      shell_type: ShellType::Cmd,
      shell_path: PathBuf::from("cmd.exe"),
    };
    assert_eq!(
      shell.derive_exec_args("echo hello", false),
      vec!["cmd.exe", "/c", "echo hello"]
    );
  }

  #[test]
  fn derive_exec_args_zsh_login() {
    let shell = Shell {
      shell_type: ShellType::Zsh,
      shell_path: PathBuf::from("/bin/zsh"),
    };
    assert_eq!(
      shell.derive_exec_args("echo hello", true),
      vec!["/bin/zsh", "-lc", "echo hello"]
    );
  }

  #[test]
  fn ultimate_fallback_produces_valid_shell() {
    let shell = ultimate_fallback_shell();
    if cfg!(windows) {
      assert_eq!(shell.shell_type, ShellType::Cmd);
    } else {
      assert_eq!(shell.shell_type, ShellType::Sh);
    }
  }

  #[test]
  fn default_user_shell_returns_a_shell() {
    let shell = default_user_shell();
    // Just ensure it doesn't panic and returns something.
    assert!(!shell.shell_path.as_os_str().is_empty());
  }
}
