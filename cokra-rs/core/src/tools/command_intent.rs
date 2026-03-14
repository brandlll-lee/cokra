use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandMutationClass {
  ReadOnly,
  WritesFiles,
  ChangesPermissions,
  Destructive,
  Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PathIntent {
  pub operation: String,
  pub path: String,
  pub external_to_cwd: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommandIntent {
  pub canonical_command: Vec<String>,
  pub command_prefix: Vec<String>,
  pub path_intents: Vec<PathIntent>,
  pub mutation_class: CommandMutationClass,
  pub network_hint: bool,
  pub external_paths: Vec<String>,
}

impl CommandIntent {
  pub fn from_command(command: &str, cwd: &Path) -> Self {
    let tokens = shlex::split(command).unwrap_or_default();
    Self::from_argv(&tokens, cwd)
  }

  pub fn from_argv(command: &[String], cwd: &Path) -> Self {
    let canonical_command = unwrap_shell_wrapper(command).unwrap_or_else(|| command.to_vec());
    let command_prefix = build_command_prefix(&canonical_command);
    let network_hint = command_uses_network(&canonical_command);
    let mutation_class = classify_mutation(&canonical_command);
    let path_intents = collect_path_intents(&canonical_command, cwd);
    let external_paths = path_intents
      .iter()
      .filter(|intent| intent.external_to_cwd)
      .map(|intent| intent.path.clone())
      .collect::<Vec<_>>();

    Self {
      canonical_command,
      command_prefix,
      path_intents,
      mutation_class,
      network_hint,
      external_paths,
    }
  }
}

fn unwrap_shell_wrapper(command: &[String]) -> Option<Vec<String>> {
  let cmd0 = command.first().map(|value| basename(value))?;
  let wrapped = match cmd0 {
    "bash" | "sh" | "zsh" if command.len() >= 3 && matches!(command[1].as_str(), "-c" | "-lc") => {
      command.get(2)
    }
    "cmd" | "cmd.exe" if command.len() >= 3 && command[1].eq_ignore_ascii_case("/c") => {
      command.get(2)
    }
    value if value.contains("powershell")
      && command.len() >= 3
      && matches!(command[1].as_str(), "-Command" | "-command" | "-c") =>
    {
      command.get(2)
    }
    _ => None,
  }?;

  let parsed = shlex::split(wrapped).unwrap_or_default();
  (!parsed.is_empty()).then_some(parsed)
}

fn basename(command: &str) -> &str {
  command
    .rsplit(['/', '\\'])
    .next()
    .unwrap_or(command)
}

fn build_command_prefix(command: &[String]) -> Vec<String> {
  let Some(cmd0) = command.first() else {
    return Vec::new();
  };

  let cmd0 = basename(cmd0);
  let mut prefix = vec![cmd0.to_string()];
  if matches!(cmd0, "git" | "cargo" | "npm" | "pnpm" | "yarn" | "uv")
    && let Some(arg1) = command.get(1)
    && !arg1.starts_with('-')
  {
    prefix.push(arg1.clone());
  }
  prefix
}

fn command_uses_network(command: &[String]) -> bool {
  let Some(cmd0) = command.first().map(|value| basename(value)) else {
    return false;
  };

  matches!(
    cmd0,
    "curl"
      | "wget"
      | "ssh"
      | "scp"
      | "rsync"
      | "nc"
      | "ncat"
      | "ping"
      | "dig"
      | "nslookup"
      | "telnet"
      | "ftp"
  )
}

fn classify_mutation(command: &[String]) -> CommandMutationClass {
  let Some(cmd0) = command.first().map(|value| basename(value)) else {
    return CommandMutationClass::Unknown;
  };

  match cmd0 {
    "cat" | "cd" | "echo" | "find" | "grep" | "head" | "ls" | "nl" | "pwd" | "rg" | "sed"
    | "tail" | "wc" => CommandMutationClass::ReadOnly,
    "touch" | "mkdir" | "cp" | "mv" | "tee" => CommandMutationClass::WritesFiles,
    "chmod" | "chown" | "chgrp" => CommandMutationClass::ChangesPermissions,
    "rm" => CommandMutationClass::Destructive,
    _ => CommandMutationClass::Unknown,
  }
}

fn collect_path_intents(command: &[String], cwd: &Path) -> Vec<PathIntent> {
  let Some(cmd0) = command.first().map(|value| basename(value)) else {
    return Vec::new();
  };

  let mut start_idx = 1usize;
  if matches!(cmd0, "chmod" | "chown" | "chgrp") {
    start_idx = 2;
  }

  if !matches!(
    cmd0,
    "cd" | "rm" | "cp" | "mv" | "mkdir" | "touch" | "chmod" | "chown" | "chgrp" | "cat"
  ) {
    return Vec::new();
  }

  command
    .iter()
    .skip(start_idx)
    .filter(|arg| !arg.starts_with('-'))
    .map(|arg| {
      let normalized = normalize_path(arg, cwd);
      let external_to_cwd = !normalized.starts_with(cwd);
      PathIntent {
        operation: cmd0.to_string(),
        path: normalized.display().to_string(),
        external_to_cwd,
      }
    })
    .collect()
}

fn normalize_path(arg: &str, cwd: &Path) -> PathBuf {
  let path = PathBuf::from(arg);
  let joined = if path.is_absolute() { path } else { cwd.join(path) };
  lexically_normalize_path(joined)
}

fn lexically_normalize_path(path: PathBuf) -> PathBuf {
  use std::path::Component;

  let mut normalized = PathBuf::new();
  for component in path.components() {
    match component {
      Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
      Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR.to_string()),
      Component::CurDir => {}
      Component::ParentDir => {
        normalized.pop();
      }
      Component::Normal(segment) => normalized.push(segment),
    }
  }
  normalized
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn shell_wrapper_is_unwrapped_for_canonical_command() {
    let intent = CommandIntent::from_argv(
      &[
        "bash".to_string(),
        "-lc".to_string(),
        "git status".to_string(),
      ],
      Path::new("/repo"),
    );

    assert_eq!(intent.canonical_command, vec!["git", "status"]);
    assert_eq!(intent.command_prefix, vec!["git", "status"]);
    assert_eq!(intent.mutation_class, CommandMutationClass::Unknown);
  }

  #[test]
  fn file_mutation_paths_are_normalized() {
    let intent = CommandIntent::from_command("mkdir src/generated", Path::new("/repo"));

    assert_eq!(intent.mutation_class, CommandMutationClass::WritesFiles);
    assert_eq!(
      intent.path_intents,
      vec![PathIntent {
        operation: "mkdir".to_string(),
        path: "/repo/src/generated".to_string(),
        external_to_cwd: false,
      }]
    );
  }

  #[test]
  fn external_paths_are_tracked() {
    let intent = CommandIntent::from_command("cat ../README.md", Path::new("/repo/worktree"));

    assert_eq!(intent.mutation_class, CommandMutationClass::ReadOnly);
    assert_eq!(intent.external_paths, vec!["/repo/README.md"]);
  }

  #[test]
  fn network_commands_are_marked() {
    let intent = CommandIntent::from_command("curl https://example.com", Path::new("/repo"));
    assert!(intent.network_hint);
  }
}
