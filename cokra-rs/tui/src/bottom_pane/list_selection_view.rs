use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use itertools::Itertools as _;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use super::bottom_pane_view::BottomPaneView;
use super::popup_consts::MAX_POPUP_ROWS;
use super::scroll_state::ScrollState;
pub(crate) use super::selection_popup_common::ColumnWidthMode;
use super::selection_popup_common::GenericDisplayRow;
use super::selection_popup_common::measure_rows_height;
use super::selection_popup_common::measure_rows_height_stable_col_widths;
use super::selection_popup_common::measure_rows_height_with_col_width_mode;
use super::selection_popup_common::render_menu_surface;
use super::selection_popup_common::render_rows;
use super::selection_popup_common::render_rows_stable_col_widths;
use super::selection_popup_common::render_rows_with_col_width_mode;
use super::selection_popup_common::wrap_styled_line;
use crate::app_event_sender::AppEventSender;
use crate::key_hint::KeyBinding;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::Renderable;

pub(crate) type SelectionAction = Box<dyn Fn(&AppEventSender) + Send + Sync>;

#[derive(Default)]
pub(crate) struct SelectionItem {
  pub name: String,
  pub display_shortcut: Option<KeyBinding>,
  pub description: Option<String>,
  pub selected_description: Option<String>,
  pub is_current: bool,
  pub is_default: bool,
  pub is_disabled: bool,
  pub actions: Vec<SelectionAction>,
  pub dismiss_on_select: bool,
  pub search_value: Option<String>,
  pub disabled_reason: Option<String>,
}

/// Construction-time configuration for [`ListSelectionView`].
pub(crate) struct SelectionViewParams {
  pub view_id: Option<&'static str>,
  pub title: Option<String>,
  pub subtitle: Option<String>,
  pub footer_note: Option<Line<'static>>,
  pub footer_hint: Option<Line<'static>>,
  pub items: Vec<SelectionItem>,
  pub is_searchable: bool,
  pub search_placeholder: Option<String>,
  pub col_width_mode: ColumnWidthMode,
  pub header: Box<dyn Renderable>,
  pub initial_selected_idx: Option<usize>,
}

impl Default for SelectionViewParams {
  fn default() -> Self {
    Self {
      view_id: None,
      title: None,
      subtitle: None,
      footer_note: None,
      footer_hint: None,
      items: Vec::new(),
      is_searchable: false,
      search_placeholder: None,
      col_width_mode: ColumnWidthMode::AutoVisible,
      header: Box::new(()),
      initial_selected_idx: None,
    }
  }
}

/// 1:1 codex ListSelectionView: runtime state for a list-based selection popup.
pub(crate) struct ListSelectionView {
  view_id: Option<&'static str>,
  footer_note: Option<Line<'static>>,
  footer_hint: Option<Line<'static>>,
  items: Vec<SelectionItem>,
  state: ScrollState,
  complete: bool,
  app_event_tx: AppEventSender,
  is_searchable: bool,
  search_query: String,
  search_placeholder: Option<String>,
  col_width_mode: ColumnWidthMode,
  filtered_indices: Vec<usize>,
  last_selected_actual_idx: Option<usize>,
  header: Box<dyn Renderable>,
  initial_selected_idx: Option<usize>,
}

impl ListSelectionView {
  pub fn new(params: SelectionViewParams, app_event_tx: AppEventSender) -> Self {
    let mut header = params.header;
    if params.title.is_some() || params.subtitle.is_some() {
      let mut children: Vec<crate::render::renderable::RenderableItem<'static>> = Vec::new();
      children.push(header.into());
      if let Some(title) = params.title {
        children.push(Box::<dyn Renderable>::from(Line::from(title.bold())).into());
      }
      if let Some(subtitle) = params.subtitle {
        children.push(Box::<dyn Renderable>::from(Line::from(subtitle.dim())).into());
      }
      header = Box::new(ColumnRenderable::with(children));
    }

    let mut s = Self {
      view_id: params.view_id,
      footer_note: params.footer_note,
      footer_hint: params.footer_hint,
      items: params.items,
      state: ScrollState::new(),
      complete: false,
      app_event_tx,
      is_searchable: params.is_searchable,
      search_query: String::new(),
      search_placeholder: if params.is_searchable {
        params.search_placeholder
      } else {
        None
      },
      col_width_mode: params.col_width_mode,
      filtered_indices: Vec::new(),
      last_selected_actual_idx: None,
      header,
      initial_selected_idx: params.initial_selected_idx,
    };
    s.apply_filter();
    s
  }

  fn visible_len(&self) -> usize {
    self.filtered_indices.len()
  }

  fn max_visible_rows(len: usize) -> usize {
    MAX_POPUP_ROWS.min(len.max(1))
  }

  fn apply_filter(&mut self) {
    let previously_selected = self
      .state
      .selected_idx
      .and_then(|visible_idx| self.filtered_indices.get(visible_idx).copied())
      .or_else(|| {
        (!self.is_searchable)
          .then(|| self.items.iter().position(|item| item.is_current))
          .flatten()
      })
      .or_else(|| self.initial_selected_idx.take());

    if self.is_searchable && !self.search_query.is_empty() {
      let query_lower = self.search_query.to_lowercase();
      self.filtered_indices = self
        .items
        .iter()
        .positions(|item| {
          item
            .search_value
            .as_ref()
            .is_some_and(|v| v.to_lowercase().contains(&query_lower))
        })
        .collect();
    } else {
      self.filtered_indices = (0..self.items.len()).collect();
    }

    let len = self.filtered_indices.len();
    self.state.selected_idx = self
      .state
      .selected_idx
      .and_then(|visible_idx| {
        self
          .filtered_indices
          .get(visible_idx)
          .and_then(|idx| self.filtered_indices.iter().position(|cur| cur == idx))
      })
      .or_else(|| {
        previously_selected.and_then(|actual_idx| {
          self
            .filtered_indices
            .iter()
            .position(|idx| *idx == actual_idx)
        })
      })
      .or_else(|| (len > 0).then_some(0));

    let visible = Self::max_visible_rows(len);
    self.state.clamp_selection(len);
    self.state.ensure_visible(len, visible);
  }

  fn move_up(&mut self) {
    let len = self.visible_len();
    self.state.move_up_wrap(len);
    let visible = Self::max_visible_rows(len);
    self.state.ensure_visible(len, visible);
    self.skip_disabled_up();
  }

  fn move_down(&mut self) {
    let len = self.visible_len();
    self.state.move_down_wrap(len);
    let visible = Self::max_visible_rows(len);
    self.state.ensure_visible(len, visible);
    self.skip_disabled_down();
  }

  fn accept(&mut self) {
    let selected_item = self
      .state
      .selected_idx
      .and_then(|idx| self.filtered_indices.get(idx))
      .and_then(|actual_idx| self.items.get(*actual_idx));
    if let Some(item) = selected_item
      && item.disabled_reason.is_none()
      && !item.is_disabled
    {
      if let Some(idx) = self.state.selected_idx
        && let Some(actual_idx) = self.filtered_indices.get(idx)
      {
        self.last_selected_actual_idx = Some(*actual_idx);
      }
      for act in &item.actions {
        act(&self.app_event_tx);
      }
      if item.dismiss_on_select {
        self.complete = true;
      }
    } else if selected_item.is_none() {
      self.complete = true;
    }
  }

  pub(crate) fn take_last_selected_index(&mut self) -> Option<usize> {
    self.last_selected_actual_idx.take()
  }

  fn rows_width(total_width: u16) -> u16 {
    total_width.saturating_sub(2)
  }

  fn skip_disabled_down(&mut self) {
    let len = self.visible_len();
    for _ in 0..len {
      if let Some(idx) = self.state.selected_idx
        && let Some(actual_idx) = self.filtered_indices.get(idx)
        && self
          .items
          .get(*actual_idx)
          .is_some_and(|item| item.disabled_reason.is_some() || item.is_disabled)
      {
        self.state.move_down_wrap(len);
      } else {
        break;
      }
    }
  }

  fn skip_disabled_up(&mut self) {
    let len = self.visible_len();
    for _ in 0..len {
      if let Some(idx) = self.state.selected_idx
        && let Some(actual_idx) = self.filtered_indices.get(idx)
        && self
          .items
          .get(*actual_idx)
          .is_some_and(|item| item.disabled_reason.is_some() || item.is_disabled)
      {
        self.state.move_up_wrap(len);
      } else {
        break;
      }
    }
  }

  fn build_rows(&self) -> Vec<GenericDisplayRow> {
    self
      .filtered_indices
      .iter()
      .enumerate()
      .filter_map(|(visible_idx, actual_idx)| {
        self.items.get(*actual_idx).map(|item| {
          let is_selected = self.state.selected_idx == Some(visible_idx);
          let prefix = if is_selected { '›' } else { ' ' };
          let name = item.name.as_str();
          let marker = if item.is_current {
            " (current)"
          } else if item.is_default {
            " (default)"
          } else {
            ""
          };
          let name_with_marker = format!("{name}{marker}");
          let n = visible_idx + 1;
          let wrap_prefix = if self.is_searchable {
            format!("{prefix} ")
          } else {
            format!("{prefix} {n}. ")
          };
          let wrap_prefix_width = UnicodeWidthStr::width(wrap_prefix.as_str());
          let display_name = format!("{wrap_prefix}{name_with_marker}");
          let description = is_selected
            .then(|| item.selected_description.clone())
            .flatten()
            .or_else(|| item.description.clone());
          let wrap_indent = description.is_none().then_some(wrap_prefix_width);
          let is_disabled = item.is_disabled || item.disabled_reason.is_some();
          GenericDisplayRow {
            name: display_name,
            display_shortcut: item.display_shortcut,
            match_indices: None,
            description,
            category_tag: None,
            wrap_indent,
            is_disabled,
            disabled_reason: item.disabled_reason.clone(),
          }
        })
      })
      .collect()
  }
}

// 1:1 codex: implement BottomPaneView so it can live on the view_stack.
impl BottomPaneView for ListSelectionView {
  fn handle_key_event(&mut self, key_event: KeyEvent) {
    match key_event {
      KeyEvent { code: KeyCode::Up, .. }
      | KeyEvent {
        code: KeyCode::Char('p'),
        modifiers: KeyModifiers::CONTROL,
        ..
      }
      | KeyEvent {
        code: KeyCode::Char('\u{0010}'),
        modifiers: KeyModifiers::NONE,
        ..
      } /* ^P */ => self.move_up(),

      KeyEvent {
        code: KeyCode::Char('k'),
        modifiers: KeyModifiers::NONE,
        ..
      } if !self.is_searchable => self.move_up(),

      KeyEvent { code: KeyCode::Down, .. }
      | KeyEvent {
        code: KeyCode::Char('n'),
        modifiers: KeyModifiers::CONTROL,
        ..
      }
      | KeyEvent {
        code: KeyCode::Char('\u{000e}'),
        modifiers: KeyModifiers::NONE,
        ..
      } /* ^N */ => self.move_down(),

      KeyEvent {
        code: KeyCode::Char('j'),
        modifiers: KeyModifiers::NONE,
        ..
      } if !self.is_searchable => self.move_down(),

      KeyEvent {
        code: KeyCode::Backspace,
        ..
      } if self.is_searchable => {
        self.search_query.pop();
        self.apply_filter();
      }

      KeyEvent {
        code: KeyCode::Esc, ..
      } => {
        self.on_cancel();
      }

      KeyEvent {
        code: KeyCode::Char(c),
        modifiers,
        ..
      } if self.is_searchable
        && !modifiers.contains(KeyModifiers::CONTROL)
        && !modifiers.contains(KeyModifiers::ALT) =>
      {
        self.search_query.push(c);
        self.apply_filter();
      }

      KeyEvent {
        code: KeyCode::Char(c),
        modifiers,
        ..
      } if !self.is_searchable
        && !modifiers.contains(KeyModifiers::CONTROL)
        && !modifiers.contains(KeyModifiers::ALT) =>
      {
        if let Some(idx) = c
          .to_digit(10)
          .map(|d| d as usize)
          .and_then(|d| d.checked_sub(1))
          && idx < self.items.len()
          && self
            .items
            .get(idx)
            .is_some_and(|item| item.disabled_reason.is_none() && !item.is_disabled)
        {
          self.state.selected_idx = Some(idx);
          self.accept();
        }
      }

      KeyEvent {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::NONE,
        ..
      } => self.accept(),

      _ => {}
    }
  }

  fn is_complete(&self) -> bool {
    self.complete
  }

  fn on_cancel(&mut self) -> bool {
    self.complete = true;
    true
  }
}

impl Renderable for ListSelectionView {
  fn desired_height(&self, width: u16) -> u16 {
    let rows = self.build_rows();
    let rows_width = Self::rows_width(width);
    let rows_height = match self.col_width_mode {
      ColumnWidthMode::AutoVisible => measure_rows_height(
        &rows,
        &self.state,
        MAX_POPUP_ROWS,
        rows_width.saturating_add(1),
      ),
      ColumnWidthMode::AutoAllRows => measure_rows_height_stable_col_widths(
        &rows,
        &self.state,
        MAX_POPUP_ROWS,
        rows_width.saturating_add(1),
      ),
      ColumnWidthMode::Fixed => measure_rows_height_with_col_width_mode(
        &rows,
        &self.state,
        MAX_POPUP_ROWS,
        rows_width.saturating_add(1),
        ColumnWidthMode::Fixed,
      ),
    };

    let mut height = self.header.desired_height(width.saturating_sub(4));
    height = height.saturating_add(rows_height + 3);
    if self.is_searchable {
      height = height.saturating_add(1);
    }
    if let Some(note) = &self.footer_note {
      let note_width = width.saturating_sub(2);
      let note_lines = wrap_styled_line(note, note_width);
      height = height.saturating_add(note_lines.len() as u16);
    }
    if self.footer_hint.is_some() {
      height = height.saturating_add(1);
    }
    height
  }

  fn render(&self, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
      return;
    }

    let note_width = area.width.saturating_sub(2);
    let note_lines = self
      .footer_note
      .as_ref()
      .map(|note| wrap_styled_line(note, note_width));
    let note_height = note_lines.as_ref().map_or(0, |lines| lines.len() as u16);
    let footer_rows = note_height + u16::from(self.footer_hint.is_some());
    let [content_area, footer_area] =
      Layout::vertical([Constraint::Fill(1), Constraint::Length(footer_rows)]).areas(area);

    let outer_content_area = content_area;
    let content_area = render_menu_surface(outer_content_area, buf);

    let header_height = self
      .header
      .desired_height(outer_content_area.width.saturating_sub(4));
    let rows = self.build_rows();
    let rows_width = Self::rows_width(outer_content_area.width);
    let rows_height = match self.col_width_mode {
      ColumnWidthMode::AutoVisible => measure_rows_height(
        &rows,
        &self.state,
        MAX_POPUP_ROWS,
        rows_width.saturating_add(1),
      ),
      ColumnWidthMode::AutoAllRows => measure_rows_height_stable_col_widths(
        &rows,
        &self.state,
        MAX_POPUP_ROWS,
        rows_width.saturating_add(1),
      ),
      ColumnWidthMode::Fixed => measure_rows_height_with_col_width_mode(
        &rows,
        &self.state,
        MAX_POPUP_ROWS,
        rows_width.saturating_add(1),
        ColumnWidthMode::Fixed,
      ),
    };

    let [header_area, _, search_area, list_area] = Layout::vertical([
      Constraint::Max(header_height),
      Constraint::Max(1),
      Constraint::Length(if self.is_searchable { 1 } else { 0 }),
      Constraint::Length(rows_height),
    ])
    .areas(content_area);

    if header_area.height < header_height {
      let [header_area, elision_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(header_area);
      self.header.render(header_area, buf);
      Paragraph::new(vec![
        Line::from(format!("[… {header_height} lines] ctrl + a view all")).dim(),
      ])
      .render(elision_area, buf);
    } else {
      self.header.render(header_area, buf);
    }

    if self.is_searchable {
      let query_span: Span<'static> = if self.search_query.is_empty() {
        self
          .search_placeholder
          .as_ref()
          .map(|placeholder| placeholder.clone().dim())
          .unwrap_or_else(|| "".into())
      } else {
        self.search_query.clone().into()
      };
      Line::from(query_span).render(search_area, buf);
    }

    if list_area.height > 0 {
      let render_area = Rect {
        x: list_area.x.saturating_sub(2),
        y: list_area.y,
        width: rows_width.max(1),
        height: list_area.height,
      };
      match self.col_width_mode {
        ColumnWidthMode::AutoVisible => render_rows(
          render_area,
          buf,
          &rows,
          &self.state,
          render_area.height as usize,
          "no matches",
        ),
        ColumnWidthMode::AutoAllRows => render_rows_stable_col_widths(
          render_area,
          buf,
          &rows,
          &self.state,
          render_area.height as usize,
          "no matches",
        ),
        ColumnWidthMode::Fixed => render_rows_with_col_width_mode(
          render_area,
          buf,
          &rows,
          &self.state,
          render_area.height as usize,
          "no matches",
          ColumnWidthMode::Fixed,
        ),
      };
    }

    if footer_area.height > 0 {
      let [note_area, hint_area] = Layout::vertical([
        Constraint::Length(note_height),
        Constraint::Length(if self.footer_hint.is_some() { 1 } else { 0 }),
      ])
      .areas(footer_area);

      if let Some(lines) = note_lines {
        let note_area = Rect {
          x: note_area.x + 2,
          y: note_area.y,
          width: note_area.width.saturating_sub(2),
          height: note_area.height,
        };
        for (idx, line) in lines.iter().enumerate() {
          if idx as u16 >= note_area.height {
            break;
          }
          let line_area = Rect {
            x: note_area.x,
            y: note_area.y + idx as u16,
            width: note_area.width,
            height: 1,
          };
          line.clone().render(line_area, buf);
        }
      }

      if let Some(hint) = &self.footer_hint {
        let hint_area = Rect {
          x: hint_area.x + 2,
          y: hint_area.y,
          width: hint_area.width.saturating_sub(2),
          height: hint_area.height,
        };
        hint.clone().dim().render(hint_area, buf);
      }
    }
  }

  fn cursor_pos(&self, _area: Rect) -> Option<(u16, u16)> {
    None
  }
}
