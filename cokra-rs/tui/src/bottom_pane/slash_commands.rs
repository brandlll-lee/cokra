//! Shared helpers for filtering and matching built-in slash commands.
//!
//! The same sandbox- and feature-gating rules are used by both the composer
//! and the command popup. Centralizing them here keeps those call sites small
//! and ensures they stay in sync.
use crate::slash_command::SlashCommand;
use crate::slash_command::built_in_slash_commands;

fn fuzzy_match_simple(haystack: &str, needle: &str) -> bool {
  if needle.is_empty() {
    return true;
  }

  let mut need_iter = needle.chars().map(|ch| ch.to_ascii_lowercase());
  let mut current = match need_iter.next() {
    Some(ch) => ch,
    None => return true,
  };

  for h in haystack.chars().map(|ch| ch.to_ascii_lowercase()) {
    if h == current {
      match need_iter.next() {
        Some(next) => current = next,
        None => return true,
      }
    }
  }

  false
}

/// Return the built-ins that should be visible/usable for the current input.
pub(crate) fn builtins_for_input(
  collaboration_modes_enabled: bool,
  connectors_enabled: bool,
  personality_command_enabled: bool,
  allow_elevate_sandbox: bool,
) -> Vec<(&'static str, SlashCommand)> {
  built_in_slash_commands()
    .into_iter()
    .filter(|(_, cmd)| allow_elevate_sandbox || *cmd != SlashCommand::ElevateSandbox)
    .filter(|(_, cmd)| {
      collaboration_modes_enabled || !matches!(*cmd, SlashCommand::Collab | SlashCommand::Plan)
    })
    .filter(|(_, cmd)| connectors_enabled || *cmd != SlashCommand::Apps)
    .filter(|(_, cmd)| personality_command_enabled || *cmd != SlashCommand::Personality)
    .collect()
}

/// Find a single built-in command by exact name, after applying the gating rules.
pub(crate) fn find_builtin_command(
  name: &str,
  collaboration_modes_enabled: bool,
  connectors_enabled: bool,
  personality_command_enabled: bool,
  allow_elevate_sandbox: bool,
) -> Option<SlashCommand> {
  builtins_for_input(
    collaboration_modes_enabled,
    connectors_enabled,
    personality_command_enabled,
    allow_elevate_sandbox,
  )
  .into_iter()
  .find(|(command_name, _)| *command_name == name)
  .map(|(_, cmd)| cmd)
}

/// Whether any visible built-in fuzzily matches the provided prefix.
pub(crate) fn has_builtin_prefix(
  name: &str,
  collaboration_modes_enabled: bool,
  connectors_enabled: bool,
  personality_command_enabled: bool,
  allow_elevate_sandbox: bool,
) -> bool {
  builtins_for_input(
    collaboration_modes_enabled,
    connectors_enabled,
    personality_command_enabled,
    allow_elevate_sandbox,
  )
  .into_iter()
  .any(|(command_name, _)| fuzzy_match_simple(command_name, name))
}

#[cfg(test)]
mod tests {
  use super::*;
  use pretty_assertions::assert_eq;

  #[test]
  fn debug_command_still_resolves_for_dispatch() {
    let cmd = find_builtin_command("debug-config", true, true, true, false);
    assert_eq!(cmd, Some(SlashCommand::DebugConfig));
  }

  #[test]
  fn fuzzy_match_accepts_subsequence() {
    assert!(fuzzy_match_simple("debug-config", "dbgcfg"));
    assert!(!fuzzy_match_simple("model", "zz"));
  }
}
