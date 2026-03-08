use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::app_event_sender::AppEventSender;
use crate::render::renderable::FlexRenderable;
use crate::render::renderable::Renderable;
use crate::render::renderable::RenderableItem;
use crate::status_indicator_widget::StatusIndicatorWidget;
use crate::tui::FrameRequester;
use approval_overlay::ApprovalChoice;
use approval_overlay::ApprovalOverlay;
use bottom_pane_view::BottomPaneView;
use chat_composer::ChatComposer;
use chat_composer::ComposerAction;
use chat_composer::ComposerSubmission;
use cokra_protocol::RequestUserInputEvent;
use queued_user_messages::QueuedUserMessages;
use request_user_input::RequestUserInputView;

pub(crate) mod api_key_entry_view;
pub(crate) mod approval_overlay;
pub(crate) mod bottom_pane_view;
pub(crate) mod chat_composer;
pub(crate) mod chat_composer_history;
pub(crate) mod command_popup;
pub(crate) mod footer;
pub(crate) mod list_selection_view;
pub(crate) mod oauth_connect_view;
pub(crate) mod paste_burst;
pub(crate) mod popup_consts;
pub(crate) mod prompt_args;
pub(crate) mod queued_user_messages;
pub(crate) mod request_user_input;
pub(crate) mod scroll_state;
pub(crate) mod selection_popup_common;
pub(crate) mod slash_commands;
pub(crate) mod textarea;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MentionBinding {
  pub(crate) mention: String,
  pub(crate) path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalImageAttachment {
  pub(crate) placeholder: String,
  pub(crate) path: PathBuf,
}

#[derive(Debug)]
pub(crate) enum BottomPaneAction {
  None,
  Submit(ComposerSubmission),
  Queue(ComposerSubmission),
  Interrupt,
  RequestQuit,
  ApprovalDecision(ApprovalChoice),
  /// A slash command was selected from the command popup.
  SlashCommand(crate::slash_command::SlashCommand),
}

/// 1:1 codex BottomPane: owns the ChatComposer and a view_stack.
///
/// When the view_stack is non-empty, the topmost view **replaces** the
/// composer in both rendering and key handling. The composer state is
/// retained underneath so it survives view dismissal.
pub(crate) struct BottomPane {
  /// Composer is retained even when a view is displayed.
  composer: ChatComposer,

  /// 1:1 codex view_stack: views displayed instead of the composer.
  view_stack: Vec<Box<dyn BottomPaneView>>,

  /// Approval overlay renders on top of whatever is active.
  approval_overlay: Option<ApprovalOverlay>,

  /// Inline status indicator shown above the composer while a task is running.
  status: Option<StatusIndicatorWidget>,
  queued_user_messages: QueuedUserMessages,
  app_event_tx: AppEventSender,
  frame_requester: FrameRequester,
  animations_enabled: bool,
}

impl BottomPane {
  pub(crate) fn new(
    app_event_tx: AppEventSender,
    frame_requester: FrameRequester,
    animations_enabled: bool,
  ) -> Self {
    Self {
      composer: ChatComposer::new(),
      view_stack: Vec::new(),
      approval_overlay: None,
      status: None,
      queued_user_messages: QueuedUserMessages::new(),
      app_event_tx,
      frame_requester,
      animations_enabled,
    }
  }

  // 1:1 codex: create/destroy StatusIndicator on task start/stop.
  pub(crate) fn set_task_running(&mut self, running: bool) {
    self.composer.set_task_running(running);
    if running {
      if self.status.is_none() {
        self.status = Some(StatusIndicatorWidget::new(
          self.app_event_tx.clone(),
          self.frame_requester.clone(),
          self.animations_enabled,
        ));
      }
      if let Some(status) = self.status.as_mut() {
        status.resume_timer();
        status.update_header("Working".to_string());
      }
    } else {
      if let Some(status) = self.status.as_mut() {
        status.pause_timer();
        status.update_details(None);
        status.update_inline_message(None);
      }
      self.status = None;
    }
  }

  pub(crate) fn status_widget(&self) -> Option<&StatusIndicatorWidget> {
    self.status.as_ref()
  }

  pub(crate) fn status_widget_mut(&mut self) -> Option<&mut StatusIndicatorWidget> {
    self.status.as_mut()
  }

  pub(crate) fn ensure_status_indicator(&mut self) {
    if self.status.is_none() {
      self.status = Some(StatusIndicatorWidget::new(
        self.app_event_tx.clone(),
        self.frame_requester.clone(),
        self.animations_enabled,
      ));
    }
  }

  pub(crate) fn set_context_window(&mut self, percent: Option<i64>, used_tokens: Option<i64>) {
    self.composer.set_context_window(percent, used_tokens);
  }

  pub(crate) fn set_status_line(&mut self, status_line: Option<Line<'static>>) {
    self.composer.set_status_line(status_line);
  }

  pub(crate) fn set_status_line_enabled(&mut self, enabled: bool) {
    self.composer.set_status_line_enabled(enabled);
  }

  pub(crate) fn set_queued_user_messages(&mut self, queued: Vec<String>) {
    self.queued_user_messages.messages = queued;
  }

  pub(crate) fn next_footer_transition_in(&self) -> Option<Duration> {
    self.composer.next_footer_transition_in()
  }

  pub(crate) fn flush_burst_if_due(&mut self) {
    self.composer.flush_burst_if_due(Instant::now());
  }

  /// Returns true if any paste-burst transient state is active (buffering or holding first char).
  pub(crate) fn is_in_paste_burst(&self) -> bool {
    self.composer.is_in_paste_burst()
  }

  /// Flush any due paste burst and return true if something was flushed (caller should redraw).
  pub(crate) fn flush_paste_burst_if_due(&mut self) -> bool {
    self.composer.flush_burst_if_due(Instant::now())
  }

  pub(crate) fn open_approval(&mut self, overlay: ApprovalOverlay) {
    self.approval_overlay = Some(overlay);
  }

  pub(crate) fn close_approval(&mut self) {
    self.approval_overlay = None;
  }

  /// 1:1 codex: push a view onto the view_stack.
  pub(crate) fn push_view(&mut self, view: Box<dyn BottomPaneView>) {
    self.view_stack.push(view);
  }

  /// 1:1 codex show_selection_view: convenience to push a ListSelectionView.
  pub(crate) fn show_selection_view(&mut self, params: list_selection_view::SelectionViewParams) {
    let view = list_selection_view::ListSelectionView::new(params, self.app_event_tx.clone());
    self.push_view(Box::new(view));
  }

  pub(crate) fn push_user_input_request(&mut self, request: RequestUserInputEvent) {
    self.push_view(Box::new(RequestUserInputView::new(
      request,
      self.app_event_tx.clone(),
    )));
  }

  pub(crate) fn dismiss_active_view(&mut self) {
    self.view_stack.pop();
  }

  /// 1:1 codex: active view is the top of the stack.
  fn active_view(&self) -> Option<&dyn BottomPaneView> {
    self.view_stack.last().map(|v| v.as_ref())
  }

  pub(crate) fn desired_height(&self, width: u16) -> u16 {
    self.as_renderable().desired_height(width)
  }

  pub(crate) fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
    if self.approval_overlay.is_some() {
      return None;
    }
    self.as_renderable().cursor_pos(area)
  }

  /// 1:1 codex: key event routing.
  /// Layer 1: approval overlay (highest priority)
  /// Layer 2: view_stack top (replaces composer)
  /// Layer 3: composer (default)
  pub(crate) fn handle_key(&mut self, key: KeyEvent) -> BottomPaneAction {
    // Layer 1: approval overlay intercepts everything.
    if let Some(overlay) = self.approval_overlay.as_mut() {
      match key.code {
        KeyCode::Left => overlay.move_left(),
        KeyCode::Right | KeyCode::Tab => overlay.move_right(),
        KeyCode::Enter => {
          let selected = overlay.selected();
          self.approval_overlay = None;
          return BottomPaneAction::ApprovalDecision(selected);
        }
        KeyCode::Esc => {
          self.approval_overlay = None;
          return BottomPaneAction::ApprovalDecision(ApprovalChoice::Deny);
        }
        _ => {}
      }
      return BottomPaneAction::None;
    }

    // Layer 2: 1:1 codex view_stack routing.
    if !self.view_stack.is_empty() {
      let last_idx = self.view_stack.len() - 1;
      let view = &mut self.view_stack[last_idx];

      if key.code == KeyCode::Esc && view.on_cancel() {
        self.view_stack.pop();
        return BottomPaneAction::None;
      }

      view.handle_key_event(key);

      if view.is_complete() {
        self.view_stack.pop();
        return BottomPaneAction::None;
      }

      return BottomPaneAction::None;
    }

    // Layer 3: composer (default).
    match self.composer.handle_key_event(key) {
      ComposerAction::None => BottomPaneAction::None,
      ComposerAction::Queue => self
        .composer
        .prepare_submission()
        .map(BottomPaneAction::Queue)
        .unwrap_or(BottomPaneAction::None),
      ComposerAction::Interrupt => BottomPaneAction::Interrupt,
      ComposerAction::RequestQuit => BottomPaneAction::RequestQuit,
      ComposerAction::Submit => self
        .composer
        .prepare_submission()
        .map(BottomPaneAction::Submit)
        .unwrap_or(BottomPaneAction::None),
      ComposerAction::SlashCommand(cmd) => BottomPaneAction::SlashCommand(cmd),
    }
  }

  pub(crate) fn handle_paste(&mut self, text: String) {
    if let Some(view) = self.view_stack.last_mut()
      && view.handle_paste(text.clone())
    {
      return;
    }
    self.composer.handle_paste(text);
  }

  /// 1:1 codex as_renderable: when view_stack is non-empty, the topmost view
  /// **replaces** the composer entirely. When empty, render status + composer.
  fn as_renderable(&self) -> RenderableItem<'_> {
    if let Some(view) = self.active_view() {
      // 1:1 codex: view replaces composer.
      RenderableItem::Borrowed(view as &dyn Renderable)
    } else {
      let mut header = FlexRenderable::new();
      if let Some(status) = &self.status {
        header.push(0, RenderableItem::Borrowed(status as &dyn Renderable));
      } else {
        // Keep inline viewport height stable when the task-status row appears.
        header.push(0, RenderableItem::Owned("".into()));
      }
      let has_queued_messages = !self.queued_user_messages.messages.is_empty();
      let has_status = true;
      if has_queued_messages && has_status {
        header.push(0, RenderableItem::Owned("".into()));
      }
      header.push(
        1,
        RenderableItem::Borrowed(&self.queued_user_messages as &dyn Renderable),
      );
      if !has_queued_messages && has_status {
        header.push(0, RenderableItem::Owned("".into()));
      }

      let mut flex = FlexRenderable::new();
      flex.push(1, RenderableItem::Owned(Box::new(header)));
      flex.push(
        0,
        RenderableItem::Borrowed(&self.composer as &dyn Renderable),
      );
      RenderableItem::Owned(Box::new(flex))
    }
  }

  pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, _task_running: bool) {
    if area.is_empty() {
      return;
    }
    self.as_renderable().render(area, buf);
    // Approval overlay renders on top of whatever is active.
    if let Some(overlay) = &self.approval_overlay {
      overlay.render(area, buf);
    }
  }
}

impl Renderable for BottomPane {
  fn render(&self, area: Rect, buf: &mut Buffer) {
    self.render(area, buf, false);
  }

  fn desired_height(&self, width: u16) -> u16 {
    self.desired_height(width)
  }

  fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
    self.cursor_pos(area)
  }
}
