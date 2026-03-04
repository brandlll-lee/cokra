use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::popup_consts::MAX_POPUP_ROWS;
use super::scroll_state::ScrollState;
use super::selection_popup_common::GenericDisplayRow;
use super::selection_popup_common::measure_rows_height;
use super::selection_popup_common::render_rows;
use super::slash_commands;
use crate::render::Insets;
use crate::render::RectExt;
use crate::slash_command::SlashCommand;

// Hide alias commands in the default popup list so each unique action appears once.
const ALIAS_COMMANDS: &[SlashCommand] = &[SlashCommand::Quit, SlashCommand::Approvals];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandItem {
  pub command: SlashCommand,
  pub display_name: String,
  pub description: String,
}

#[derive(Debug)]
pub(crate) struct CommandPopup {
  command_filter: String,
  items: Vec<CommandItem>,
  scroll: ScrollState,
}

impl CommandPopup {
  pub(crate) fn new(collaboration_modes_enabled: bool) -> Self {
    let items = slash_commands::builtins_for_input(collaboration_modes_enabled, true, true, false)
      .into_iter()
      .filter(|(_, cmd)| !cmd.command().starts_with("debug"))
      .map(|(_, command)| CommandItem {
        command,
        display_name: format!("/{}", command.command()),
        description: command.description().to_string(),
      })
      .collect();

    Self {
      command_filter: String::new(),
      items,
      scroll: ScrollState::new(),
    }
  }

  /// Update the filter string based on the current composer text.
  /// 1:1 codex: `CommandPopup::on_composer_text_change`.
  pub(crate) fn on_composer_text_change(&mut self, text: String) {
    let first_line = text.lines().next().unwrap_or("");

    if let Some(stripped) = first_line.strip_prefix('/') {
      let token = stripped.trim_start();
      let cmd_token = token.split_whitespace().next().unwrap_or("");
      self.command_filter = cmd_token.to_string();
    } else {
      self.command_filter.clear();
    }

    let len = self.filtered_commands().len();
    self.scroll.clamp_selection(len);
    self.scroll.ensure_visible(len, MAX_POPUP_ROWS.min(len));
  }

  /// Move the selection cursor one step up.
  pub(crate) fn move_up(&mut self) {
    let len = self.filtered_commands().len();
    self.scroll.move_up_wrap(len);
    self.scroll.ensure_visible(len, MAX_POPUP_ROWS.min(len));
  }

  /// Move the selection cursor one step down.
  pub(crate) fn move_down(&mut self) {
    let len = self.filtered_commands().len();
    self.scroll.move_down_wrap(len);
    self.scroll.ensure_visible(len, MAX_POPUP_ROWS.min(len));
  }

  /// Determine the preferred height of the popup for a given width.
  /// 1:1 codex: CommandPopup::calculate_required_height.
  pub(crate) fn calculate_required_height(&self, width: u16) -> u16 {
    let rows = self.render_rows_data();
    measure_rows_height(&rows, &self.scroll, MAX_POPUP_ROWS, width)
  }

  /// Return currently selected command, if any.
  pub(crate) fn selected_command(&self) -> Option<SlashCommand> {
    let filtered = self.filtered_commands();
    if filtered.is_empty() {
      return None;
    }
    let idx = self
      .scroll
      .selected_idx
      .unwrap_or(0)
      .min(filtered.len() - 1);
    filtered.get(idx).copied()
  }

  fn filtered(&self) -> Vec<(usize, Option<Vec<usize>>)> {
    let filter = self.command_filter.trim();
    if filter.is_empty() {
      return self
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| !ALIAS_COMMANDS.contains(&item.command))
        .map(|(idx, _)| (idx, None))
        .collect();
    }

    let filter_lower = filter.to_lowercase();
    let filter_chars = filter.chars().count();

    let mut exact = Vec::new();
    let mut prefix = Vec::new();

    for (idx, item) in self.items.iter().enumerate() {
      let name = item.command.command();
      let lower = name.to_lowercase();
      if lower == filter_lower {
        exact.push((idx, Some((0..filter_chars).collect())));
      } else if lower.starts_with(&filter_lower) {
        prefix.push((idx, Some((0..filter_chars).collect())));
      }
    }

    exact.extend(prefix);
    exact
  }

  fn filtered_commands(&self) -> Vec<SlashCommand> {
    self
      .filtered()
      .into_iter()
      .filter_map(|(idx, _)| self.items.get(idx))
      .map(|item| item.command)
      .collect()
  }

  fn render_rows_data(&self) -> Vec<GenericDisplayRow> {
    self
      .filtered()
      .into_iter()
      .filter_map(|(idx, match_indices)| self.items.get(idx).map(|item| (item, match_indices)))
      .map(|(item, match_indices)| GenericDisplayRow {
        name: item.display_name.clone(),
        display_shortcut: None,
        match_indices: match_indices.map(|v| v.into_iter().map(|i| i + 1).collect()),
        description: Some(item.description.clone()),
        category_tag: None,
        disabled_reason: None,
        is_disabled: false,
        wrap_indent: None,
      })
      .collect()
  }

  pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
    let rows = self.render_rows_data();
    render_rows(
      area.inset(Insets::tlbr(0, 2, 0, 0)),
      buf,
      &rows,
      &self.scroll,
      MAX_POPUP_ROWS,
      "no matches",
    );
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn exact_match_ranks_first() {
    let mut popup = CommandPopup::new(true);
    popup.on_composer_text_change("/model".to_string());
    assert_eq!(popup.selected_command(), Some(SlashCommand::Model));
  }

  #[test]
  fn alias_commands_hidden_when_filter_empty() {
    let popup = CommandPopup::new(true);
    let filtered = popup.filtered_commands();
    assert!(!filtered.contains(&SlashCommand::Quit));
    assert!(!filtered.contains(&SlashCommand::Approvals));
  }
}
