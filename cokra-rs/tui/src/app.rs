use std::collections::HashMap;
use std::collections::VecDeque;
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
use ratatui::text::Span;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

use cokra_core::Cokra;
use cokra_core::model::ModelCatalogEntry;
use cokra_core::model::ProviderAuth;
use cokra_core::model::auth_orchestrator::uses_local_callback;
use cokra_core::model::oauth_connect::OAuthConnectStart;
use cokra_core::model::oauth_connect::PendingOAuthConnect;
use cokra_core::model::provider::ProviderConnectMethod;
use cokra_core::model::provider_catalog::find_provider_catalog_entry;
use cokra_protocol::Event;
use cokra_protocol::EventMsg;
use cokra_protocol::ExecApprovalRequestEvent;
use cokra_protocol::Op;
use cokra_protocol::RequestUserInputEvent;
use cokra_protocol::ReviewDecision;
use cokra_protocol::UserInput;

use crate::app_event::AppEvent;
use crate::app_event::ExitMode;
use crate::app_event::StatusLineMode;
use crate::app_event::UiMode;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::BottomPaneAction;
use crate::bottom_pane::approval_overlay::ApprovalRequest;
use crate::bottom_pane::chat_composer::ComposerSubmission;
use crate::bottom_pane::footer::InlineFooterStatus;
use crate::chatwidget::ActiveCellTranscriptKey;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::ChatWidgetAction;
use crate::chatwidget::StreamRenderMode;
use crate::chatwidget::TokenUsage;
use crate::custom_terminal::Frame;
use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;
use crate::path_utils::get_git_branch;
use crate::render::renderable::Renderable;
use crate::slash_command::SlashCommand;
use crate::tui::FrameRequester;
use crate::tui::InlineViewportSizing;
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
  inline_stable_height: u16,
  pending_approval: Option<PendingApproval>,
  pending_approval_request: Option<ExecApprovalRequestEvent>,
  pending_user_input_request: Option<RequestUserInputEvent>,
  prompt_queue: VecDeque<PromptRequest>,
  ui_mode: UiMode,
  transcript_cells: Vec<Box<dyn HistoryCell>>,
  transcript_lines_cache: Vec<Line<'static>>,
  active_tail_cache_key: Option<ActiveCellTranscriptKey>,
  active_tail_cache_width: u16,
  active_tail_cache_lines: Vec<Line<'static>>,
  deferred_history_lines: Vec<Line<'static>>,
  transcript_cache_width: u16,
  scroll_offset: u16,
  history_width: u16,
  has_emitted_history_lines: bool,
  backtrack_render_pending: bool,
  status_line_mode: StatusLineMode,
  queued_submissions: VecDeque<ComposerSubmission>,
  footer_effort_label: String,
  footer_model_catalog: HashMap<String, ModelCatalogEntry>,
  footer_context_window_limit: HashMap<String, Option<i64>>,
  primary_thread_id: String,
  active_thread_id: String,
  primary_active_turn_id: Option<String>,
  status_line_agent_selector_active: bool,
  thread_event_stores: HashMap<String, ThreadEventStore>,
  agent_picker_threads: HashMap<String, crate::multi_agents::AgentPickerThreadEntry>,
  background_pending_threads: HashMap<String, BackgroundPending>,
  background_approval_requests: HashMap<String, Vec<ExecApprovalRequestEvent>>,
  background_user_input_requests: HashMap<String, Vec<RequestUserInputEvent>>,
  pending_oauth_flows: HashMap<String, PendingOAuthFlowState>,
  did_show_welcome: bool,
}

#[derive(Debug, Clone)]
struct PendingApproval {
  id: String,
  turn_id: Option<String>,
}

#[derive(Debug, Clone)]
enum PromptRequest {
  ExecApproval(ExecApprovalRequestEvent),
  UserInput(RequestUserInputEvent),
}

#[derive(Debug, Clone, Default)]
struct BackgroundPending {
  approval_count: usize,
  user_input_count: usize,
}

#[derive(Debug, Clone)]
struct PendingOAuthFlowState {
  pending: PendingOAuthConnect,
  cancel: CancellationToken,
}

#[derive(Debug, Clone)]
struct ThreadEventSnapshot {
  session_configured: Option<Event>,
  events: Vec<Event>,
}

#[derive(Debug)]
struct ThreadEventStore {
  session_configured: Option<Event>,
  buffer: VecDeque<Event>,
}

impl ThreadEventStore {
  fn new() -> Self {
    Self {
      session_configured: None,
      buffer: VecDeque::new(),
    }
  }

  fn push_event(&mut self, event: Event) {
    if matches!(event.msg, EventMsg::SessionConfigured(_)) {
      self.session_configured = Some(event);
      return;
    }

    self.buffer.push_back(event);
    // Tradeoff: keep the full per-thread event log so switching agent views can
    // faithfully rebuild the entire transcript instead of silently dropping
    // early history once a long discussion exceeds a ring-buffer cap.
  }

  fn snapshot(&self) -> ThreadEventSnapshot {
    ThreadEventSnapshot {
      session_configured: self.session_configured.clone(),
      events: self.buffer.iter().cloned().collect(),
    }
  }
}

fn cycle_agent_thread(
  thread_ids: &[String],
  current_thread_id: &str,
  direction: isize,
) -> Option<String> {
  if thread_ids.len() < 2 {
    return None;
  }

  let current_idx = thread_ids
    .iter()
    .position(|thread_id| thread_id == current_thread_id)
    .unwrap_or(0) as isize;
  // Tradeoff: wrap around the footer member strip so repeated Left/Right presses
  // can keep cycling through a team without forcing the user to re-arm at the ends.
  let next_idx = (current_idx + direction).rem_euclid(thread_ids.len() as isize) as usize;
  thread_ids.get(next_idx).cloned()
}

fn status_line_agent_tabs(
  threads: &[(String, crate::multi_agents::AgentPickerThreadEntry)],
  primary_thread_id: &str,
  active_thread_id: &str,
  selector_active: bool,
) -> Vec<Span<'static>> {
  let mut spans = Vec::new();
  for (idx, (thread_id, entry)) in threads.iter().enumerate() {
    if idx > 0 {
      spans.push(Span::from(" ").dim());
    }

    let label = crate::multi_agents::format_agent_picker_item_name(
      entry.nickname.as_deref(),
      entry.role.as_deref(),
      thread_id == primary_thread_id,
    );
    let label = if thread_id == active_thread_id {
      format!("[{label}]")
    } else {
      label
    };

    let mut span = Span::from(label);
    if entry.is_closed && thread_id != active_thread_id {
      span = span.dim();
    }
    if thread_id == active_thread_id {
      span = span.fg(crate::terminal_palette::light_blue()).bold();
      if selector_active {
        span = span.underlined();
      }
    }
    spans.push(span);
  }

  spans
}

fn footer_effort_label(effort: Option<&cokra_protocol::ReasoningEffortConfig>) -> Option<String> {
  let effort = effort?;
  let label = match effort.effort {
    cokra_protocol::ReasoningEffort::Minimal => "minimal",
    cokra_protocol::ReasoningEffort::Low => "low",
    cokra_protocol::ReasoningEffort::Medium => "medium",
    cokra_protocol::ReasoningEffort::High => "high",
  };
  Some(label.to_string())
}

fn configured_footer_effort_label(_config: &cokra_config::Config) -> String {
  "medium".to_string()
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
    let footer_effort_label = configured_footer_effort_label(cokra.config());
    let primary_thread_id = cokra
      .thread_id()
      .map(ToString::to_string)
      .unwrap_or_else(|| "main".to_string());
    let mut thread_event_stores = HashMap::new();
    thread_event_stores.insert(primary_thread_id.clone(), ThreadEventStore::new());

    let mut app = Self {
      cokra,
      chat_widget: ChatWidget::new(
        app_event_sender.clone(),
        frame_requester,
        true,
        match ui_mode {
          UiMode::Inline => StreamRenderMode::ScrollbackFirst,
          UiMode::AltScreen => StreamRenderMode::AnimatedPreview,
        },
      ),
      exit_info: None,
      app_event_rx,
      app_event_tx: app_event_sender,
      commit_anim_running: Arc::new(AtomicBool::new(false)),
      task_running: false,
      inline_stable_height: 0,
      pending_approval: None,
      pending_approval_request: None,
      pending_user_input_request: None,
      prompt_queue: VecDeque::new(),
      ui_mode,
      transcript_cells: Vec::new(),
      transcript_lines_cache: Vec::new(),
      active_tail_cache_key: None,
      active_tail_cache_width: 0,
      active_tail_cache_lines: Vec::new(),
      deferred_history_lines: Vec::new(),
      transcript_cache_width: 0,
      scroll_offset: 0,
      history_width: 1,
      has_emitted_history_lines: false,
      backtrack_render_pending: false,
      status_line_mode: StatusLineMode::Off,
      queued_submissions: VecDeque::new(),
      footer_effort_label,
      footer_model_catalog: HashMap::new(),
      footer_context_window_limit: HashMap::new(),
      primary_thread_id: primary_thread_id.clone(),
      active_thread_id: primary_thread_id,
      primary_active_turn_id: None,
      status_line_agent_selector_active: false,
      thread_event_stores,
      agent_picker_threads: HashMap::new(),
      background_pending_threads: HashMap::new(),
      background_approval_requests: HashMap::new(),
      background_user_input_requests: HashMap::new(),
      pending_oauth_flows: HashMap::new(),
      did_show_welcome: false,
    };

    app.remember_agent_thread(
      app.primary_thread_id.clone(),
      Some("main".to_string()),
      Some("leader".to_string()),
      false,
    );

    // Claude Code-style footer by default (no status line).
    app.refresh_status_line();

    app
  }

  pub(crate) async fn run(&mut self, tui: &mut Tui) -> Result<AppExitInfo> {
    let mut events = tui.event_stream();

    self.insert_startup_welcome(tui)?;

    // Trigger an initial draw so the UI is visible before any events arrive.
    self.draw(tui)?;

    loop {
      if let Some(exit_info) = self.exit_info.take() {
        return Ok(exit_info);
      }

      tokio::select! {
        Some(event) = events.next() => {
          self.handle_tui_event(event, tui).await?;
        }
        core_event = self.cokra.next_event() => {
          match core_event {
            Ok(event) => {
              let redraw_inline_now =
                self.ui_mode == UiMode::Inline
                  && matches!(
                    &event.msg,
                    EventMsg::ExecCommandBegin(_) | EventMsg::ExecCommandEnd(_)
                  );
              self.handle_cokra_event(event).await?;
              // 1:1 codex streaming flush: after handling a core event in Inline mode,
              // immediately drain any InsertHistoryCell events that were produced by the
              // handler (e.g. from on_agent_message_delta -> append_boxed_history) and
              // write them directly into scrollback. This eliminates the two-round-trip
              // delay (app_event_rx -> TuiEvent::Draw) that makes streaming appear sluggish
              // when LLM tokens arrive faster than the tokio::select! scheduling rate.
              if self.ui_mode == UiMode::Inline {
                self.flush_inline_history_cells(tui);
                // If history lines were staged, immediately flush them to the terminal
                // rather than waiting for the next TuiEvent::Draw. This removes the
                // second round-trip delay and makes each committed line appear on screen
                // in the same scheduling cycle it was produced.
                if tui.has_pending_history_lines() {
                  self.draw(tui)?;
                }
              }
              if redraw_inline_now {
                self.draw(tui)?;
              }
              // SessionConfigured arrives before the first user turn starts, so
              // the app must keep polling core events even while idle. Always
              // schedule a frame after handling core events to surface startup
              // history cells and status changes immediately.
              tui.frame_requester().schedule_frame();
            }
            Err(err) => {
              self.exit_info = Some(self.build_exit_info(ExitReason::Fatal(err.to_string())));
            }
          }
        }
        Some(app_event) = self.app_event_rx.recv() => {
          self.handle_app_event(app_event, tui).await?;
          tui.frame_requester().schedule_frame();
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
    let requested_height = self.chat_widget.desired_height(width);
    let height = if self.ui_mode == UiMode::Inline {
      if self.task_running {
        self.inline_stable_height = self.inline_stable_height.max(requested_height);
        self.inline_stable_height
      } else {
        self.inline_stable_height = 0;
        requested_height
      }
    } else {
      requested_height
    };
    let sizing = if self.ui_mode == UiMode::Inline {
      self.chat_widget.inline_viewport_sizing()
    } else {
      InlineViewportSizing::PreserveVisibleHistory
    };
    tui.draw(height, sizing, |frame| self.render(frame))?;
    Ok(())
  }

  fn insert_startup_welcome(&mut self, tui: &mut Tui) -> Result<()> {
    if self.did_show_welcome {
      return Ok(());
    }
    self.did_show_welcome = true;

    let width = tui.terminal.size().map(|s| s.width).unwrap_or(80).max(1);
    let ctx = crate::welcome::WelcomeContext::from_config(self.cokra.config());
    let cell = crate::welcome::WelcomeWidget::into_history_cell(ctx);
    self.stage_history_cell(cell, width, Some(tui));
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
      if let Some(label) = footer_effort_label(effort.as_ref()) {
        self.footer_effort_label = label;
      }
      self.refresh_status_line();
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
      if let Some(label) = footer_effort_label(effort.as_ref()) {
        self.footer_effort_label = label;
      }
      self.refresh_status_line();
      return Ok(());
    }

    // Provider not wired yet: attempt a lazy runtime registration from stored
    // connect-catalog credentials (pi-mono parity for connected providers).
    match ProviderAuth::ensure_runtime_registered(
      self.cokra.model_client().registry(),
      self.cokra.config(),
      &provider_id,
    )
    .await
    {
      Ok(true) => {
        self.apply_model_selection(model_id).await?;
        if let Some(label) = footer_effort_label(effort.as_ref()) {
          self.footer_effort_label = label;
        }
        self.refresh_status_line();
        return Ok(());
      }
      Ok(false) => {}
      Err(err) => {
        self
          .chat_widget
          .add_to_history(PlainHistoryCell::new(vec![Line::from(
            format!("● Failed to wire provider runtime for {provider_id}: {err}").red(),
          )]));
        // Tradeoff: don't early-return here; if wiring fails we still want to fall back
        // to the provider's connect flow so users can re-auth / reconfigure in-TUI.
      }
    }

    // Fallback: open the appropriate connect flow for this provider if known.
    // Use ProviderAuth::find_connect_entry to support both runtime ids ("github")
    // and connect catalog ids ("github-copilot").
    if let Some(entry) = ProviderAuth::find_connect_entry(&provider_id) {
      let connect_id = entry.id.to_string();
      match entry.connect_method {
        ProviderConnectMethod::OAuth => {
          // start_oauth_connect will surface any missing env/config as an in-TUI error.
          if let Err(err) = self.start_oauth_connect(&connect_id).await {
            self
              .chat_widget
              .add_to_history(PlainHistoryCell::new(vec![Line::from(
                format!("● OAuth connect failed for {connect_id}: {err}").red(),
              )]));
          }
          return Ok(());
        }
        ProviderConnectMethod::ApiKey => {
          self.open_api_key_entry(connect_id, model_id, effort);
          return Ok(());
        }
        ProviderConnectMethod::None => {}
      }
    }

    self.open_api_key_entry(provider_id, model_id, effort);
    Ok(())
  }

  async fn cache_footer_model_catalog(&mut self, thread_id: &str, model_id: &str) {
    let Some(entry) = self
      .cokra
      .model_client()
      .resolve_model_catalog(model_id)
      .await
    else {
      return;
    };

    self
      .footer_model_catalog
      .insert(thread_id.to_string(), entry);
  }

  fn active_footer_model_catalog_entry(&self) -> Option<&ModelCatalogEntry> {
    self.footer_model_catalog.get(&self.active_thread_id)
  }

  fn active_footer_context_window_limit(&self) -> Option<i64> {
    self
      .footer_context_window_limit
      .get(&self.active_thread_id)
      .and_then(|limit| *limit)
  }

  fn open_api_key_entry(
    &mut self,
    provider_id: String,
    model_id: String,
    effort: Option<cokra_protocol::ReasoningEffortConfig>,
  ) {
    use crate::bottom_pane::api_key_entry_view::ApiKeyEntryView;

    let view = ApiKeyEntryView::new(provider_id, model_id, effort, self.app_event_tx.clone());
    self.chat_widget.bottom_pane.push_view(Box::new(view));
  }

  async fn register_provider_with_api_key(
    &mut self,
    provider_id: &str,
    api_key: String,
  ) -> Result<()> {
    let result = ProviderAuth::connect_api_key(
      self.cokra.model_client().registry(),
      self.cokra.config(),
      provider_id,
      api_key,
    )
    .await?;
    if let Some(err) = result.save_error {
      self
        .chat_widget
        .add_to_history(PlainHistoryCell::new(vec![Line::from(
          format!("● Connected, but failed to save credentials locally: {err}").red(),
        )]));
    }
    Ok(())
  }

  fn open_oauth_connect_view(&mut self, start: OAuthConnectStart) {
    use crate::bottom_pane::oauth_connect_view::OAuthConnectView;

    let prompt = start
      .prompt
      .clone()
      .unwrap_or_else(|| "Paste the authorization response:".to_string());
    let auto_callback_enabled = uses_local_callback(start.pending.kind);
    let view = OAuthConnectView::new(
      start.pending.provider_id.clone(),
      start.provider_name,
      start.auth_url,
      start.instructions,
      prompt,
      auto_callback_enabled,
      self.app_event_tx.clone(),
    );
    self.chat_widget.bottom_pane.push_view(Box::new(view));
  }

  fn cleanup_pending_oauth_flow(&mut self, provider_id: &str) {
    if let Some(flow) = self.pending_oauth_flows.remove(provider_id) {
      flow.cancel.cancel();
    }
  }

  async fn open_connected_provider_models_popup(&mut self, provider_id: &str) -> Result<()> {
    let probe = self
      .cokra
      .model_client()
      .registry()
      .post_connect_probe(provider_id)
      .await;

    if let Some(warning) = probe.warning.clone() {
      self
        .chat_widget
        .add_to_history(PlainHistoryCell::new(vec![Line::from(
          format!("● Post-connect probe: {warning}").dim(),
        )]));
    }

    if !probe.runtime_ready || probe.models.is_empty() {
      self.open_available_models_popup().await?;
      return Ok(());
    }

    let mut provider = cokra_core::model::ProviderInfo::new(
      probe.runtime_provider_id.clone(),
      probe.connect_provider_name,
    )
    .models(probe.models)
    .authenticated(true)
    .visible(true)
    .live(probe.used_live_models);
    provider.options = serde_json::json!({
      "runtime_ready": probe.runtime_ready,
      "post_connect_probe": true,
    });
    let providers = vec![provider];

    self.open_all_models_popup(providers).await?;
    Ok(())
  }

  async fn finalize_connected_provider(
    &mut self,
    provider_id: &str,
    stored: cokra_core::model::auth::StoredCredentials,
  ) -> Result<()> {
    let result = ProviderAuth::persist_and_register(
      self.cokra.model_client().registry(),
      self.cokra.config(),
      provider_id,
      stored,
      None,
    )
    .await?;

    if let Some(err) = result.save_error {
      self
        .chat_widget
        .add_to_history(PlainHistoryCell::new(vec![Line::from(
          format!("● Connected, but failed to save credentials locally: {err}").red(),
        )]));
    }

    let mut runtime_registered = result.runtime_registered;

    if !runtime_registered {
      if let Ok(registered) = ProviderAuth::ensure_runtime_registered(
        self.cokra.model_client().registry(),
        self.cokra.config(),
        provider_id,
      )
      .await
      {
        runtime_registered = registered;
      }
    }

    if let Some(entry) = find_provider_catalog_entry(provider_id) {
      if !runtime_registered {
        self
          .chat_widget
          .add_to_history(PlainHistoryCell::new(vec![Line::from(
            format!(
              "● Connected auth for {}, but no runtime token was produced.",
              entry.name
            )
            .red(),
          )]));
      }

      self
        .chat_widget
        .add_to_history(PlainHistoryCell::new(vec![Line::from(
          format!("● Connected provider: {}", entry.name).dim(),
        )]));
    }

    self.cleanup_pending_oauth_flow(provider_id);
    if runtime_registered {
      self
        .open_connected_provider_models_popup(provider_id)
        .await?;
      return Ok(());
    }

    self.open_available_models_popup().await?;
    Ok(())
  }

  async fn start_oauth_connect(&mut self, provider_id: &str) -> Result<()> {
    let start = ProviderAuth::start_oauth(provider_id).await?;
    let kind = start.pending.kind;
    self.pending_oauth_flows.insert(
      start.pending.provider_id.clone(),
      PendingOAuthFlowState {
        pending: start.pending.clone(),
        cancel: CancellationToken::new(),
      },
    );

    let opens_connect_view = start.prompt.is_some();
    if !opens_connect_view {
      self.chat_widget.add_to_history(PlainHistoryCell::new(vec![
        Line::from(format!(
          "● Starting OAuth connect for {}",
          start.provider_name
        ))
        .dim(),
        Line::from(start.auth_url.clone()).cyan(),
        // Tradeoff: device-code style flows do not own an active dialog, so the
        // transcript still needs the browser URL and instructions as the durable
        // place for the user to refer back to them.
        Line::from(start.instructions.clone()).dim(),
      ]));
    }

    if uses_local_callback(kind) {
      let tx = self.app_event_tx.clone();
      let flow = self.pending_oauth_flows.get(provider_id).cloned();
      tokio::spawn(async move {
        let Some(flow) = flow else {
          return;
        };
        match cokra_core::model::oauth_connect::wait_for_local_callback(
          &flow.pending,
          flow.cancel.clone(),
        )
        .await
        {
          Ok(input) => {
            tx.send(AppEvent::DismissBottomPaneView);
            tx.send(AppEvent::OAuthCodeSubmitted {
              provider_id: flow.pending.provider_id.clone(),
              input,
            });
          }
          Err(err) => {
            if !flow.cancel.is_cancelled() {
              tx.insert_history_cell(PlainHistoryCell::new(vec![Line::from(
                format!("● Automatic localhost callback did not complete: {err}. You can still paste the redirect URL manually.").dim(),
              )]));
            }
          }
        }
      });
    }

    if opens_connect_view {
      self.open_oauth_connect_view(start);
      return Ok(());
    }

    self
      .chat_widget
      .add_to_history(PlainHistoryCell::new(vec![Line::from(
        "Waiting for OAuth approval...".dim(),
      )]));

    let tx = self.app_event_tx.clone();
    let flow = self.pending_oauth_flows.get(provider_id).cloned();
    tokio::spawn(async move {
      let Some(flow) = flow else {
        return;
      };
      match ProviderAuth::complete_oauth(&flow.pending, None).await {
        Ok(stored) => tx.send(AppEvent::OAuthCompleted {
          provider_id: flow.pending.provider_id.clone(),
          stored,
        }),
        Err(err) => tx.send(AppEvent::OAuthFailed {
          provider_id: flow.pending.provider_id.clone(),
          message: err.to_string(),
        }),
      }
    });
    Ok(())
  }

  async fn submit_oauth_code(&mut self, provider_id: &str, input: String) -> Result<()> {
    self.chat_widget.bottom_pane.dismiss_active_view();

    let Some(flow) = self.pending_oauth_flows.remove(provider_id) else {
      self
        .chat_widget
        .add_to_history(PlainHistoryCell::new(vec![Line::from(
          format!("● OAuth session expired for {provider_id}; start Connect again.").dim(),
        )]));
      return Ok(());
    };
    flow.cancel.cancel();

    let stored = match ProviderAuth::complete_oauth(&flow.pending, Some(&input)).await {
      Ok(stored) => stored,
      Err(err) => {
        self
          .chat_widget
          .add_to_history(PlainHistoryCell::new(vec![Line::from(
            format!("● OAuth connect failed for {provider_id}: {err}").red(),
          )]));
        if provider_id == "openai-codex" && err.to_string().contains("unknown_error") {
          self
            .chat_widget
            .add_to_history(PlainHistoryCell::new(vec![Line::from(
              "● OpenAI browser auth returned unknown_error. Recovery: Connect OpenAI with API key from Connect menu.".dim(),
            )]));
        }
        return Ok(());
      }
    };

    self
      .finalize_connected_provider(provider_id, stored)
      .await?;
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
      AppEvent::SelectAgentThread { thread_id } => {
        self.select_agent_thread(tui, thread_id)?;
      }
      AppEvent::OpenAllModelsPopup { providers } => {
        self.open_all_models_popup(providers).await?;
      }
      AppEvent::OpenModelRootPopup => {
        self.open_model_root_popup();
      }
      AppEvent::OpenAvailableModelsPopup => {
        self.open_available_models_popup().await?;
      }
      AppEvent::OpenConnectProvidersPopup => {
        self.open_connect_providers_popup().await?;
      }
      AppEvent::OpenConnectProviderDetail { provider } => {
        self.open_connect_provider_detail(provider);
      }
      AppEvent::StartOAuthConnect { provider_id } => {
        if let Err(err) = self.start_oauth_connect(&provider_id).await {
          self
            .chat_widget
            .add_to_history(PlainHistoryCell::new(vec![Line::from(
              format!("● OAuth connect failed for {provider_id}: {err}").red(),
            )]));
          if let Some(provider) = self
            .cokra
            .model_client()
            .registry()
            .list_connect_catalog()
            .await
            .into_iter()
            .find(|provider| provider.id == provider_id)
          {
            self.open_connect_provider_detail(provider);
          }
        }
      }
      AppEvent::CancelOAuthConnect { provider_id } => {
        if let Some(flow) = self.pending_oauth_flows.remove(&provider_id) {
          flow.cancel.cancel();
        }
        self.open_available_models_popup().await?;
      }
      AppEvent::DismissBottomPaneView => {
        self.chat_widget.bottom_pane.dismiss_active_view();
      }
      AppEvent::DisconnectProvider { provider_id } => {
        if let Err(err) = self.disconnect_provider(&provider_id).await {
          self
            .chat_widget
            .add_to_history(PlainHistoryCell::new(vec![Line::from(
              format!("● Failed to disconnect provider {provider_id}: {err}").red(),
            )]));
        }
      }
      AppEvent::OpenReasoningPopup { model_id } => {
        self.open_reasoning_popup(model_id);
      }
      AppEvent::OpenApiKeyEntry {
        provider_id,
        model_id,
      } => {
        self.open_api_key_entry(provider_id, model_id, None);
      }
      AppEvent::ApplyModelSelection { model_id, effort } => {
        if let Err(err) = self
          .apply_model_selection_or_connect(model_id.clone(), effort)
          .await
        {
          self
            .chat_widget
            .add_to_history(PlainHistoryCell::new(vec![Line::from(
              format!("● Failed to apply model selection {model_id}: {err}").red(),
            )]));
        }
      }
      AppEvent::OpenBackgroundApproval(req) => {
        self.open_background_approval(req);
      }
      AppEvent::OpenBackgroundUserInput(req) => {
        self.open_background_user_input(req);
      }
      AppEvent::ApiKeySubmitted {
        provider_id,
        api_key,
        model_id,
        effort,
      } => {
        match self
          .register_provider_with_api_key(&provider_id, api_key)
          .await
        {
          Ok(()) => {
            if let Err(err) = self
              .apply_model_selection_or_connect(model_id.clone(), effort)
              .await
            {
              self
                .chat_widget
                .add_to_history(PlainHistoryCell::new(vec![Line::from(
                  format!("● Failed to apply model selection {model_id}: {err}").red(),
                )]));
            }
          }
          Err(err) => {
            self
              .chat_widget
              .add_to_history(PlainHistoryCell::new(vec![Line::from(
                format!("● Failed to register API key for {provider_id}: {err}").red(),
              )]));
          }
        }
      }
      AppEvent::OAuthCodeSubmitted { provider_id, input } => {
        if let Err(err) = self.submit_oauth_code(&provider_id, input).await {
          self
            .chat_widget
            .add_to_history(PlainHistoryCell::new(vec![Line::from(
              format!("● OAuth code submission failed for {provider_id}: {err}").red(),
            )]));
        }
      }
      AppEvent::OAuthCompleted {
        provider_id,
        stored,
      } => {
        self.chat_widget.bottom_pane.dismiss_active_view();

        if let Err(err) = self.finalize_connected_provider(&provider_id, stored).await {
          if let Some(flow) = self.pending_oauth_flows.remove(&provider_id) {
            flow.cancel.cancel();
          }
          self
            .chat_widget
            .add_to_history(PlainHistoryCell::new(vec![Line::from(
            format!(
              "● OAuth connect succeeded but provider registration failed for {provider_id}: {err}"
            )
            .red(),
          )]));
        }
      }
      AppEvent::OAuthFailed {
        provider_id,
        message,
      } => {
        if let Some(flow) = self.pending_oauth_flows.remove(&provider_id) {
          flow.cancel.cancel();
        }
        self
          .chat_widget
          .add_to_history(PlainHistoryCell::new(vec![Line::from(
            format!("● OAuth connect failed for {provider_id}: {message}").red(),
          )]));
        if provider_id == "openai-codex" && message.contains("unknown_error") {
          self
            .chat_widget
            .add_to_history(PlainHistoryCell::new(vec![Line::from(
              "● This browser auth flow is unstable. Recovery: Connect OpenAI with API key, or retry this flow later.".dim(),
            )]));
        }
        if let Some(provider) = self
          .cokra
          .model_client()
          .registry()
          .list_connect_catalog()
          .await
          .into_iter()
          .find(|provider| provider.id == provider_id)
        {
          self.open_connect_provider_detail(provider);
        }
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
        self.clear_transcript_view();
        self.insert_startup_welcome(tui)?;
        self
          .chat_widget
          .add_to_history(crate::history_cell::PlainHistoryCell::new(vec![
            Line::from("● New session".dim()),
          ]));
      }
      AppEvent::ForkCurrentSession => {
        self
          .chat_widget
          .add_to_history(crate::history_cell::PlainHistoryCell::new(vec![
            Line::from("● Fork current session is not implemented yet.".dim()),
          ]));
      }
      AppEvent::SetStatusLineMode(mode) => {
        if self.status_line_mode != mode {
          let label = match mode {
            StatusLineMode::Default => "Default",
            StatusLineMode::Minimal => "Minimal",
            StatusLineMode::Off => "Off",
          };
          self
            .chat_widget
            .add_to_history(crate::history_cell::PlainHistoryCell::new(vec![
              Line::from(format!("● Status line: {label}")).dim(),
            ]));
        }
        self.status_line_mode = mode;
        self.refresh_status_line();
        tui.frame_requester().schedule_frame();
      }
    }
    Ok(())
  }

  /// Inline-mode streaming flush: drain any `InsertHistoryCell` events queued in
  /// `app_event_rx` and write them directly into scrollback without waiting for the
  /// next `tokio::select!` round-trip. Called immediately after `handle_cokra_event`
  /// so that delta lines produced by `on_agent_message_delta -> append_boxed_history`
  /// appear on screen in the same scheduling cycle they were emitted.
  fn flush_inline_history_cells(&mut self, tui: &mut Tui) {
    loop {
      match self.app_event_rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => {
          let _ = self.insert_history_cell(cell, tui);
        }
        Ok(other) => {
          // Non-history event encountered. Re-enqueue it so the regular
          // select loop picks it up, then stop draining. Events produced
          // during streaming (CommitTick, StopCommitAnimation) are order-
          // insensitive so a one-cycle delay is harmless.
          self.app_event_tx.send(other);
          break;
        }
        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
      }
    }
  }

  fn insert_history_cell(&mut self, cell: Box<dyn HistoryCell>, tui: &mut Tui) -> Result<()> {
    let width = tui.terminal.last_known_screen_size.width.max(1);
    self.stage_history_cell(cell, width, Some(tui));
    Ok(())
  }

  fn stage_history_cell(&mut self, cell: Box<dyn HistoryCell>, width: u16, tui: Option<&mut Tui>) {
    self.transcript_cells.push(cell);

    let cell = self.transcript_cells.last().unwrap();
    let mut has_emitted = self.has_emitted_history_lines;
    let display = prepare_history_display(cell.as_ref(), width.max(1), &mut has_emitted);
    if display.is_empty() {
      return;
    }
    self.has_emitted_history_lines = has_emitted;

    match self.ui_mode {
      UiMode::Inline => {
        if let Some(tui) = tui {
          if self.deferred_history_lines.is_empty() {
            tui.insert_history_lines(display);
          } else {
            self.deferred_history_lines.extend(display);
          }
        } else {
          self.deferred_history_lines.extend(display);
        }
      }
      UiMode::AltScreen => {
        self.transcript_lines_cache.extend(display);
        self.transcript_cache_width = width.max(1);
      }
    }
  }

  fn rebuild_transcript_cache(&mut self, width: u16) {
    let width = width.max(1);
    self.transcript_lines_cache.clear();

    let mut has_emitted = false;
    for cell in self.transcript_cells.iter() {
      let lines = prepare_history_display(cell.as_ref(), width, &mut has_emitted);
      self.transcript_lines_cache.extend(lines);
    }

    self.transcript_cache_width = width;
  }

  fn refresh_active_tail_cache(&mut self, width: u16) {
    let width = width.max(1);
    let key = self.chat_widget.active_cell_transcript_key();
    if self.active_tail_cache_width == width && self.active_tail_cache_key == key {
      return;
    }

    self.active_tail_cache_width = width;
    self.active_tail_cache_key = key;
    self.active_tail_cache_lines = self
      .chat_widget
      .active_cell_transcript_lines(width)
      .unwrap_or_default();
  }

  fn render_transcript_once(&mut self, tui: &mut Tui) {
    if self.transcript_cells.is_empty() {
      return;
    }

    let width = tui.terminal.last_known_screen_size.width.max(1);
    let mut has_emitted = false;
    let mut lines = Vec::new();
    for cell in &self.transcript_cells {
      lines.extend(prepare_history_display(
        cell.as_ref(),
        width,
        &mut has_emitted,
      ));
    }
    if !lines.is_empty() {
      tui.insert_history_lines(lines);
    }
  }

  async fn handle_tui_event(&mut self, event: TuiEvent, tui: &mut Tui) -> Result<()> {
    match event {
      TuiEvent::Key(key) => {
        self.handle_key_event(tui, key).await?;
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
        if let Ok(size) = tui.terminal.size()
          && size != tui.terminal.last_known_screen_size
        {
          self.refresh_status_line();
        }

        if self.backtrack_render_pending {
          self.backtrack_render_pending = false;
          self.render_transcript_once(tui);
        }

        if !self.deferred_history_lines.is_empty() && self.ui_mode == UiMode::Inline {
          let lines = std::mem::take(&mut self.deferred_history_lines);
          tui.insert_history_lines(lines);
        }

        // 1:1 codex: flush paste burst first; if something flushed, redraw immediately.
        // If still in a burst (first char held for flicker suppression), schedule a
        // follow-up tick so the held char is eventually released even without a keypress.
        if self.chat_widget.bottom_pane.flush_paste_burst_if_due() {
          // Something flushed - schedule an immediate redraw and skip this frame.
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
        // Keep the exploring-cell spinner alive: if there is a live exploring
        // group in the viewport, schedule the next animation frame immediately
        // after each draw so the spinner advances without waiting for a new
        // core event to arrive.
        if self.chat_widget.has_live_exploring_cell() {
          tui
            .frame_requester()
            .schedule_frame_in(std::time::Duration::from_millis(
              crate::exec_cell::SPINNER_INTERVAL_MS as u64,
            ));
        }
      }
    }

    Ok(())
  }

  fn agent_switcher_threads(
    &mut self,
  ) -> Vec<(String, crate::multi_agents::AgentPickerThreadEntry)> {
    self.remember_threads_from_snapshot();
    self.remember_agent_thread(
      self.primary_thread_id.clone(),
      Some("main".to_string()),
      Some("leader".to_string()),
      false,
    );

    let mut threads = self
      .agent_picker_threads
      .iter()
      .map(|(thread_id, entry)| (thread_id.clone(), entry.clone()))
      .collect::<Vec<_>>();
    crate::multi_agents::sort_agent_picker_threads(&mut threads, &self.primary_thread_id);
    threads
  }

  fn set_status_line_agent_selector_active(&mut self, active: bool) {
    if self.status_line_agent_selector_active == active {
      return;
    }
    self.status_line_agent_selector_active = active;
    self.refresh_status_line();
  }

  fn can_focus_status_line_agent_selector(&mut self) -> bool {
    self
      .chat_widget
      .bottom_pane
      .can_focus_status_line_selector()
      && self.agent_switcher_threads().len() > 1
  }

  fn select_adjacent_agent_thread(&mut self, tui: &mut Tui, direction: isize) -> Result<bool> {
    let thread_ids = self
      .agent_switcher_threads()
      .into_iter()
      .map(|(thread_id, _)| thread_id)
      .collect::<Vec<_>>();
    let Some(next_thread_id) = cycle_agent_thread(&thread_ids, &self.active_thread_id, direction)
    else {
      return Ok(false);
    };

    self.status_line_agent_selector_active = true;
    // Tradeoff: footer team switching applies immediately on Left/Right so the
    // inline experience matches Claude Code's fast member cycling instead of
    // forcing a second confirmation key before every replay.
    self.select_agent_thread(tui, next_thread_id)?;
    Ok(true)
  }

  fn handle_status_line_agent_selector_key(
    &mut self,
    tui: &mut Tui,
    key: KeyEvent,
  ) -> Result<bool> {
    if !self.status_line_agent_selector_active && !matches!(key.code, KeyCode::Down) {
      return Ok(false);
    }

    if !self.can_focus_status_line_agent_selector() {
      self.set_status_line_agent_selector_active(false);
      return Ok(false);
    }

    if !self.status_line_agent_selector_active {
      self.set_status_line_agent_selector_active(true);
      return Ok(true);
    }

    match key.code {
      KeyCode::Left => self.select_adjacent_agent_thread(tui, -1),
      KeyCode::Right => self.select_adjacent_agent_thread(tui, 1),
      KeyCode::Esc | KeyCode::Enter => {
        self.set_status_line_agent_selector_active(false);
        Ok(true)
      }
      KeyCode::Down => Ok(true),
      _ => {
        self.set_status_line_agent_selector_active(false);
        Ok(false)
      }
    }
  }

  async fn handle_key_event(&mut self, tui: &mut Tui, key: KeyEvent) -> Result<()> {
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

    if self.handle_status_line_agent_selector_key(tui, key)? {
      return Ok(());
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
        self.submit_user_input(submission).await?;
      }
      BottomPaneAction::Queue(submission) => {
        if self.active_thread_id != self.primary_thread_id {
          self.show_switch_to_main_warning();
        } else {
          self.submit_steer_input(submission).await?;
        }
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

          self.pending_approval_request = None;
          self.maybe_open_next_prompt();
        }
      }
      BottomPaneAction::UserInputDismissed => {
        self.pending_user_input_request = None;
        self.maybe_open_next_prompt();
      }
      BottomPaneAction::SlashCommand(cmd) => {
        self.dispatch_command(cmd).await?;
      }
    }

    Ok(())
  }

  fn show_switch_to_main_warning(&mut self) {
    self
      .chat_widget
      .add_to_history(PlainHistoryCell::new(vec![Line::from(
        "You are currently viewing a member workspace. Switch back to @main before sending team-level instructions.".dim(),
      )]));
  }

  async fn submit_user_input(&mut self, submission: ComposerSubmission) -> Result<()> {
    if self.active_thread_id != self.primary_thread_id {
      self.show_switch_to_main_warning();
      return Ok(());
    }

    if submission.text.trim().is_empty() {
      return Ok(());
    }

    self.scroll_offset = 0;

    let _ = self
      .cokra
      .submit(Op::UserInput {
        items: vec![UserInput::Text {
          text: submission.text,
          text_elements: submission.text_elements,
        }],
        final_output_json_schema: None,
      })
      .await?;

    self.task_running = true;
    self.chat_widget.set_agent_turn_running(true);

    Ok(())
  }

  async fn submit_steer_input(&mut self, submission: ComposerSubmission) -> Result<()> {
    if submission.text.trim().is_empty() {
      return Ok(());
    }

    let Some(expected_turn_id) = self.primary_active_turn_id.clone() else {
      self.queued_submissions.push_back(submission);
      self.sync_bottom_pane_context();
      return Ok(());
    };

    self.scroll_offset = 0;

    let _ = self
      .cokra
      .submit(Op::SteerInput {
        expected_turn_id: Some(expected_turn_id),
        items: vec![UserInput::Text {
          text: submission.text,
          text_elements: submission.text_elements,
        }],
      })
      .await?;

    Ok(())
  }

  async fn flush_buffered_steer_inputs(&mut self) -> Result<()> {
    if self.primary_active_turn_id.is_none() {
      self.sync_bottom_pane_context();
      return Ok(());
    }

    while let Some(submission) = self.queued_submissions.pop_front() {
      self.sync_bottom_pane_context();
      self.submit_steer_input(submission).await?;
    }
    self.sync_bottom_pane_context();
    Ok(())
  }

  fn ensure_thread_store(&mut self, thread_id: &str) -> &mut ThreadEventStore {
    self
      .thread_event_stores
      .entry(thread_id.to_string())
      .or_insert_with(ThreadEventStore::new)
  }

  fn remember_agent_thread(
    &mut self,
    thread_id: String,
    nickname: Option<String>,
    role: Option<String>,
    is_closed: bool,
  ) {
    self.ensure_thread_store(&thread_id);
    let entry = self.agent_picker_threads.entry(thread_id).or_insert(
      crate::multi_agents::AgentPickerThreadEntry {
        nickname: None,
        role: None,
        is_closed,
      },
    );
    if nickname.is_some() {
      entry.nickname = nickname;
    }
    if role.is_some() {
      entry.role = role;
    }
    entry.is_closed = is_closed;
  }

  fn remember_threads_from_snapshot(&mut self) {
    let Some(snapshot) = self.cokra.team_snapshot() else {
      return;
    };

    for member in snapshot.members {
      let is_closed = matches!(member.status, cokra_protocol::AgentStatus::Completed(_));
      self.remember_agent_thread(
        member.thread_id,
        member.nickname,
        Some(member.role),
        is_closed,
      );
    }
  }

  fn note_thread_event_metadata(&mut self, event: &EventMsg) -> bool {
    match event {
      EventMsg::CollabAgentSpawnEnd(ev) => {
        self.remember_agent_thread(
          ev.agent_id.clone(),
          ev.nickname.clone(),
          ev.role.clone(),
          matches!(ev.status, cokra_protocol::AgentStatus::Completed(_)),
        );
        true
      }
      EventMsg::CollabCloseEnd(ev) => {
        self.remember_agent_thread(
          ev.receiver_thread_id.clone(),
          ev.receiver_nickname.clone(),
          ev.receiver_role.clone(),
          true,
        );
        true
      }
      EventMsg::CollabWaitingEnd(ev) => {
        for agent in &ev.agent_statuses {
          self.remember_agent_thread(
            agent.thread_id.clone(),
            agent.nickname.clone(),
            agent.role.clone(),
            matches!(agent.status, cokra_protocol::AgentStatus::Completed(_)),
          );
        }
        true
      }
      EventMsg::CollabTeamSnapshot(_) => {
        self.remember_threads_from_snapshot();
        true
      }
      _ => false,
    }
  }

  fn known_agent_ref(
    &self,
    thread_id: &str,
    nickname: Option<String>,
    role: Option<String>,
  ) -> cokra_protocol::CollabAgentRef {
    let entry = self.agent_picker_threads.get(thread_id);
    let nickname = nickname.or_else(|| entry.and_then(|it| it.nickname.clone()));
    let role = role.or_else(|| entry.and_then(|it| it.role.clone()));
    cokra_protocol::CollabAgentRef {
      thread_id: thread_id.to_string(),
      nickname,
      role,
    }
  }

  fn enrich_collab_event(&self, event: &EventMsg) -> EventMsg {
    match event {
      EventMsg::CollabWaitingBegin(ev) => {
        let mut receivers = ev.receiver_agents.clone();
        for receiver in &mut receivers {
          if let Some(entry) = self.agent_picker_threads.get(&receiver.thread_id) {
            if receiver.nickname.is_none() {
              receiver.nickname = entry.nickname.clone();
            }
            if receiver.role.is_none() {
              receiver.role = entry.role.clone();
            }
          }
        }

        for thread_id in &ev.receiver_thread_ids {
          if receivers
            .iter()
            .any(|receiver| receiver.thread_id == *thread_id)
          {
            continue;
          }
          receivers.push(self.known_agent_ref(thread_id, None, None));
        }

        EventMsg::CollabWaitingBegin(cokra_protocol::CollabWaitingBeginEvent {
          sender_thread_id: ev.sender_thread_id.clone(),
          receiver_thread_ids: ev.receiver_thread_ids.clone(),
          receiver_agents: receivers,
          call_id: ev.call_id.clone(),
        })
      }
      EventMsg::CollabWaitingEnd(ev) => {
        let mut entries = if ev.agent_statuses.is_empty() {
          ev.statuses
            .iter()
            .map(|(thread_id, status)| {
              let entry = self.agent_picker_threads.get(thread_id);
              cokra_protocol::CollabAgentStatusEntry {
                thread_id: thread_id.clone(),
                nickname: entry.and_then(|it| it.nickname.clone()),
                role: entry.and_then(|it| it.role.clone()),
                status: status.clone(),
              }
            })
            .collect::<Vec<_>>()
        } else {
          ev.agent_statuses.clone()
        };

        for entry in &mut entries {
          if let Some(known) = self.agent_picker_threads.get(&entry.thread_id) {
            if entry.nickname.is_none() {
              entry.nickname = known.nickname.clone();
            }
            if entry.role.is_none() {
              entry.role = known.role.clone();
            }
          }
        }

        EventMsg::CollabWaitingEnd(cokra_protocol::CollabWaitingEndEvent {
          sender_thread_id: ev.sender_thread_id.clone(),
          call_id: ev.call_id.clone(),
          agent_statuses: entries,
          statuses: ev.statuses.clone(),
        })
      }
      EventMsg::CollabCloseEnd(ev) => {
        let entry = self.agent_picker_threads.get(&ev.receiver_thread_id);
        EventMsg::CollabCloseEnd(cokra_protocol::CollabCloseEndEvent {
          sender_thread_id: ev.sender_thread_id.clone(),
          call_id: ev.call_id.clone(),
          receiver_thread_id: ev.receiver_thread_id.clone(),
          receiver_nickname: ev
            .receiver_nickname
            .clone()
            .or_else(|| entry.and_then(|it| it.nickname.clone())),
          receiver_role: ev
            .receiver_role
            .clone()
            .or_else(|| entry.and_then(|it| it.role.clone())),
          status: ev.status.clone(),
        })
      }
      EventMsg::CollabMessagePosted(ev) => {
        let sender = self.known_agent_ref(
          &ev.sender_thread_id,
          ev.sender_nickname.clone(),
          ev.sender_role.clone(),
        );
        let recipient = ev.recipient_thread_id.as_deref().map(|thread_id| {
          self.known_agent_ref(
            thread_id,
            ev.recipient_nickname.clone(),
            ev.recipient_role.clone(),
          )
        });

        EventMsg::CollabMessagePosted(cokra_protocol::CollabMessagePostedEvent {
          sender_thread_id: ev.sender_thread_id.clone(),
          sender_nickname: sender.nickname,
          sender_role: sender.role,
          recipient_thread_id: ev.recipient_thread_id.clone(),
          recipient_nickname: recipient.as_ref().and_then(|agent| agent.nickname.clone()),
          recipient_role: recipient.as_ref().and_then(|agent| agent.role.clone()),
          message: ev.message.clone(),
        })
      }
      EventMsg::CollabMessagesRead(ev) => {
        let reader = self.known_agent_ref(
          &ev.reader_thread_id,
          ev.reader_nickname.clone(),
          ev.reader_role.clone(),
        );
        EventMsg::CollabMessagesRead(cokra_protocol::CollabMessagesReadEvent {
          reader_thread_id: ev.reader_thread_id.clone(),
          reader_nickname: reader.nickname,
          reader_role: reader.role,
          count: ev.count,
        })
      }
      _ => event.clone(),
    }
  }

  fn clear_transcript_view(&mut self) {
    self.transcript_cells.clear();
    self.transcript_lines_cache.clear();
    self.active_tail_cache_key = None;
    self.active_tail_cache_width = 0;
    self.active_tail_cache_lines.clear();
    self.deferred_history_lines.clear();
    self.transcript_cache_width = 0;
    self.scroll_offset = 0;
    self.has_emitted_history_lines = false;
    self.backtrack_render_pending = false;
    self.did_show_welcome = false;
  }

  fn drain_replay_history_cells(&mut self, width: u16) {
    loop {
      match self.app_event_rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => {
          self.stage_history_cell(cell, width, None);
        }
        Ok(AppEvent::StartCommitAnimation)
        | Ok(AppEvent::StopCommitAnimation)
        | Ok(AppEvent::CommitTick) => {
          // Tradeoff: replay renders historical output immediately instead of
          // recreating the original typing animation timeline.
        }
        Ok(_) => {
          // Tradeoff: switching threads is a focused replay action. We ignore
          // unrelated queued app events here so historical rebuilds stay pure.
        }
        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
      }
    }
  }

  fn replay_thread_snapshot(&mut self, snapshot: ThreadEventSnapshot, width: u16) {
    if let Some(event) = snapshot.session_configured {
      let enriched = self.enrich_collab_event(&event.msg);
      let _ = self.chat_widget.handle_event(&enriched);
      self.drain_replay_history_cells(width);
    }

    for event in snapshot.events {
      let enriched = self.enrich_collab_event(&event.msg);
      let _ = self.chat_widget.handle_event(&enriched);
      self.drain_replay_history_cells(width);
    }
  }

  fn select_agent_thread(&mut self, tui: &mut Tui, thread_id: String) -> Result<()> {
    if self.active_thread_id == thread_id {
      return Ok(());
    }

    let Some(snapshot) = self
      .thread_event_stores
      .get(&thread_id)
      .map(ThreadEventStore::snapshot)
    else {
      self
        .chat_widget
        .add_to_history(PlainHistoryCell::new(vec![Line::from(
          format!("Cannot switch to thread {thread_id}: no replayable events are available yet.")
            .dim(),
        )]));
      return Ok(());
    };

    self.active_thread_id = thread_id.clone();
    tui.clear_pending_history_lines();
    match self.ui_mode {
      UiMode::Inline => {
        let _ = tui.terminal.clear_scrollback();
        // Tradeoff: switching teammate threads pays for a full visible clear so the
        // new thread view never inherits stale tail rows from the previously shown
        // inline viewport/history region.
        let _ = tui.terminal.clear_visible_screen();
      }
      UiMode::AltScreen => {
        let _ = tui.terminal.clear();
      }
    }
    self.chat_widget = ChatWidget::new(
      self.app_event_tx.clone(),
      tui.frame_requester(),
      true,
      match self.ui_mode {
        UiMode::Inline => StreamRenderMode::ScrollbackFirst,
        UiMode::AltScreen => StreamRenderMode::AnimatedPreview,
      },
    );
    self.chat_widget.bottom_pane.set_status_line_enabled(true);
    self.clear_transcript_view();
    self.insert_startup_welcome(tui)?;
    self.task_running = false;
    self.inline_stable_height = 0;
    self.chat_widget.set_agent_turn_running(false);
    self.replay_thread_snapshot(snapshot, tui.terminal.last_known_screen_size.width.max(1));
    if self.ui_mode == UiMode::Inline && !self.deferred_history_lines.is_empty() {
      let lines = std::mem::take(&mut self.deferred_history_lines);
      tui.insert_history_lines(lines);
    }
    self.restore_pending_prompts();
    self.sync_bottom_pane_context();
    Ok(())
  }

  async fn handle_cokra_event(&mut self, event: Event) -> Result<()> {
    if self.note_thread_event_metadata(&event.msg) {
      // Tradeoff: this recomputes the footer on metadata-only collab events so
      // member nicknames appear in the status line as soon as they are known.
      self.refresh_status_line();
    }

    let owner_thread_id =
      event_thread_id(&event.msg).unwrap_or_else(|| self.primary_thread_id.clone());
    self
      .ensure_thread_store(&owner_thread_id)
      .push_event(event.clone());

    let is_active_thread = owner_thread_id == self.active_thread_id;
    let turn_started = matches!(event.msg, EventMsg::TurnStarted(_));
    let turn_finished = matches!(
      event.msg,
      EventMsg::TurnComplete(_) | EventMsg::TurnAborted(_)
    );

    match &event.msg {
      EventMsg::SessionConfigured(e) => {
        self
          .cache_footer_model_catalog(&e.thread_id, &e.model)
          .await;
        self.footer_context_window_limit.insert(
          e.thread_id.clone(),
          e.context_window_limit.and_then(|limit| i64::try_from(limit).ok()),
        );
      }
      EventMsg::TurnStarted(e) if e.thread_id == self.primary_thread_id => {
        self.primary_active_turn_id = Some(e.turn_id.clone());
      }
      EventMsg::TurnComplete(e) if e.thread_id == self.primary_thread_id => {
        if self.primary_active_turn_id.as_deref() == Some(e.turn_id.as_str()) {
          self.primary_active_turn_id = None;
        }
      }
      EventMsg::TurnAborted(e) if e.thread_id == self.primary_thread_id => {
        if self.primary_active_turn_id.as_deref() == Some(e.turn_id.as_str()) {
          self.primary_active_turn_id = None;
        }
      }
      _ => {}
    }

    if !is_active_thread {
      match &event.msg {
        EventMsg::ExecApprovalRequest(req) => {
          let pending = self
            .background_pending_threads
            .entry(req.thread_id.clone())
            .or_default();
          pending.approval_count += 1;
          self
            .background_approval_requests
            .entry(req.thread_id.clone())
            .or_default()
            .push(req.clone());
          self.enqueue_prompt(PromptRequest::ExecApproval(req.clone()));
        }
        EventMsg::RequestUserInput(req) => {
          let pending = self
            .background_pending_threads
            .entry(req.thread_id.clone())
            .or_default();
          pending.user_input_count += 1;
          self
            .background_user_input_requests
            .entry(req.thread_id.clone())
            .or_default()
            .push(req.clone());
          self.enqueue_prompt(PromptRequest::UserInput(req.clone()));
        }
        _ => {}
      }

      if turn_finished {
        self.background_pending_threads.remove(&owner_thread_id);
        self.background_approval_requests.remove(&owner_thread_id);
        self.background_user_input_requests.remove(&owner_thread_id);
        self.remove_queued_prompts_for_thread(&owner_thread_id);
        if owner_thread_id == self.primary_thread_id {
          self.queued_submissions.clear();
        }
      }

      if turn_started && owner_thread_id == self.primary_thread_id {
        self.flush_buffered_steer_inputs().await?;
      }

      self.sync_bottom_pane_context();
      return Ok(());
    }

    if turn_started {
      self.task_running = true;
      self.chat_widget.set_agent_turn_running(true);
    }

    let enriched_event = self.enrich_collab_event(&event.msg);
    if let Some(action) = self.chat_widget.handle_event(&enriched_event) {
      match action {
        ChatWidgetAction::ShowApproval(req) => {
          self.enqueue_prompt(PromptRequest::ExecApproval(req));
        }
        ChatWidgetAction::ShowRequestUserInput(req) => {
          self.enqueue_prompt(PromptRequest::UserInput(req));
        }
      }
    }

    self.sync_bottom_pane_context();

    if turn_started && owner_thread_id == self.primary_thread_id {
      self.flush_buffered_steer_inputs().await?;
    }

    if turn_finished {
      self.background_pending_threads.remove(&owner_thread_id);
      self.background_approval_requests.remove(&owner_thread_id);
      self.background_user_input_requests.remove(&owner_thread_id);
      self.task_running = false;
      self.chat_widget.set_agent_turn_running(false);
      if owner_thread_id == self.primary_thread_id {
        self.queued_submissions.clear();
      }
      if self
        .pending_approval_request
        .as_ref()
        .is_some_and(|req| req.thread_id == owner_thread_id)
      {
        self.pending_approval = None;
        self.pending_approval_request = None;
      }
      if self
        .pending_user_input_request
        .as_ref()
        .is_some_and(|req| req.thread_id == owner_thread_id)
      {
        self.pending_user_input_request = None;
      }
      self.remove_queued_prompts_for_thread(&owner_thread_id);
      self.maybe_open_next_prompt();
      self.sync_bottom_pane_context();
    }

    Ok(())
  }

  // 1:1 codex: dispatch_command handles all slash commands at the app layer.
  async fn dispatch_command(&mut self, cmd: SlashCommand) -> Result<()> {
    if !cmd.available_during_task() && self.task_running {
      self
        .chat_widget
        .add_to_history(crate::history_cell::PlainHistoryCell::new(vec![
          Line::from(vec![ratatui::text::Span::from(format!(
            "'/{}' is disabled while a task is in progress.",
            cmd.command()
          ))]),
        ]));
      return Ok(());
    }

    match cmd {
      SlashCommand::Model => {
        self.open_model_root_popup();
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
        let _ = self.cokra.submit(Op::Compact).await?;
      }
      SlashCommand::Quit | SlashCommand::Exit => {
        self.exit_info = Some(self.build_exit_info(ExitReason::UserRequested));
      }
      SlashCommand::Status => {
        let usage = self.chat_widget.token_usage();
        let model = self.chat_widget.model_name();
        let cwd = self
          .chat_widget
          .cwd()
          .cloned()
          .unwrap_or_else(|| self.cokra.cwd());
        let lines = vec![
          Line::from(format!("● Model: {model}")),
          Line::from(format!("● Cwd: {}", cwd.display())),
          Line::from(format!(
            "● Tokens: {} input, {} output, {} total",
            usage.input_tokens, usage.output_tokens, usage.total_tokens
          )),
          Line::from(format!("● Task running: {}", self.task_running)),
        ];
        self
          .chat_widget
          .add_to_history(crate::history_cell::PlainHistoryCell::new(lines));
      }
      SlashCommand::DebugConfig => {
        let Some(stack) = self.cokra.config_layer_stack() else {
          self
            .chat_widget
            .add_to_history(crate::history_cell::PlainHistoryCell::new(vec![
              Line::from("● No config layer stack available.".dim()),
            ]));
          return Ok(());
        };

        let mut lines = Vec::new();
        lines.push(Line::from("● Config layers (high -> low):".to_string()));
        for layer in stack.layers_high_to_low() {
          let mut header = match &layer.source {
            cokra_config::ConfigLayerSource::Default => "  - default".to_string(),
            cokra_config::ConfigLayerSource::System { file } => {
              format!("  - system  {}", file.display())
            }
            cokra_config::ConfigLayerSource::User { file } => {
              format!("  - user    {}", file.display())
            }
            cokra_config::ConfigLayerSource::Project { dot_cokra_folder } => {
              format!("  - project {}", dot_cokra_folder.display())
            }
            cokra_config::ConfigLayerSource::SessionFlags => "  - session-flags".to_string(),
          };
          if layer.disabled_reason.is_some() {
            header.push_str("  [disabled]");
          }
          header.push_str(&format!("  v={}", layer.version));
          lines.push(Line::from(header));
          if let Some(reason) = &layer.disabled_reason {
            lines.push(Line::from(format!("    reason: {reason}")).dim());
          }
        }

        let origins = stack.origins();
        lines.push(Line::from(format!("● Origins keys: {}", origins.len())));

        self
          .chat_widget
          .add_to_history(crate::history_cell::PlainHistoryCell::new(lines));
      }
      SlashCommand::Statusline => {
        self.open_statusline_popup();
      }
      SlashCommand::Approvals => {
        self.open_background_approvals_picker();
      }
      SlashCommand::Diff => {
        self
          .chat_widget
          .add_to_history(crate::history_cell::PlainHistoryCell::new(vec![
            Line::from("● /diff is not implemented yet.".dim()),
          ]));
      }
      SlashCommand::Agent => {
        self.open_agent_picker();
      }
      SlashCommand::Collab => {
        self.show_team_dashboard();
      }
      SlashCommand::Clean => {
        self.cleanup_team().await?;
      }
      // All other commands: show not-yet-implemented message.
      _ => {
        self
          .chat_widget
          .add_to_history(crate::history_cell::PlainHistoryCell::new(vec![
            Line::from(format!("● /{} is not implemented yet.", cmd.command()).dim()),
          ]));
      }
    }
    Ok(())
  }

  fn open_model_root_popup(&mut self) {
    use crate::bottom_pane::list_selection_view::SelectionItem;
    use crate::bottom_pane::list_selection_view::SelectionViewParams;
    use crate::bottom_pane::popup_consts::standard_popup_hint_line;

    let items = vec![
      SelectionItem {
        name: "Available Models".to_string(),
        description: Some("Browse usable models from connected providers.".to_string()),
        actions: vec![Box::new(|tx| tx.send(AppEvent::OpenAvailableModelsPopup))],
        dismiss_on_select: true,
        ..Default::default()
      },
      SelectionItem {
        name: "Connect".to_string(),
        description: Some("Connect a provider using OAuth or API key.".to_string()),
        actions: vec![Box::new(|tx| tx.send(AppEvent::OpenConnectProvidersPopup))],
        dismiss_on_select: true,
        ..Default::default()
      },
    ];

    self
      .chat_widget
      .bottom_pane
      .show_selection_view(SelectionViewParams {
        title: Some("Model".to_string()),
        subtitle: Some("Choose whether to browse models or connect a provider.".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        ..Default::default()
      });
  }

  async fn open_available_models_popup(&mut self) -> Result<()> {
    let providers = self
      .cokra
      .model_client()
      .registry()
      .list_connected_models_catalog()
      .await;
    if providers.is_empty() {
      use crate::bottom_pane::list_selection_view::SelectionItem;
      use crate::bottom_pane::list_selection_view::SelectionViewParams;
      use crate::bottom_pane::popup_consts::standard_popup_hint_line;
      self
        .chat_widget
        .bottom_pane
        .show_selection_view(SelectionViewParams {
          title: Some("Available Models".to_string()),
          subtitle: Some("No providers are connected yet. Go to Connect first.".to_string()),
          footer_hint: Some(standard_popup_hint_line()),
          items: vec![SelectionItem {
            name: "Go to Connect".to_string(),
            description: Some("Open provider connection menu.".to_string()),
            actions: vec![Box::new(|tx| tx.send(AppEvent::OpenConnectProvidersPopup))],
            dismiss_on_select: true,
            ..Default::default()
          }],
          ..Default::default()
        });
      return Ok(());
    }
    self.open_all_models_popup(providers).await?;
    Ok(())
  }

  async fn open_connect_providers_popup(&mut self) -> Result<()> {
    use crate::bottom_pane::list_selection_view::SelectionItem;
    use crate::bottom_pane::list_selection_view::SelectionViewParams;
    use crate::bottom_pane::popup_consts::standard_popup_hint_line;

    let mut defs = self
      .cokra
      .model_client()
      .registry()
      .list_connect_catalog()
      .await;

    let rank = |provider_id: &str| -> usize {
      match provider_id {
        "openai" => 0,
        "anthropic" => 1,
        "google" => 2,
        "github-copilot" => 3,
        "openrouter" => 4,
        _ => 100,
      }
    };
    defs.sort_by(|a, b| {
      rank(&a.id)
        .cmp(&rank(&b.id))
        .then_with(|| a.name.cmp(&b.name))
    });

    let mut items = Vec::new();
    for def in defs {
      let method_label = match def.connect_method {
        cokra_core::model::provider::ProviderConnectMethod::ApiKey => "API key",
        cokra_core::model::provider::ProviderConnectMethod::OAuth => "OAuth",
        cokra_core::model::provider::ProviderConnectMethod::None => "Manual",
      };
      let mut desc = if def.authenticated {
        format!("{method_label} | connected")
      } else {
        method_label.to_string()
      };
      if rank(&def.id) < 5 {
        desc = format!("Popular | {desc}");
      }

      let provider_id = def.id.to_string();
      let provider = cokra_core::model::ProviderInfo::new(provider_id.clone(), def.name.clone())
        .connect_method(def.connect_method)
        .connectable(def.connectable)
        .env_vars(def.env_vars.clone())
        .models(def.models.clone())
        .authenticated(def.authenticated)
        .visible(def.visible);

      let actions: Vec<crate::bottom_pane::list_selection_view::SelectionAction> = vec![Box::new(
        move |tx: &crate::app_event_sender::AppEventSender| {
          tx.send(AppEvent::OpenConnectProviderDetail {
            provider: provider.clone(),
          });
        },
      )];

      items.push(SelectionItem {
        name: def.name,
        description: Some(desc),
        is_current: def.authenticated,
        actions,
        dismiss_on_select: true,
        ..Default::default()
      });
    }

    self
      .chat_widget
      .bottom_pane
      .show_selection_view(SelectionViewParams {
        title: Some("Connect".to_string()),
        subtitle: Some("Connect a provider. Popular choices are listed first.".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        is_searchable: true,
        search_placeholder: Some("Filter providers".to_string()),
        ..Default::default()
      });
    Ok(())
  }

  fn open_connect_provider_detail(&mut self, provider: cokra_core::model::ProviderInfo) {
    use crate::bottom_pane::list_selection_view::SelectionAction;
    use crate::bottom_pane::list_selection_view::SelectionItem;
    use crate::bottom_pane::list_selection_view::SelectionViewParams;
    use crate::bottom_pane::popup_consts::standard_popup_hint_line;

    let method_label = match provider.connect_method {
      cokra_core::model::provider::ProviderConnectMethod::ApiKey => "API key",
      cokra_core::model::provider::ProviderConnectMethod::OAuth => "OAuth",
      cokra_core::model::provider::ProviderConnectMethod::None => "Manual",
    };

    let default_model = provider.models.first().cloned().unwrap_or_default();
    let provider_id = provider.id.clone();
    let connect_action: SelectionAction =
      if provider.connect_method == cokra_core::model::provider::ProviderConnectMethod::ApiKey {
        Box::new(move |tx| {
          tx.send(AppEvent::OpenApiKeyEntry {
            provider_id: provider_id.clone(),
            model_id: default_model.clone(),
          });
        })
      } else {
        let provider_id = provider.id.clone();
        Box::new(move |tx| {
          tx.send(AppEvent::StartOAuthConnect {
            provider_id: provider_id.clone(),
          });
        })
      };

    let mut items = vec![SelectionItem {
      name: "Connect".to_string(),
      description: Some(format!("Method: {method_label}")),
      actions: vec![connect_action],
      dismiss_on_select: true,
      ..Default::default()
    }];

    if provider.authenticated {
      let provider_id = provider.id.clone();
      items.push(SelectionItem {
        name: "Disconnect".to_string(),
        description: Some("Remove saved credentials and disconnect provider.".to_string()),
        actions: vec![Box::new(move |tx| {
          tx.send(AppEvent::DisconnectProvider {
            provider_id: provider_id.clone(),
          });
        })],
        dismiss_on_select: true,
        ..Default::default()
      });
    }

    self
      .chat_widget
      .bottom_pane
      .show_selection_view(SelectionViewParams {
        title: Some(provider.name.clone()),
        subtitle: Some(format!(
          "{} - {}",
          if provider.authenticated {
            "Connected"
          } else {
            "Not connected"
          },
          method_label
        )),
        footer_note: provider
          .models
          .first()
          .map(|model| Line::from(format!("Default model: {model}")).dim()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        ..Default::default()
      });
  }

  async fn disconnect_provider(&mut self, provider_id: &str) -> Result<()> {
    use cokra_core::model::auth::AuthManager;

    if let Ok(auth) = AuthManager::new() {
      let _ = auth.remove(provider_id).await;
    }
    if let Some(entry) = find_provider_catalog_entry(provider_id) {
      if let Some(runtime_provider_id) = entry.primary_model_provider_id() {
        let _ = self
          .cokra
          .model_client()
          .registry()
          .remove(&runtime_provider_id)
          .await;
      }
    } else {
      let _ = self
        .cokra
        .model_client()
        .registry()
        .remove(provider_id)
        .await;
    }
    self
      .chat_widget
      .add_to_history(PlainHistoryCell::new(vec![Line::from(
        format!("● Disconnected provider: {provider_id}").dim(),
      )]));
    Ok(())
  }

  fn open_statusline_popup(&mut self) {
    use crate::bottom_pane::list_selection_view::SelectionAction;
    use crate::bottom_pane::list_selection_view::SelectionItem;
    use crate::bottom_pane::list_selection_view::SelectionViewParams;
    use crate::bottom_pane::popup_consts::standard_popup_hint_line;

    let current = self.status_line_mode;
    let mut items: Vec<SelectionItem> = Vec::new();

    let mut push_mode = |name: &str, mode: StatusLineMode, description: &str, is_default: bool| {
      let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(AppEvent::SetStatusLineMode(mode));
      })];
      items.push(SelectionItem {
        name: name.to_string(),
        description: Some(description.to_string()),
        is_current: mode == current,
        is_default,
        actions,
        dismiss_on_select: true,
        ..Default::default()
      });
    };

    push_mode(
      "Default",
      StatusLineMode::Default,
      "model + tokens + cwd",
      true,
    );
    push_mode("Minimal", StatusLineMode::Minimal, "cwd only", false);
    push_mode("Off", StatusLineMode::Off, "hide status line", false);

    let initial_selected_idx = items.iter().position(|i| i.is_current);

    self
      .chat_widget
      .bottom_pane
      .show_selection_view(SelectionViewParams {
        title: Some("Status Line".to_string()),
        subtitle: Some("Choose what appears in the status line.".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        initial_selected_idx,
        ..Default::default()
      });
  }

  async fn open_all_models_popup(
    &mut self,
    providers: Vec<cokra_core::model::ProviderInfo>,
  ) -> Result<()> {
    use crate::bottom_pane::list_selection_view::SelectionAction;
    use crate::bottom_pane::list_selection_view::SelectionItem;
    use crate::bottom_pane::list_selection_view::SelectionViewParams;
    use crate::bottom_pane::popup_consts::standard_popup_hint_line;

    let current_model = self.chat_widget.model_name().to_string();

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
        let is_runtime_ready = provider
          .options
          .get("runtime_ready")
          .and_then(serde_json::Value::as_bool)
          .unwrap_or(true);
        items.push(SelectionItem {
          name: model_id.clone(),
          description: (idx == 0).then_some(provider.name.clone()),
          is_current: model_id == current_model,
          actions,
          dismiss_on_select: true,
          search_value: Some(model_id),
          is_disabled: !is_runtime_ready,
          disabled_reason: (!is_runtime_ready)
            .then_some("Model runtime is not wired yet for this provider.".to_string()),
          ..Default::default()
        });
      }
    }

    self
      .chat_widget
      .bottom_pane
      .show_selection_view(SelectionViewParams {
        title: Some("Select Model and Effort".to_string()),
        subtitle: Some("Type to search models.".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        is_searchable: true,
        search_placeholder: Some("Type to search models".to_string()),
        ..Default::default()
      });
    Ok(())
  }

  fn open_reasoning_popup(&mut self, model_id: String) {
    use crate::bottom_pane::list_selection_view::SelectionAction;
    use crate::bottom_pane::list_selection_view::SelectionItem;
    use crate::bottom_pane::list_selection_view::SelectionViewParams;
    use crate::bottom_pane::popup_consts::standard_popup_hint_line;

    let mut items: Vec<SelectionItem> = Vec::new();
    let push_effort =
      |items: &mut Vec<SelectionItem>,
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

    self
      .chat_widget
      .bottom_pane
      .show_selection_view(SelectionViewParams {
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
    //    The model_id may be "provider/model" - extract provider portion.
    if let Some(provider_id) = model_id.split('/').next() {
      let _ = self
        .cokra
        .model_client()
        .set_default_provider(provider_id)
        .await;
    }

    // 3) Update local model name so the UI reflects the change immediately.
    let thread_id = self.active_thread_id.clone();
    self.cache_footer_model_catalog(&thread_id, &model_id).await;
    self.footer_context_window_limit.insert(thread_id, None);
    self.chat_widget.set_model_name(model_id.clone());
    self.refresh_status_line();

    // 4) Show confirmation in chat history.
    self
      .chat_widget
      .add_to_history(crate::history_cell::PlainHistoryCell::new(vec![
        Line::from(format!("● Model changed to {model_id}")),
      ]));
    Ok(())
  }

  fn sync_bottom_pane_context(&mut self) {
    let used_tokens = self.chat_widget.context_used_tokens();
    let context_percent = self
      .active_footer_context_window_limit()
      .zip(used_tokens)
      .and_then(|(limit, used)| {
        (limit > 0).then_some(
          (100.0 - ((used.max(0) as f64 / limit as f64) * 100.0))
            .clamp(0.0, 100.0)
            .round() as i64,
        )
      });
    self
      .chat_widget
      .bottom_pane
      .set_context_window(context_percent, used_tokens);
    self.chat_widget.bottom_pane.set_steer_enabled(
      self.active_thread_id == self.primary_thread_id
        && (self.task_running || self.primary_active_turn_id.is_some()),
    );
    self.chat_widget.bottom_pane.set_queued_user_messages(
      if self.active_thread_id == self.primary_thread_id {
        self
          .queued_submissions
          .iter()
          .map(|submission| submission.text.clone())
          .collect()
      } else {
        Vec::new()
      },
    );
    self.refresh_status_line();
  }

  fn background_pending_count(&self) -> usize {
    self
      .background_pending_threads
      .values()
      .map(|pending| pending.approval_count + pending.user_input_count)
      .sum()
  }

  fn active_thread_label(&self) -> String {
    if self.active_thread_id == self.primary_thread_id {
      return "@main".to_string();
    }

    self
      .agent_picker_threads
      .get(&self.active_thread_id)
      .and_then(|entry| entry.nickname.as_deref())
      .map(|nickname| format!("@{nickname}"))
      .unwrap_or_else(|| {
        let short = self.active_thread_id.chars().take(8).collect::<String>();
        format!("@{short}")
      })
  }

  fn refresh_status_line(&mut self) {
    let cwd = self
      .chat_widget
      .cwd()
      .cloned()
      .unwrap_or_else(|| self.cokra.cwd());
    let model = self.chat_widget.model_name().to_string();
    let usage = self.chat_widget.token_usage();
    let context_used_tokens = self.chat_widget.context_used_tokens();
    let catalog_entry = self.active_footer_model_catalog_entry();
    let context_window_limit = self.active_footer_context_window_limit();

    let (provider_id, model_name) = if let Some(entry) = catalog_entry {
      (
        Some(entry.provider_id.clone()),
        entry.model_id.clone(),
      )
    } else if let Some((provider_id, model_name)) = model.split_once('/') {
      (Some(provider_id.to_string()), model_name.to_string())
    } else {
      let provider_id = self.cokra.config().models.provider.trim();
      let provider_id = (!provider_id.is_empty()).then(|| provider_id.to_string());
      (provider_id, model.clone())
    };

    self
      .chat_widget
      .bottom_pane
      .set_inline_footer_status(Some(InlineFooterStatus {
        cwd: cwd.display().to_string(),
        git_branch: get_git_branch(&cwd),
        input_tokens: usage.input_tokens.max(0),
        cached_input_tokens: usage.cached_input_tokens.max(0),
        output_tokens: usage.output_tokens.max(0),
        context_window_used_tokens: context_used_tokens.map(|tokens| tokens.max(0)),
        context_window_limit,
        compaction_mode: Some("auto".to_string()),
        provider_id,
        model_name,
        effort_label: self.footer_effort_label.clone(),
      }));

    let enabled = !matches!(self.status_line_mode, StatusLineMode::Off);
    self
      .chat_widget
      .bottom_pane
      .set_status_line_enabled(enabled);
    if !enabled {
      self.chat_widget.bottom_pane.set_status_line(None);
      return;
    }
    let team_snapshot = self.cokra.team_snapshot();
    let agent_switcher_threads = self.agent_switcher_threads();
    let has_team_switcher = agent_switcher_threads.len() > 1;
    if !has_team_switcher {
      self.status_line_agent_selector_active = false;
    }

    let push_agent_switcher = |spans: &mut Vec<Span<'static>>| {
      if has_team_switcher {
        spans.push(ratatui::text::Span::from("team: ").dim());
        spans.extend(status_line_agent_tabs(
          &agent_switcher_threads,
          &self.primary_thread_id,
          &self.active_thread_id,
          self.status_line_agent_selector_active,
        ));
        spans.push(ratatui::text::Span::from("  |  ").dim());
        let hint = if self.status_line_agent_selector_active {
          "Left/Right switch  Esc done"
        } else {
          "Left/Right team switch"
        };
        spans.push(ratatui::text::Span::from(hint).dim());
      } else {
        spans.push(ratatui::text::Span::from("view: ").dim());
        spans.push(ratatui::text::Span::from(self.active_thread_label()));
      }
    };

    let line = match self.status_line_mode {
      StatusLineMode::Minimal => {
        let mut spans = Vec::new();
        push_agent_switcher(&mut spans);
        spans.push(ratatui::text::Span::from("  |  ").dim());
        spans.push(ratatui::text::Span::from("cwd: ").dim());
        spans.push(ratatui::text::Span::from(cwd.display().to_string()));
        Line::from(spans)
      }
      StatusLineMode::Default => {
        let mut spans = Vec::new();
        push_agent_switcher(&mut spans);
        spans.push(ratatui::text::Span::from("  |  ").dim());
        if !model.is_empty() {
          spans.push(ratatui::text::Span::from("model: ").dim());
          spans.push(ratatui::text::Span::from(model));
          spans.push(ratatui::text::Span::from("  |  ").dim());
        }
        spans.push(ratatui::text::Span::from("tokens: ").dim());
        spans.push(ratatui::text::Span::from(format!(
          "{} in, {} out, {} total",
          usage.input_tokens, usage.output_tokens, usage.total_tokens
        )));
        spans.push(ratatui::text::Span::from("  |  ").dim());
        if let Some(snapshot) = &team_snapshot {
          let unread_total: usize = snapshot.unread_counts.values().copied().sum();
          let pending_plans = snapshot
            .plans
            .iter()
            .filter(|plan| matches!(plan.status, cokra_protocol::TeamPlanStatus::PendingApproval))
            .count();
          spans.push(ratatui::text::Span::from("alerts: ").dim());
          spans.push(ratatui::text::Span::from(format!(
            "{} unread, {} bg approvals, {} pending plans",
            unread_total,
            self.background_pending_count(),
            pending_plans
          )));
          spans.push(ratatui::text::Span::from("  |  ").dim());
        }
        spans.push(ratatui::text::Span::from("cwd: ").dim());
        spans.push(ratatui::text::Span::from(cwd.display().to_string()));
        Line::from(spans)
      }
      StatusLineMode::Off => Line::from(""),
    };

    self.chat_widget.bottom_pane.set_status_line(Some(line));
  }

  fn render(&mut self, frame: &mut Frame) {
    let area = frame.area();
    self.history_width = area.width;

    match self.ui_mode {
      UiMode::Inline => {
        // 1:1 codex: inline viewport only renders the live chat surface.
        // Committed history is written into normal scrollback via
        // Tui::insert_history_lines and must not be re-rendered here, or
        // resize/turn redraws will duplicate separators and footer borders.
        self.chat_widget.render(area, frame.buffer_mut());
        if let Some((x, y)) = self.chat_widget.cursor_pos(area) {
          frame.set_cursor_position((x, y));
        }
      }
      UiMode::AltScreen => {
        if self.transcript_cache_width != area.width {
          self.rebuild_transcript_cache(area.width);
        }
        self.refresh_active_tail_cache(area.width);
        let bottom_height = self
          .chat_widget
          .bottom_pane
          .desired_height(area.width)
          .min(area.height);
        let chunks =
          Layout::vertical([Constraint::Min(1), Constraint::Length(bottom_height)]).split(area);
        self.chat_widget.render_alt_screen(
          chunks[0],
          frame.buffer_mut(),
          &self.transcript_lines_cache,
          &self.active_tail_cache_lines,
          self.scroll_offset,
        );
        self
          .chat_widget
          .bottom_pane
          .render(chunks[1], frame.buffer_mut(), self.task_running);
        if let Some((x, y)) = self.chat_widget.bottom_pane.cursor_pos(chunks[1]) {
          frame.set_cursor_position((x, y));
        }
      }
    }
  }

  fn show_team_dashboard(&mut self) {
    let Some(root_thread_id) = self.cokra.thread_id().map(ToString::to_string) else {
      self
        .chat_widget
        .add_to_history(PlainHistoryCell::new(vec![Line::from(
          "● No active team runtime".dim(),
        )]));
      return;
    };
    let Some(snapshot) = self.cokra.team_snapshot() else {
      self
        .chat_widget
        .add_to_history(PlainHistoryCell::new(vec![Line::from(
          "● No active team runtime".dim(),
        )]));
      return;
    };
    let cell = crate::multi_agents::team_snapshot(cokra_protocol::CollabTeamSnapshotEvent {
      actor_thread_id: root_thread_id,
      snapshot,
    });
    self.chat_widget.add_to_history(cell);
  }

  fn open_agent_picker(&mut self) {
    use crate::bottom_pane::list_selection_view::SelectionAction;
    use crate::bottom_pane::list_selection_view::SelectionItem;
    use crate::bottom_pane::list_selection_view::SelectionViewParams;
    use crate::bottom_pane::popup_consts::standard_popup_hint_line;

    let threads = self.agent_switcher_threads();
    if threads.len() <= 1 {
      self
        .chat_widget
        .add_to_history(PlainHistoryCell::new(vec![Line::from(
          "No other team member threads are available to switch to yet.".dim(),
        )]));
      return;
    }

    let snapshot = self.cokra.team_snapshot();
    let mut initial_selected_idx = None;
    let items = threads
      .into_iter()
      .enumerate()
      .map(|(idx, (thread_id, entry))| {
        if thread_id == self.active_thread_id {
          initial_selected_idx = Some(idx);
        }
        let is_primary = thread_id == self.primary_thread_id;
        let unread = snapshot
          .as_ref()
          .and_then(|state| state.unread_counts.get(&thread_id))
          .copied()
          .unwrap_or(0);
        let pending = self
          .background_pending_threads
          .get(&thread_id)
          .map(|item| item.approval_count + item.user_input_count)
          .unwrap_or(0);
        let status = snapshot.as_ref().and_then(|state| {
          state
            .members
            .iter()
            .find(|member| member.thread_id == thread_id)
            .map(|member| format!("{:?}", member.status))
        });
        let task = snapshot.as_ref().and_then(|state| {
          state
            .members
            .iter()
            .find(|member| member.thread_id == thread_id)
            .map(|member| member.task.clone())
        });
        let mut description_parts = Vec::new();
        if let Some(status) = status {
          description_parts.push(status);
        }
        if unread > 0 {
          description_parts.push(format!("{unread} unread"));
        }
        if pending > 0 {
          description_parts.push(format!("{pending} pending"));
        }
        let description = if description_parts.is_empty() {
          Some(thread_id.clone())
        } else {
          Some(format!("{} | {}", thread_id, description_parts.join(" | ")))
        };
        let selected_description = task.map(|task| format!("Task: {task}"));
        let thread_id_for_action = thread_id.clone();
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
          tx.send(AppEvent::SelectAgentThread {
            thread_id: thread_id_for_action.clone(),
          });
        })];
        SelectionItem {
          name: crate::multi_agents::format_agent_picker_item_name(
            entry.nickname.as_deref(),
            entry.role.as_deref(),
            is_primary,
          ),
          description,
          selected_description,
          is_current: thread_id == self.active_thread_id,
          actions,
          dismiss_on_select: true,
          search_value: Some(format!(
            "{} {}",
            thread_id,
            entry.nickname.clone().unwrap_or_default()
          )),
          ..Default::default()
        }
      })
      .collect();

    self
      .chat_widget
      .bottom_pane
      .show_selection_view(SelectionViewParams {
        title: Some("Agent Teams".to_string()),
        subtitle: Some(
          "Choose a member workspace to inspect; @main remains the coordinator.".to_string(),
        ),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        initial_selected_idx,
        is_searchable: true,
        search_placeholder: Some("Filter members".to_string()),
        ..Default::default()
      });
  }

  async fn cleanup_team(&mut self) -> Result<()> {
    let _ = self.cokra.cleanup_team_runtime().await;
    self.background_pending_threads.clear();
    self.background_approval_requests.clear();
    self.background_user_input_requests.clear();
    self.prompt_queue.clear();
    self.pending_approval = None;
    self.pending_approval_request = None;
    self.pending_user_input_request = None;
    self.show_team_dashboard();
    self.sync_bottom_pane_context();
    Ok(())
  }

  fn has_active_prompt(&self) -> bool {
    self.pending_approval.is_some() || self.pending_user_input_request.is_some()
  }

  fn enqueue_prompt(&mut self, prompt: PromptRequest) {
    self.prompt_queue.push_back(prompt);
    self.maybe_open_next_prompt();
  }

  fn maybe_open_next_prompt(&mut self) {
    if self.has_active_prompt() {
      return;
    }

    let Some(prompt) = self.prompt_queue.pop_front() else {
      return;
    };

    match prompt {
      PromptRequest::ExecApproval(req) => self.open_background_approval(req),
      PromptRequest::UserInput(req) => self.open_background_user_input(req),
    }
  }

  fn restore_pending_prompts(&mut self) {
    if let Some(req) = self.pending_approval_request.clone() {
      if self.pending_approval.is_some() {
        self
          .chat_widget
          .bottom_pane
          .push_approval_request(ApprovalRequest {
            call_id: req.id.clone(),
            tool_name: req.tool_name.clone(),
            command: self.format_exec_approval_command(&req),
          });
        return;
      }

      self.pending_approval_request = None;
    }

    if let Some(req) = self.pending_user_input_request.clone() {
      self.chat_widget.bottom_pane.push_user_input_request(req);
      return;
    }

    self.maybe_open_next_prompt();
  }

  fn thread_label_for(&self, thread_id: &str) -> String {
    if thread_id == self.primary_thread_id {
      return "@main".to_string();
    }

    self
      .agent_picker_threads
      .get(thread_id)
      .and_then(|entry| entry.nickname.as_deref())
      .map(|nickname| format!("@{nickname}"))
      .unwrap_or_else(|| {
        let short = thread_id.chars().take(8).collect::<String>();
        format!("@{short}")
      })
  }

  fn format_exec_approval_command(&self, req: &ExecApprovalRequestEvent) -> String {
    let thread_label = self.thread_label_for(&req.thread_id);
    format!(
      "{thread_label} | {} (cwd: {})",
      req.command,
      req.cwd.display()
    )
  }

  fn remove_queued_exec_approval(&mut self, id: &str) {
    self.prompt_queue.retain(|prompt| match prompt {
      PromptRequest::ExecApproval(req) => req.id != id,
      PromptRequest::UserInput(_) => true,
    });
  }

  fn remove_queued_user_input(&mut self, turn_id: &str) {
    self.prompt_queue.retain(|prompt| match prompt {
      PromptRequest::ExecApproval(_) => true,
      PromptRequest::UserInput(req) => req.turn_id != turn_id,
    });
  }

  fn remove_queued_prompts_for_thread(&mut self, thread_id: &str) {
    self.prompt_queue.retain(|prompt| match prompt {
      PromptRequest::ExecApproval(req) => req.thread_id != thread_id,
      PromptRequest::UserInput(req) => req.thread_id != thread_id,
    });
  }

  fn open_background_approval(&mut self, req: ExecApprovalRequestEvent) {
    self.remove_queued_exec_approval(&req.id);

    if self.has_active_prompt() {
      self
        .prompt_queue
        .push_front(PromptRequest::ExecApproval(req));
      return;
    }

    decrement_background_approval(&mut self.background_pending_threads, &req.thread_id);
    if let Some(requests) = self.background_approval_requests.get_mut(&req.thread_id) {
      requests.retain(|item| item.id != req.id);
    }
    self.pending_approval = Some(PendingApproval {
      id: req.id.clone(),
      turn_id: Some(req.turn_id.clone()),
    });
    self.pending_approval_request = Some(req.clone());
    let command = self.format_exec_approval_command(&req);
    self
      .chat_widget
      .bottom_pane
      .push_approval_request(ApprovalRequest {
        call_id: req.id,
        tool_name: req.tool_name,
        command,
      });
    self.sync_bottom_pane_context();
  }

  fn open_background_user_input(&mut self, req: RequestUserInputEvent) {
    self.remove_queued_user_input(&req.turn_id);

    if self.has_active_prompt() {
      self.prompt_queue.push_front(PromptRequest::UserInput(req));
      return;
    }

    decrement_background_user_input(&mut self.background_pending_threads, &req.thread_id);
    if let Some(requests) = self.background_user_input_requests.get_mut(&req.thread_id) {
      requests.retain(|item| item.turn_id != req.turn_id);
    }
    self.pending_user_input_request = Some(req.clone());
    self.chat_widget.bottom_pane.push_user_input_request(req);
    self.sync_bottom_pane_context();
  }

  fn open_background_approvals_picker(&mut self) {
    use crate::bottom_pane::list_selection_view::SelectionAction;
    use crate::bottom_pane::list_selection_view::SelectionItem;
    use crate::bottom_pane::list_selection_view::SelectionViewParams;
    use crate::bottom_pane::popup_consts::standard_popup_hint_line;

    let mut items = Vec::new();
    for requests in self.background_approval_requests.values() {
      for req in requests {
        let req_clone = req.clone();
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
          tx.send(AppEvent::OpenBackgroundApproval(req_clone.clone()));
        })];
        items.push(SelectionItem {
          name: format!("approval {}", req.id),
          description: Some(format!("thread={} | tool={}", req.thread_id, req.tool_name)),
          actions,
          dismiss_on_select: true,
          ..Default::default()
        });
      }
    }
    for requests in self.background_user_input_requests.values() {
      for req in requests {
        let req_clone = req.clone();
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
          tx.send(AppEvent::OpenBackgroundUserInput(req_clone.clone()));
        })];
        items.push(SelectionItem {
          name: format!("user input {}", req.turn_id),
          description: Some(format!(
            "thread={} | questions={}",
            req.thread_id,
            req.questions.len()
          )),
          actions,
          dismiss_on_select: true,
          ..Default::default()
        });
      }
    }

    if items.is_empty() {
      self
        .chat_widget
        .add_to_history(PlainHistoryCell::new(vec![Line::from(
          "● No background approvals pending".dim(),
        )]));
      return;
    }

    self
      .chat_widget
      .bottom_pane
      .show_selection_view(SelectionViewParams {
        title: Some("Background Approvals".to_string()),
        subtitle: Some(
          "Review pending approvals and input requests from background teammates.".to_string(),
        ),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        is_searchable: true,
        search_placeholder: Some("Filter pending items".to_string()),
        ..Default::default()
      });
  }
}

fn event_thread_id(event: &EventMsg) -> Option<String> {
  match event {
    EventMsg::Error(e) => Some(e.thread_id.clone()),
    EventMsg::Warning(e) => Some(e.thread_id.clone()),
    EventMsg::TokenCount(e) => Some(e.thread_id.clone()),
    EventMsg::AgentMessage(e) => Some(e.thread_id.clone()),
    EventMsg::AgentMessageDelta(e) => Some(e.thread_id.clone()),
    EventMsg::AgentMessageContentDelta(e) => Some(e.thread_id.clone()),
    EventMsg::UserMessage(e) => Some(e.thread_id.clone()),
    EventMsg::SessionConfigured(e) => Some(e.thread_id.clone()),
    EventMsg::ThreadNameUpdated(e) => Some(e.thread_id.clone()),
    EventMsg::ExecCommandBegin(e) => Some(e.thread_id.clone()),
    EventMsg::ExecCommandOutputDelta(e) => Some(e.thread_id.clone()),
    EventMsg::ExecCommandEnd(e) => Some(e.thread_id.clone()),
    EventMsg::ExecApprovalRequest(e) => Some(e.thread_id.clone()),
    EventMsg::RequestUserInput(e) => Some(e.thread_id.clone()),
    EventMsg::StreamError(e) => Some(e.thread_id.clone()),
    EventMsg::TurnComplete(e) => Some(e.thread_id.clone()),
    EventMsg::TurnAborted(e) => Some(e.thread_id.clone()),
    EventMsg::TurnStarted(e) => Some(e.thread_id.clone()),
    EventMsg::ItemStarted(e) => Some(e.thread_id.clone()),
    EventMsg::ItemCompleted(e) => Some(e.thread_id.clone()),
    EventMsg::PlanDelta(e) => Some(e.thread_id.clone()),
    EventMsg::ReasoningContentDelta(e) => Some(e.thread_id.clone()),
    EventMsg::ReasoningRawContentDelta(e) => Some(e.thread_id.clone()),
    EventMsg::CollabAgentSpawnBegin(e) => Some(e.thread_id.clone()),
    EventMsg::CollabAgentSpawnEnd(e) => Some(e.thread_id.clone()),
    EventMsg::CollabAgentInteractionBegin(e) => Some(e.thread_id.clone()),
    EventMsg::CollabAgentInteractionEnd(e) => Some(e.thread_id.clone()),
    EventMsg::CollabMessagePosted(e) => Some(e.sender_thread_id.clone()),
    EventMsg::CollabMessagesRead(e) => Some(e.reader_thread_id.clone()),
    EventMsg::CollabTaskUpdated(e) => Some(e.actor_thread_id.clone()),
    EventMsg::CollabTeamSnapshot(e) => Some(e.actor_thread_id.clone()),
    EventMsg::CollabPlanSubmitted(e) => Some(e.actor_thread_id.clone()),
    EventMsg::CollabPlanDecision(e) => Some(e.actor_thread_id.clone()),
    EventMsg::CollabWaitingBegin(e) => Some(e.sender_thread_id.clone()),
    EventMsg::CollabWaitingEnd(e) => Some(e.sender_thread_id.clone()),
    EventMsg::CollabCloseBegin(e) => Some(e.sender_thread_id.clone()),
    EventMsg::CollabCloseEnd(e) => Some(e.sender_thread_id.clone()),
    EventMsg::CollabResumeBegin(e) => Some(e.sender_thread_id.clone()),
    EventMsg::CollabResumeEnd(e) => Some(e.sender_thread_id.clone()),
    _ => None,
  }
}

fn decrement_background_approval(
  pending_threads: &mut HashMap<String, BackgroundPending>,
  thread_id: &str,
) {
  if let Some(pending) = pending_threads.get_mut(thread_id) {
    pending.approval_count = pending.approval_count.saturating_sub(1);
    if pending.approval_count == 0 && pending.user_input_count == 0 {
      pending_threads.remove(thread_id);
    }
  }
}

fn decrement_background_user_input(
  pending_threads: &mut HashMap<String, BackgroundPending>,
  thread_id: &str,
) {
  if let Some(pending) = pending_threads.get_mut(thread_id) {
    pending.user_input_count = pending.user_input_count.saturating_sub(1);
    if pending.approval_count == 0 && pending.user_input_count == 0 {
      pending_threads.remove(thread_id);
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

fn prepare_history_display(
  cell: &dyn HistoryCell,
  width: u16,
  has_emitted_history_lines: &mut bool,
) -> Vec<Line<'static>> {
  let mut display = cell.display_lines(width.max(1));
  if display.is_empty() {
    return display;
  }

  if !cell.is_stream_continuation() {
    if *has_emitted_history_lines {
      display.insert(0, Line::from(""));
    } else {
      *has_emitted_history_lines = true;
    }
  }

  display
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::history_cell::PlainHistoryCell;

  fn lines_to_string(lines: &[Line<'static>]) -> String {
    lines
      .iter()
      .map(|line| {
        line
          .spans
          .iter()
          .map(|span| span.content.as_ref())
          .collect::<String>()
      })
      .collect::<Vec<_>>()
      .join("\n")
  }

  #[test]
  fn prepare_history_display_inserts_separator_after_first_non_stream_cell() {
    let first = PlainHistoryCell::new(vec![Line::from("first")]);
    let second = PlainHistoryCell::new(vec![Line::from("second")]);
    let mut emitted = false;

    let first_lines = prepare_history_display(&first, 80, &mut emitted);
    let second_lines = prepare_history_display(&second, 80, &mut emitted);

    assert_eq!(lines_to_string(&first_lines), "first");
    assert_eq!(lines_to_string(&second_lines), "\nsecond");
  }

  #[test]
  fn thread_event_store_keeps_full_history_without_truncation() {
    let mut store = ThreadEventStore::new();
    for idx in 0..600 {
      store.push_event(Event {
        id: format!("event-{idx}"),
        msg: EventMsg::AgentMessage(cokra_protocol::AgentMessageEvent {
          thread_id: "agent-1".to_string(),
          turn_id: "turn-1".to_string(),
          item_id: format!("item-{idx}"),
          content: vec![cokra_protocol::AgentMessageContent::Text {
            text: format!("message-{idx}"),
          }],
        }),
      });
    }

    let snapshot = store.snapshot();
    assert_eq!(snapshot.events.len(), 600);

    let first = snapshot.events.first().expect("first event");
    let last = snapshot.events.last().expect("last event");
    assert_eq!(first.id, "event-0");
    assert_eq!(last.id, "event-599");
  }

  #[test]
  fn cycle_agent_thread_wraps_in_both_directions() {
    let thread_ids = vec!["main".to_string(), "elon".to_string(), "dario".to_string()];

    assert_eq!(
      cycle_agent_thread(&thread_ids, "main", -1),
      Some("dario".to_string())
    );
    assert_eq!(
      cycle_agent_thread(&thread_ids, "dario", 1),
      Some("main".to_string())
    );
    assert_eq!(
      cycle_agent_thread(&thread_ids, "elon", 1),
      Some("dario".to_string())
    );
  }

  #[test]
  fn status_line_agent_tabs_mark_active_member_and_selector_focus() {
    let threads = vec![
      (
        "main".to_string(),
        crate::multi_agents::AgentPickerThreadEntry {
          nickname: Some("main".to_string()),
          role: Some("leader".to_string()),
          is_closed: false,
        },
      ),
      (
        "elon-thread".to_string(),
        crate::multi_agents::AgentPickerThreadEntry {
          nickname: Some("elon".to_string()),
          role: Some("worker".to_string()),
          is_closed: false,
        },
      ),
    ];

    let spans = status_line_agent_tabs(&threads, "main", "elon-thread", true);
    let rendered = spans
      .iter()
      .map(|span| span.content.as_ref())
      .collect::<String>();
    assert_eq!(rendered, "@main [@elon (worker)]");
    assert!(spans.iter().any(|span| {
      span.content.as_ref() == "[@elon (worker)]"
        && span
          .style
          .add_modifier
          .contains(ratatui::style::Modifier::UNDERLINED)
    }));
  }
}
