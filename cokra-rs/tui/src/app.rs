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
use ratatui::prelude::Stylize;
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
use crate::slash_command::SlashCommand;
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

  async fn apply_model_selection_or_connect(
    &mut self,
    model_id: String,
    effort: Option<cokra_protocol::ReasoningEffortConfig>,
  ) -> Result<()> {
    let provider_id = model_id.split('/').next().unwrap_or("").to_string();
    if provider_id.is_empty() {
      self.apply_model_selection(model_id).await?;
      return Ok(());
    }

    let has_provider = self
      .cokra
      .model_client()
      .registry()
      .has_provider(&provider_id)
      .await;
    if has_provider {
      self.apply_model_selection(model_id).await?;
      return Ok(());
    }

    self.open_api_key_entry(provider_id, model_id, effort);
    Ok(())
  }

  fn open_api_key_entry(
    &mut self,
    provider_id: String,
    model_id: String,
    effort: Option<cokra_protocol::ReasoningEffortConfig>,
  ) {
    use crate::bottom_pane::api_key_entry_view::ApiKeyEntryView;

    let view = ApiKeyEntryView::new(
      provider_id,
      model_id,
      effort,
      self.app_event_tx.clone(),
    );
    self
      .chat_widget
      .bottom_pane
      .push_view(Box::new(view));
  }

  async fn register_provider_with_api_key(
    &mut self,
    provider_id: &str,
    api_key: String,
  ) -> Result<()> {
    use cokra_core::model::providers::AnthropicProvider;
    use cokra_core::model::providers::GitHubCopilotProvider;
    use cokra_core::model::providers::GoogleProvider;
    use cokra_core::model::providers::OpenAIProvider;
    use cokra_core::model::providers::OpenRouterProvider;
    use cokra_core::model::ProviderConfig;

    let config = ProviderConfig {
      provider_id: provider_id.to_string(),
      api_key: Some(api_key.clone()),
      base_url: None,
      ..Default::default()
    };

    match provider_id {
      "openai" => {
        let provider = OpenAIProvider::new(api_key, config.clone());
        self
          .cokra
          .model_client()
          .registry()
          .register_with_config(provider, config)
          .await;
      }
      "anthropic" => {
        let provider = AnthropicProvider::new(api_key, config.clone());
        self
          .cokra
          .model_client()
          .registry()
          .register_with_config(provider, config)
          .await;
      }
      "openrouter" => {
        let provider = OpenRouterProvider::new(api_key, config.clone());
        self
          .cokra
          .model_client()
          .registry()
          .register_with_config(provider, config)
          .await;
      }
      "google" => {
        let provider = GoogleProvider::new(api_key, config.clone());
        self
          .cokra
          .model_client()
          .registry()
          .register_with_config(provider, config)
          .await;
      }
      "github" => {
        let provider = GitHubCopilotProvider::new(api_key, config.clone());
        self
          .cokra
          .model_client()
          .registry()
          .register_with_config(provider, config)
          .await;
      }
      _ => {
        let provider = OpenAIProvider::new(api_key, config.clone());
        self
          .cokra
          .model_client()
          .registry()
          .register_with_config(provider, config)
          .await;
      }
    }

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
      AppEvent::OpenAllModelsPopup { providers } => {
        self.open_all_models_popup(providers);
      }
      AppEvent::OpenReasoningPopup { model_id } => {
        self.open_reasoning_popup(model_id);
      }
      AppEvent::ApplyModelSelection { model_id, effort } => {
        self.apply_model_selection_or_connect(model_id, effort).await?;
      }
      AppEvent::ApiKeySubmitted {
        provider_id,
        api_key,
        model_id,
        effort,
      } => {
        self
          .register_provider_with_api_key(&provider_id, api_key)
          .await?;
        self.apply_model_selection_or_connect(model_id, effort).await?;
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
      }
      AppEvent::OpenResumePicker => {
        self.chat_widget.open_resume_picker();
      }
      AppEvent::NewSession => {
        // Clear transcript and push a new session cell.
        self.transcript_cells.clear();
        self.transcript_lines_cache.clear();
        self.has_emitted_history_lines = false;
        self.chat_widget.add_to_history(crate::history_cell::PlainHistoryCell::new(vec![
          Line::from("• New session".dim()),
        ]));
      }
      AppEvent::ForkCurrentSession => {
        self.chat_widget.add_to_history(crate::history_cell::PlainHistoryCell::new(vec![
          Line::from("• Fork current session (not implemented)".dim()),
        ]));
      }
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
        // If a paste-burst hold is active (e.g. first fast char like '/'),
        // schedule a follow-up flush tick so the held char is released
        // even if no further keypress arrives.
        if self.chat_widget.bottom_pane.is_in_paste_burst() {
          tui.frame_requester().schedule_frame_in(
            crate::bottom_pane::chat_composer::ChatComposer::recommended_paste_flush_delay(),
          );
        }
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
      BottomPaneAction::SlashCommand(cmd) => {
        self.dispatch_command(cmd).await?;
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

  // 1:1 codex: dispatch_command handles all slash commands at the app layer.
  async fn dispatch_command(&mut self, cmd: SlashCommand) -> Result<()> {
    if !cmd.available_during_task() && self.task_running {
      self.chat_widget.add_to_history(
        crate::history_cell::PlainHistoryCell::new(vec![Line::from(vec![
          ratatui::text::Span::from(format!("'/{}' is disabled while a task is in progress.", cmd.command())).into(),
        ])]),
      );
      return Ok(());
    }

    match cmd {
      SlashCommand::Model => {
        self.open_model_popup().await?;
      }
      SlashCommand::New => {
        self.app_event_tx.send(AppEvent::NewSession);
      }
      SlashCommand::Resume => {
        self.app_event_tx.send(AppEvent::OpenResumePicker);
      }
      SlashCommand::Fork => {
        self.app_event_tx.send(AppEvent::ForkCurrentSession);
      }
      SlashCommand::Compact => {
        let _ = self.cokra.submit(Op::Interrupt).await?;
        self.chat_widget.add_to_history(
          crate::history_cell::PlainHistoryCell::new(vec![Line::from(
            "• Context compacted".dim(),
          )]),
        );
      }
      SlashCommand::Quit | SlashCommand::Exit => {
        self.exit_info = Some(self.build_exit_info(ExitReason::UserRequested));
      }
      SlashCommand::Status => {
        let usage = self.chat_widget.token_usage();
        let model = &self.chat_widget.model_name;
        let lines = vec![
          Line::from(format!("• Model: {model}")),
          Line::from(format!(
            "• Tokens: {} input, {} output, {} total",
            usage.input_tokens, usage.output_tokens, usage.total_tokens
          )),
          Line::from(format!("• Task running: {}", self.task_running)),
        ];
        self.chat_widget.add_to_history(
          crate::history_cell::PlainHistoryCell::new(lines),
        );
      }
      SlashCommand::Diff => {
        self.chat_widget.add_to_history(
          crate::history_cell::PlainHistoryCell::new(vec![Line::from(
            "• /diff is not yet implemented.".dim(),
          )]),
        );
      }
      // All other commands: show not-yet-implemented message.
      _ => {
        self.chat_widget.add_to_history(
          crate::history_cell::PlainHistoryCell::new(vec![Line::from(
            format!("• /{}  — not yet implemented.", cmd.command()).dim(),
          )]),
        );
      }
    }
    Ok(())
  }

  async fn open_model_popup(&mut self) -> Result<()> {
    use crate::bottom_pane::list_selection_view::SelectionAction;
    use crate::bottom_pane::list_selection_view::SelectionItem;
    use crate::bottom_pane::list_selection_view::SelectionViewParams;
    use crate::bottom_pane::popup_consts::standard_popup_hint_line;

    let providers = self
      .cokra
      .model_client()
      .registry()
      .list_models_catalog()
      .await;
    if providers.is_empty() {
      self.chat_widget.add_to_history(
        crate::history_cell::PlainHistoryCell::new(vec![Line::from(vec![
          ratatui::text::Span::from("No models found. ").red(),
          ratatui::text::Span::from(
            "models.dev database is empty or unavailable; try again later.",
          ),
        ])]),
      );
      return Ok(());
    }

    let current_model = self.chat_widget.model_name.clone();

    let mut all_model_ids: std::collections::HashSet<String> =
      std::collections::HashSet::new();
    for provider in &providers {
      for model in &provider.models {
        let model_id = if model.starts_with(&format!("{}/", provider.id)) {
          model.clone()
        } else {
          format!("{}/{}", provider.id, model)
        };
        all_model_ids.insert(model_id);
      }
    }

    let find_candidate = |candidates: &[&str]| -> Option<String> {
      candidates
        .iter()
        .find(|id| all_model_ids.contains(**id))
        .map(|id| id.to_string())
    };

    let fast = find_candidate(&[
      "openai/gpt-4o-mini",
      "openrouter/openai/gpt-4o-mini",
      "github/gpt-4o-mini",
    ]);
    let balanced = find_candidate(&[
      "openai/gpt-4o",
      "openrouter/openai/gpt-4o",
      "github/gpt-4o",
    ]);
    let thorough = find_candidate(&[
      "openai/o3",
      "openai/o1",
      "openrouter/openai/o1",
      "github/o1-2024-12-17",
    ]);

    let mut items: Vec<SelectionItem> = Vec::new();
    let mut push_auto = |label: &str, model_id: Option<String>| {
      let Some(model_id) = model_id else {
        return;
      };
      let model_for_action = model_id.clone();
      let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(AppEvent::ApplyModelSelection {
          model_id: model_for_action.clone(),
          effort: None,
        });
      })];
      items.push(SelectionItem {
        name: label.to_string(),
        description: Some(model_id.clone()),
        is_current: model_id == current_model,
        actions,
        dismiss_on_select: true,
        search_value: Some(model_id),
        ..Default::default()
      });
    };

    push_auto("Auto fast", fast);
    push_auto("Auto balanced", balanced);
    push_auto("Auto thorough", thorough);

    let providers_for_popup = providers.clone();
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
      tx.send(AppEvent::OpenAllModelsPopup {
        providers: providers_for_popup.clone(),
      });
    })];
    let is_current = !items.iter().any(|item| item.is_current);
    items.push(SelectionItem {
      name: "All models".to_string(),
      description: Some(format!(
        "Choose a specific model and reasoning level (current: {current_model})"
      )),
      is_current,
      actions,
      dismiss_on_select: true,
      ..Default::default()
    });

    self.chat_widget.bottom_pane.show_selection_view(SelectionViewParams {
      title: Some("Select Model".to_string()),
      subtitle: Some("Pick a quick auto mode or browse all models.".to_string()),
      footer_hint: Some(standard_popup_hint_line()),
      items,
      ..Default::default()
    });
    Ok(())
  }

  fn open_all_models_popup(&mut self, providers: Vec<cokra_core::model::ProviderInfo>) {
    use crate::bottom_pane::list_selection_view::SelectionAction;
    use crate::bottom_pane::list_selection_view::SelectionItem;
    use crate::bottom_pane::list_selection_view::SelectionViewParams;
    use crate::bottom_pane::popup_consts::standard_popup_hint_line;

    let current_model = self.chat_widget.model_name.clone();

    let mut items: Vec<SelectionItem> = Vec::new();
    for provider in &providers {
      for (idx, model) in provider.models.iter().enumerate() {
        let model_id = if model.starts_with(&format!("{}/", provider.id)) {
          model.clone()
        } else {
          format!("{}/{}", provider.id, model)
        };
        let model_for_action = model_id.clone();
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
          tx.send(AppEvent::OpenReasoningPopup {
            model_id: model_for_action.clone(),
          });
        })];
        items.push(SelectionItem {
          name: model_id.clone(),
          description: (idx == 0).then_some(provider.name.clone()),
          is_current: model_id == current_model,
          actions,
          dismiss_on_select: true,
          search_value: Some(model_id),
          ..Default::default()
        });
      }
    }

    self.chat_widget.bottom_pane.show_selection_view(SelectionViewParams {
      title: Some("Select Model and Effort".to_string()),
      subtitle: Some("Type to search models.".to_string()),
      footer_hint: Some(standard_popup_hint_line()),
      items,
      is_searchable: true,
      search_placeholder: Some("Type to search models".to_string()),
      ..Default::default()
    });
  }

  fn open_reasoning_popup(&mut self, model_id: String) {
    use crate::bottom_pane::list_selection_view::SelectionAction;
    use crate::bottom_pane::list_selection_view::SelectionItem;
    use crate::bottom_pane::list_selection_view::SelectionViewParams;
    use crate::bottom_pane::popup_consts::standard_popup_hint_line;

    let mut items: Vec<SelectionItem> = Vec::new();
    let push_effort = |items: &mut Vec<SelectionItem>,
                       label: &str,
                       effort: Option<cokra_protocol::ReasoningEffortConfig>| {
      let model_for_action = model_id.clone();
      let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(AppEvent::ApplyModelSelection {
          model_id: model_for_action.clone(),
          effort: effort.clone(),
        });
      })];
      items.push(SelectionItem {
        name: label.to_string(),
        actions,
        dismiss_on_select: true,
        ..Default::default()
      });
    };

    push_effort(&mut items, "Default", None);
    push_effort(
      &mut items,
      "Minimal",
      Some(cokra_protocol::ReasoningEffortConfig {
        effort: cokra_protocol::ReasoningEffort::Minimal,
      }),
    );
    push_effort(
      &mut items,
      "Low",
      Some(cokra_protocol::ReasoningEffortConfig {
        effort: cokra_protocol::ReasoningEffort::Low,
      }),
    );
    push_effort(
      &mut items,
      "Medium",
      Some(cokra_protocol::ReasoningEffortConfig {
        effort: cokra_protocol::ReasoningEffort::Medium,
      }),
    );
    push_effort(
      &mut items,
      "High",
      Some(cokra_protocol::ReasoningEffortConfig {
        effort: cokra_protocol::ReasoningEffort::High,
      }),
    );

    self.chat_widget.bottom_pane.show_selection_view(SelectionViewParams {
      title: Some("Select Reasoning Effort".to_string()),
      subtitle: Some(model_id),
      footer_hint: Some(standard_popup_hint_line()),
      items,
      ..Default::default()
    });
  }

  // 1:1 codex: model_selection_actions sends OverrideTurnContext + UpdateModel +
  // PersistModelSelection. We collapse into a single method that:
  //   1. Submits Op::OverrideTurnContext to the core agent loop.
  //   2. Updates ModelClient default provider (opencode-style persistence).
  //   3. Updates the local model_name for immediate UI feedback.
  //   4. Shows a confirmation message in chat history.
  async fn apply_model_selection(&mut self, model_id: String) -> Result<()> {
    // 1) Send OverrideTurnContext to switch the active model.
    let _ = self
      .cokra
      .submit(Op::OverrideTurnContext {
        cwd: None,
        approval_policy: None,
        sandbox_policy: None,
        model: Some(model_id.clone()),
        collaboration_mode: None,
        personality: None,
      })
      .await?;

    // 2) Persist selection: update ModelClient default provider.
    //    The model_id may be "provider/model" — extract provider portion.
    if let Some(provider_id) = model_id.split('/').next() {
      let _ = self
        .cokra
        .model_client()
        .set_default_provider(provider_id)
        .await;
    }

    // 3) Update local model name so the UI reflects the change immediately.
    self.chat_widget.model_name = model_id.clone();

    // 4) Show confirmation in chat history.
    self.chat_widget.add_to_history(
      crate::history_cell::PlainHistoryCell::new(vec![Line::from(format!(
        "• Model changed to {model_id}"
      ))]),
    );
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
