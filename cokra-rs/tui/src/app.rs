use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::text::Line;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use cokra_core::Cokra;
use cokra_protocol::Event;
use cokra_protocol::EventMsg;
use cokra_protocol::Op;
use cokra_protocol::ReviewDecision;
use cokra_protocol::UserInput;

use crate::app_event::AppEvent;
use crate::app_event::ExitMode;
use crate::app_event::UiMode;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::BottomPaneAction;
use crate::bottom_pane::approval_overlay::ApprovalOverlay;
use crate::bottom_pane::approval_overlay::ApprovalRequest;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::ChatWidgetAction;
use crate::chatwidget::TokenUsage;
use crate::custom_terminal::Frame;
use crate::history_cell::HistoryCell;
use crate::render::renderable::Renderable;
use crate::tui::FrameRequester;
use crate::tui::Tui;
use crate::tui::TuiEvent;

/// Baseline cadence for periodic stream commit animation ticks.
///
/// Smooth-mode streaming drains one line per tick, so this interval controls
/// perceived typing speed for non-backlogged output.
const COMMIT_ANIMATION_TICK: Duration = crate::tui::TARGET_FRAME_INTERVAL;

pub struct App {
  cokra: Cokra,
  chat_widget: ChatWidget,
  exit_info: Option<AppExitInfo>,
  app_event_rx: mpsc::UnboundedReceiver<AppEvent>,
  /// Sender used by the commit animation OS thread to deliver CommitTick events.
  app_event_tx: AppEventSender,
  /// Controls the animation thread that sends CommitTick events.
  commit_anim_running: Arc<AtomicBool>,
  task_running: bool,
  pending_approval: Option<PendingApproval>,
  ui_mode: UiMode,
  transcript_cells: Vec<Box<dyn HistoryCell>>,
  transcript_lines_cache: Vec<Line<'static>>,
  transcript_cache_width: u16,
  scroll_offset: u16,
  history_width: u16,
  has_emitted_history_lines: bool,
}

#[derive(Debug, Clone)]
struct PendingApproval {
  id: String,
  turn_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AppExitInfo {
  pub token_usage: TokenUsage,
  pub thread_id: Option<String>,
  pub exit_reason: ExitReason,
}

#[derive(Debug, Clone)]
pub enum ExitReason {
  UserRequested,
  Fatal(String),
}

impl App {
  pub fn new(cokra: Cokra, frame_requester: FrameRequester, ui_mode: UiMode) -> Self {
    let (app_event_tx, app_event_rx) = mpsc::unbounded_channel();
    let app_event_sender = AppEventSender::new(app_event_tx);

    Self {
      cokra,
      chat_widget: ChatWidget::new(app_event_sender.clone(), frame_requester, true),
      exit_info: None,
      app_event_rx,
      app_event_tx: app_event_sender,
      commit_anim_running: Arc::new(AtomicBool::new(false)),
      task_running: false,
      pending_approval: None,
      ui_mode,
      transcript_cells: Vec::new(),
      transcript_lines_cache: Vec::new(),
      transcript_cache_width: 0,
      scroll_offset: 0,
      history_width: 1,
      has_emitted_history_lines: false,
    }
  }

  pub(crate) async fn run(&mut self, tui: &mut Tui) -> Result<AppExitInfo> {
    let mut events = tui.event_stream();

    // Trigger an initial draw so the UI is visible before any events arrive.
    self.draw(tui)?;

    loop {
      if let Some(exit_info) = self.exit_info.take() {
        return Ok(exit_info);
      }

      if self.task_running {
        tokio::select! {
          Some(event) = events.next() => {
            self.handle_tui_event(event, tui).await?;
          }
          core_event = self.cokra.next_event() => {
            match core_event {
              Ok(event) => {
                self.handle_cokra_event(event).await?;
                // Status changes (TurnStarted, etc.) need a frame even if no
                // InsertHistoryCell was sent.
                tui.frame_requester().schedule_frame();
              }
              Err(err) => {
                self.exit_info = Some(self.build_exit_info(ExitReason::Fatal(err.to_string())));
              }
            }
          }
          Some(app_event) = self.app_event_rx.recv() => {
            self.handle_app_event(app_event, tui).await?;
          }
        }
      } else {
        tokio::select! {
          Some(event) = events.next() => {
            self.handle_tui_event(event, tui).await?;
          }
          Some(app_event) = self.app_event_rx.recv() => {
            self.handle_app_event(app_event, tui).await?;
          }
        }
      }
    }
  }

  fn draw(&mut self, tui: &mut Tui) -> Result<()> {
    let width = tui.terminal.size().map(|s| s.width).unwrap_or(80);
    self.history_width = width;
    if self.ui_mode == UiMode::AltScreen && self.transcript_cache_width != width {
      self.rebuild_transcript_cache(width);
    }
    let height = self.chat_widget.desired_height(width);
    tui.draw(height, |frame| self.render(frame))?;
    Ok(())
  }

  fn build_exit_info(&self, reason: ExitReason) -> AppExitInfo {
    AppExitInfo {
      token_usage: self.chat_widget.token_usage(),
      thread_id: self.cokra.thread_id().map(ToString::to_string),
      exit_reason: reason,
    }
  }

  async fn handle_app_event(&mut self, event: AppEvent, tui: &mut Tui) -> Result<()> {
    match event {
      AppEvent::CodexOp(op) => {
        let _ = self.cokra.submit(op).await?;
      }
      AppEvent::InsertHistoryCell(cell) => {
        self.insert_history_cell(cell, tui)?;
      }
      AppEvent::Exit(mode) => match mode {
        ExitMode::ShutdownFirst => {
          let _ = self.cokra.submit(Op::Shutdown).await?;
        }
        ExitMode::Immediate => {
          self.exit_info = Some(self.build_exit_info(ExitReason::UserRequested));
        }
      },
      AppEvent::FatalExitRequest(message) => {
        self.exit_info = Some(self.build_exit_info(ExitReason::Fatal(message)));
      }
      AppEvent::StartCommitAnimation => {
        // 1:1 codex: spawn an OS thread that sleeps and sends CommitTick events.
        // compare_exchange ensures only one animation thread runs at a time.
        if self
          .commit_anim_running
          .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
          .is_ok()
        {
          let tx = self.app_event_tx.clone();
          let running = self.commit_anim_running.clone();
          thread::spawn(move || {
            while running.load(Ordering::Relaxed) {
              thread::sleep(COMMIT_ANIMATION_TICK);
              tx.send(AppEvent::CommitTick);
            }
          });
        }
      }
      AppEvent::StopCommitAnimation => {
        // Lower the flag; the animation thread will exit its loop naturally.
        self.commit_anim_running.store(false, Ordering::Release);
      }
      AppEvent::CommitTick => {
        self.chat_widget.on_commit_tick();
        // CommitTick may have updated the active streaming cell or paused the status
        // indicator timer; schedule a frame so the change becomes visible.
        tui.frame_requester().schedule_frame();
      }
      AppEvent::OpenResumePicker | AppEvent::NewSession | AppEvent::ForkCurrentSession => {}
    }
    Ok(())
  }

  fn insert_history_cell(&mut self, cell: Box<dyn HistoryCell>, tui: &mut Tui) -> Result<()> {
    self.transcript_cells.push(cell);

    let width = self.history_width.max(1);
    let cell = self.transcript_cells.last().unwrap();
    let mut display = cell.display_lines(width);

    if display.is_empty() {
      return Ok(());
    }

    // 1:1 codex: only insert a separating blank line for new cells that are
    // not part of an ongoing stream. Streaming continuations should not
    // accrue extra blank lines between chunks.
    let needs_separator = !cell.is_stream_continuation();

    match self.ui_mode {
      UiMode::Inline => {
        if needs_separator {
          if self.has_emitted_history_lines {
            display.insert(0, Line::from(""));
          } else {
            self.has_emitted_history_lines = true;
          }
        }
        tui.insert_history_lines(display);
      }
      UiMode::AltScreen => {
        if needs_separator && self.has_emitted_history_lines {
          self.transcript_lines_cache.push(Line::from(""));
        }
        if needs_separator && !self.has_emitted_history_lines {
          self.has_emitted_history_lines = true;
        }
        self.transcript_lines_cache.extend(display);
        self.transcript_cache_width = width;
      }
    }

    Ok(())
  }

  fn rebuild_transcript_cache(&mut self, width: u16) {
    let width = width.max(1);
    self.transcript_lines_cache.clear();

    let mut emitted = false;
    for cell in self.transcript_cells.iter() {
      let lines = cell.display_lines(width);
      if lines.is_empty() {
        continue;
      }
      if !cell.is_stream_continuation() {
        if emitted {
          self.transcript_lines_cache.push(Line::from(""));
        } else {
          emitted = true;
        }
      }
      self.transcript_lines_cache.extend(lines);
    }

    self.transcript_cache_width = width;
  }

  async fn handle_tui_event(&mut self, event: TuiEvent, tui: &mut Tui) -> Result<()> {
    match event {
      TuiEvent::Key(key) => {
        self.handle_key_event(key).await?;
        // Schedule a redraw after every keypress so the compositor
        // reflects the updated textarea/cursor state immediately.
        tui.frame_requester().schedule_frame();
        if let Some(delay) = self.chat_widget.bottom_pane.next_footer_transition_in() {
          tui.frame_requester().schedule_frame_in(delay);
        }
      }
      TuiEvent::Paste(text) => {
        self.chat_widget.bottom_pane.handle_paste(text);
        // Paste begins a burst; schedule the first flush tick.
        tui.frame_requester().schedule_frame_in(
          crate::bottom_pane::chat_composer::ChatComposer::recommended_paste_flush_delay(),
        );
        if let Some(delay) = self.chat_widget.bottom_pane.next_footer_transition_in() {
          tui.frame_requester().schedule_frame_in(delay);
        }
      }
      TuiEvent::Draw => {
        // 1:1 codex: flush paste burst first; if something flushed, redraw immediately.
        // If still in a burst (first char held for flicker suppression), schedule a
        // follow-up tick so the held char is eventually released even without a keypress.
        if self.chat_widget.bottom_pane.flush_paste_burst_if_due() {
          // Something flushed — schedule an immediate redraw and skip this frame.
          tui.frame_requester().schedule_frame();
          return Ok(());
        }
        if self.chat_widget.bottom_pane.is_in_paste_burst() {
          tui.frame_requester().schedule_frame_in(
            crate::bottom_pane::chat_composer::ChatComposer::recommended_paste_flush_delay(),
          );
          return Ok(());
        }
        self.draw(tui)?;
        if let Some(delay) = self.chat_widget.bottom_pane.next_footer_transition_in() {
          tui.frame_requester().schedule_frame_in(delay);
        }
      }
    }

    Ok(())
  }

  async fn handle_key_event(&mut self, key: KeyEvent) -> Result<()> {
    // 1:1 codex: only forward Press and Repeat events; silently drop Release.
    // In keyboard-enhancement mode crossterm emits Press + Release pairs; without
    // this guard every keystroke would be processed twice.
    match key.kind {
      KeyEventKind::Press | KeyEventKind::Repeat => {}
      KeyEventKind::Release => return Ok(()),
    }

    if self.ui_mode == UiMode::AltScreen {
      match key.code {
        KeyCode::PageUp => {
          self.scroll_offset = self.scroll_offset.saturating_add(10);
          return Ok(());
        }
        KeyCode::PageDown => {
          self.scroll_offset = self.scroll_offset.saturating_sub(10);
          return Ok(());
        }
        KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
          self.scroll_offset = self.scroll_offset.saturating_add(3);
          return Ok(());
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
          self.scroll_offset = self.scroll_offset.saturating_sub(3);
          return Ok(());
        }
        KeyCode::Home => {
          self.scroll_offset = u16::MAX;
          return Ok(());
        }
        KeyCode::End => {
          self.scroll_offset = 0;
          return Ok(());
        }
        _ => {}
      }
    }

    match self.chat_widget.bottom_pane.handle_key(key) {
      BottomPaneAction::None => {}
      BottomPaneAction::Interrupt => {
        if self.task_running {
          let _ = self.cokra.submit(Op::Interrupt).await?;
        } else {
          self.exit_info = Some(self.build_exit_info(ExitReason::UserRequested));
        }
      }
      BottomPaneAction::RequestQuit => {
        self.exit_info = Some(self.build_exit_info(ExitReason::UserRequested));
      }
      BottomPaneAction::Submit(submission) => {
        self
          .submit_user_input(submission.text, submission.text_elements)
          .await?;
      }
      BottomPaneAction::ApprovalDecision(choice) => {
        if let Some(pending) = self.pending_approval.take() {
          let decision = match choice {
            crate::bottom_pane::approval_overlay::ApprovalChoice::Allow => ReviewDecision::Approved,
            crate::bottom_pane::approval_overlay::ApprovalChoice::AllowAlways => {
              ReviewDecision::Always
            }
            crate::bottom_pane::approval_overlay::ApprovalChoice::Deny => ReviewDecision::Denied,
          };
          let _ = self
            .cokra
            .submit(Op::ExecApproval {
              id: pending.id,
              turn_id: pending.turn_id,
              decision,
            })
            .await?;
        }
      }
    }

    Ok(())
  }

  async fn submit_user_input(
    &mut self,
    text: String,
    text_elements: Vec<cokra_protocol::TextElement>,
  ) -> Result<()> {
    if text.trim().is_empty() {
      return Ok(());
    }

    self.scroll_offset = 0;
    self.chat_widget.push_user_input_text(text.clone());

    let _ = self
      .cokra
      .submit(Op::UserInput {
        items: vec![UserInput::Text {
          text,
          text_elements,
        }],
        final_output_json_schema: None,
      })
      .await?;

    self.task_running = true;
    self.chat_widget.set_agent_turn_running(true);

    Ok(())
  }

  async fn handle_cokra_event(&mut self, event: Event) -> Result<()> {
    let turn_finished = matches!(
      event.msg,
      EventMsg::TurnComplete(_) | EventMsg::TurnAborted(_) | EventMsg::Error(_)
    );

    if let Some(action) = self.chat_widget.handle_event(&event.msg) {
      match action {
        ChatWidgetAction::ShowApproval(req) => {
          self.pending_approval = Some(PendingApproval {
            id: req.id.clone(),
            turn_id: Some(req.turn_id.clone()),
          });

          self
            .chat_widget
            .bottom_pane
            .open_approval(ApprovalOverlay::new(ApprovalRequest {
              call_id: req.id,
              tool_name: "shell".to_string(),
              command: format!("{}  (cwd: {})", req.command, req.cwd.display()),
            }));
        }
      }
    }

    self.sync_bottom_pane_context();

    if turn_finished {
      self.task_running = false;
      self.chat_widget.set_agent_turn_running(false);
    }

    Ok(())
  }

  fn sync_bottom_pane_context(&mut self) {
    let usage = self.chat_widget.token_usage();
    let used_tokens = if usage.total_tokens > 0 {
      Some(usage.total_tokens)
    } else if usage.is_zero() {
      None
    } else {
      Some(usage.input_tokens.saturating_add(usage.output_tokens))
    };
    self.chat_widget.bottom_pane.set_context_window(None, used_tokens);
  }

  fn render(&mut self, frame: &mut Frame) {
    let area = frame.area();
    self.history_width = area.width;

    match self.ui_mode {
      UiMode::Inline => {
        // 1:1 codex: ChatWidget.as_renderable() composes active_cell + bottom_pane
        // via FlexRenderable. App only calls render and sets cursor.
        self.chat_widget.render(area, frame.buffer_mut());
        if let Some((x, y)) = self.chat_widget.cursor_pos(area) {
          frame.set_cursor_position((x, y));
        }
      }
      UiMode::AltScreen => {
        if self.transcript_cache_width != area.width {
          self.rebuild_transcript_cache(area.width);
        }
        let bottom_height = self
          .chat_widget
          .bottom_pane
          .desired_height(area.width)
          .min(area.height);
        let chunks = Layout::vertical([
          Constraint::Min(1),
          Constraint::Length(bottom_height),
        ])
        .split(area);
        self.chat_widget.render_alt_screen(
          chunks[0],
          frame.buffer_mut(),
          &self.transcript_lines_cache,
          self.scroll_offset,
        );
        self.chat_widget.bottom_pane.render(
          chunks[1],
          frame.buffer_mut(),
          self.task_running,
        );
        if let Some((x, y)) = self.chat_widget.bottom_pane.cursor_pos(chunks[1]) {
          frame.set_cursor_position((x, y));
        }
      }
    }
  }
}

pub(crate) fn make_frame_requester() -> (broadcast::Sender<()>, FrameRequester) {
  let (draw_tx, _draw_rx) = broadcast::channel(64);
  let frame_requester = FrameRequester::new(draw_tx.clone());
  (draw_tx, frame_requester)
}

pub(crate) fn default_cwd() -> PathBuf {
  std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}
