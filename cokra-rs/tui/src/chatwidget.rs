use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use cokra_protocol::AgentMessageContent;
use cokra_protocol::EventMsg;
use cokra_protocol::ExecApprovalRequestEvent;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::BottomPane;
use crate::exec_cell::ExecCall;
use crate::exec_cell::ExecCell;
use crate::exec_cell::model::CommandOutput;
use crate::exec_cell::new_active_exec_command;
use crate::history_cell::AgentMessageCell;
use crate::history_cell::ApprovalRequestedHistoryCell;
use crate::history_cell::ExecHistoryCell;
use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;
use crate::history_cell::SessionConfiguredCell;
use crate::history_cell::TurnCompleteHistoryCell;
use crate::history_cell::UserHistoryCell;
use crate::multi_agents;
use crate::render::Insets;
use crate::render::renderable::FlexRenderable;
use crate::render::renderable::Renderable;
use crate::render::renderable::RenderableExt as _;
use crate::render::renderable::RenderableItem;
use crate::streaming::chunking::AdaptiveChunkingPolicy;
use crate::streaming::commit_tick::CommitTickScope;
use crate::streaming::commit_tick::run_commit_tick;
use crate::streaming::controller::PlanStreamController;
use crate::streaming::controller::StreamController;
use crate::tui::FrameRequester;

#[derive(Debug)]
pub(crate) enum ChatWidgetAction {
  ShowApproval(ExecApprovalRequestEvent),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TokenUsage {
  pub input_tokens: i64,
  pub output_tokens: i64,
  pub total_tokens: i64,
}

impl TokenUsage {
  pub fn is_zero(&self) -> bool {
    self.input_tokens == 0 && self.output_tokens == 0 && self.total_tokens == 0
  }
}

pub(crate) struct ChatWidget {
  active_cell: Option<Box<dyn HistoryCell>>,
  active_cell_revision: u64,
  stream_controller: Option<StreamController>,
  plan_stream_controller: Option<PlanStreamController>,
  chunking_policy: AdaptiveChunkingPolicy,
  pub(crate) bottom_pane: BottomPane,
  token_usage: TokenUsage,
  pub(crate) model_name: String,
  pub(crate) cwd: Option<std::path::PathBuf>,
  agent_turn_running: bool,
  has_seen_session_configured: bool,
  pending_exec_calls: HashMap<String, ExecCall>,
  streamed_agent_item_ids: HashSet<String>,
  animations_enabled: bool,
  app_event_tx: AppEventSender,
}

impl ChatWidget {
  pub(crate) fn new(
    app_event_tx: AppEventSender,
    frame_requester: FrameRequester,
    animations_enabled: bool,
  ) -> Self {
    Self {
      active_cell: None,
      active_cell_revision: 0,
      stream_controller: None,
      plan_stream_controller: None,
      chunking_policy: AdaptiveChunkingPolicy::default(),
      bottom_pane: BottomPane::new(app_event_tx.clone(), frame_requester, animations_enabled),
      token_usage: TokenUsage::default(),
      model_name: String::new(),
      cwd: None,
      agent_turn_running: false,
      has_seen_session_configured: false,
      pending_exec_calls: HashMap::new(),
      streamed_agent_item_ids: HashSet::new(),
      animations_enabled,
      app_event_tx,
    }
  }

  pub(crate) fn token_usage(&self) -> TokenUsage {
    self.token_usage
  }

  pub(crate) fn cwd(&self) -> Option<&std::path::PathBuf> {
    self.cwd.as_ref()
  }

  fn flush_active_cell(&mut self) {
    if let Some(cell) = self.active_cell.take() {
      self.app_event_tx.insert_boxed_history_cell(cell);
    }
  }

  pub(crate) fn add_to_history(&mut self, cell: impl HistoryCell + 'static) {
    self.add_boxed_history(Box::new(cell));
  }

  fn add_boxed_history(&mut self, cell: Box<dyn HistoryCell>) {
    self.flush_active_cell();
    self.app_event_tx.insert_boxed_history_cell(cell);
  }

  fn flush_stream_controllers(&mut self) {
    if let Some(mut controller) = self.stream_controller.take()
      && let Some(cell) = controller.finalize()
    {
      self.add_boxed_history(cell);
    }
    if let Some(mut controller) = self.plan_stream_controller.take()
      && let Some(cell) = controller.finalize()
    {
      self.add_boxed_history(cell);
    }
  }

  fn bump_active_cell_revision(&mut self) {
    self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
  }

  pub(crate) fn set_agent_turn_running(&mut self, running: bool) {
    self.agent_turn_running = running;
    // 1:1 codex: BottomPane.set_task_running creates/destroys StatusIndicator internally.
    self.bottom_pane.set_task_running(running);
  }

  pub(crate) fn push_user_input_text(&mut self, text: String) {
    self.add_to_history(UserHistoryCell::from_text(text));
  }

  pub(crate) fn open_resume_picker(&mut self) {
    self.add_to_history(PlainHistoryCell::new(vec![Line::from(
      "• /resume is not yet implemented.".dim(),
    )]));
  }

  pub(crate) fn handle_event(&mut self, event: &EventMsg) -> Option<ChatWidgetAction> {
    match event {
      EventMsg::UserMessage(e) => {
        let mut text_parts: Vec<String> = Vec::new();
        let mut text_elements = Vec::new();
        let mut remote_image_urls = Vec::new();
        let mut byte_offset = 0usize;

        for item in &e.items {
          match item {
            cokra_protocol::UserInput::Text {
              text,
              text_elements: elems,
            } => {
              if !text_parts.is_empty() {
                byte_offset += 1; // for the '\n' join separator
              }
              // Remap element byte ranges relative to the joined string.
              for elem in elems {
                text_elements.push(cokra_protocol::TextElement {
                  byte_range: cokra_protocol::ByteRange {
                    start: elem.byte_range.start + byte_offset,
                    end: elem.byte_range.end + byte_offset,
                  },
                  placeholder: elem.placeholder.clone(),
                });
              }
              byte_offset += text.len();
              text_parts.push(text.clone());
            }
            cokra_protocol::UserInput::Image { image_url } => {
              remote_image_urls.push(image_url.clone());
            }
            cokra_protocol::UserInput::LocalImage { path } => {
              text_parts.push(format!("[local_image] {}", path.display()));
              byte_offset = text_parts.iter().map(|s| s.len()).sum::<usize>()
                + text_parts.len().saturating_sub(1);
            }
            cokra_protocol::UserInput::Skill { name, .. } => {
              text_parts.push(format!("[skill] {name}"));
              byte_offset = text_parts.iter().map(|s| s.len()).sum::<usize>()
                + text_parts.len().saturating_sub(1);
            }
            cokra_protocol::UserInput::Mention { name, path } => {
              text_parts.push(format!("[@{name}] {path}"));
              byte_offset = text_parts.iter().map(|s| s.len()).sum::<usize>()
                + text_parts.len().saturating_sub(1);
            }
          }
        }

        let text = text_parts.join("\n");
        self.add_to_history(UserHistoryCell::new(text, text_elements, remote_image_urls));
      }
      EventMsg::TurnStarted(e) => {
        self.set_agent_turn_running(true);
        self.cwd = Some(e.cwd.clone());
      }
      EventMsg::AgentMessageDelta(e) | EventMsg::AgentMessageContentDelta(e) => {
        // 1:1 codex: track item_id so we can hard-dedup any later AgentMessage
        // carrying the same item_id (prevents double-rendering regardless of
        // stream_controller lifetime).
        self.streamed_agent_item_ids.insert(e.item_id.clone());
        let is_new = self.stream_controller.is_none();
        let controller = self
          .stream_controller
          .get_or_insert_with(|| StreamController::new(None));
        let _ = controller.push(&e.delta);
        if is_new {
          self.app_event_tx.send(AppEvent::StartCommitAnimation);
        }
      }
      EventMsg::AgentMessage(e) => {
        // 1:1 codex: hard dedup by item_id. If streaming deltas already
        // committed content for this item, unconditionally drop the final
        // AgentMessage regardless of stream_controller lifetime/timing.
        if !self.streamed_agent_item_ids.contains(&e.item_id) {
          let mut lines = Vec::new();
          for part in &e.content {
            match part {
              AgentMessageContent::Text { text } => lines.push(Line::from(text.clone())),
            }
          }
          if !lines.is_empty() {
            self.add_to_history(AgentMessageCell::new(lines, true));
          }
        }
      }
      EventMsg::TokenCount(e) => {
        self.token_usage.input_tokens = e.input_tokens;
        self.token_usage.output_tokens = e.output_tokens;
        self.token_usage.total_tokens = e.total_tokens;
      }
      EventMsg::SessionConfigured(e) => {
        let is_first = !self.has_seen_session_configured;
        self.has_seen_session_configured = true;
        self.model_name = e.model.clone();
        self.add_to_history(SessionConfiguredCell {
          model: e.model.clone(),
          approval_policy: e.approval_policy.clone(),
          sandbox_mode: e.sandbox_mode.clone(),
          cwd: None,
          is_first_session: is_first,
        });
      }
      EventMsg::ThreadNameUpdated(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Thread renamed: {}",
          e.name
        ))]));
      }
      EventMsg::ExecCommandBegin(e) => {
        self.flush_stream_controllers();
        if let Some(status) = self.bottom_pane.status_widget_mut() {
          status.update_header("Running command".to_string());
          status.update_details(Some(e.command.clone()));
        }

        let call = ExecCall {
          command_id: e.command_id.clone(),
          command: e.command.clone(),
          cwd: e.cwd.clone(),
          output: None,
          start_time: Some(Instant::now()),
          duration: None,
        };

        let reuse_exec_cell = self
          .active_cell
          .as_ref()
          .and_then(|cell| cell.as_any().downcast_ref::<ExecCell>())
          .is_some();

        if reuse_exec_cell {
          if let Some(cell) = self
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
          {
            cell.push_call(call.clone());
          }
        } else {
          self.flush_active_cell();
          self.active_cell = Some(Box::new(new_active_exec_command(
            call.command_id.clone(),
            call.command.clone(),
            call.cwd.clone(),
            self.animations_enabled,
          )));
        }

        self
          .pending_exec_calls
          .insert(call.command_id.clone(), call);
        self.bump_active_cell_revision();
      }
      EventMsg::ExecCommandOutputDelta(e) => {
        if let Some(call) = self.pending_exec_calls.get_mut(&e.command_id) {
          let output = call.output.get_or_insert_with(CommandOutput::default);
          output.output.push_str(&e.output);
        }

        if let Some(cell) = self
          .active_cell
          .as_mut()
          .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
          && cell.append_output(&e.command_id, &e.output)
        {
          self.bump_active_cell_revision();
        }
      }
      EventMsg::ExecCommandEnd(e) => {
        let mut call = self
          .pending_exec_calls
          .remove(&e.command_id)
          .unwrap_or(ExecCall {
            command_id: e.command_id.clone(),
            command: "<unknown>".to_string(),
            cwd: PathBuf::from("."),
            output: None,
            start_time: None,
            duration: None,
          });

        let mut output = call.output.unwrap_or_default();
        if !e.output.is_empty() {
          output.output.push_str(&e.output);
        }
        output.exit_code = e.exit_code;

        let duration = call
          .start_time
          .map(|st| st.elapsed())
          .unwrap_or_else(|| Duration::from_millis(0));
        call.start_time = None;
        call.duration = Some(duration);
        call.output = Some(output.clone());

        let mut updated_active_exec = false;
        let mut should_flush_active = false;
        if let Some(cell) = self
          .active_cell
          .as_mut()
          .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
        {
          cell.complete_call(&e.command_id, output, duration);
          updated_active_exec = true;
          should_flush_active = !cell.is_active();
        } else {
          self.add_to_history(ExecHistoryCell::from_exec_call(
            call,
            self.animations_enabled,
          ));
        }
        if updated_active_exec {
          self.bump_active_cell_revision();
        }
        if should_flush_active {
          self.flush_active_cell();
        }
      }
      EventMsg::ExecApprovalRequest(e) => {
        self.add_to_history(ApprovalRequestedHistoryCell {
          command: e.command.clone(),
        });
        return Some(ChatWidgetAction::ShowApproval(e.clone()));
      }
      EventMsg::RequestUserInput(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Request user input: {}",
          e.prompt
        ))]));
      }
      EventMsg::Warning(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("warning: ").yellow(),
          Span::from(e.message.clone()),
        ])]));
      }
      EventMsg::StreamError(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("stream error: ").red(),
          Span::from(e.error.clone()),
        ])]));
      }
      EventMsg::Error(e) => {
        self.app_event_tx.send(AppEvent::StopCommitAnimation);
        self.stream_controller = None;
        self.plan_stream_controller = None;
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("error: ").red(),
          Span::from(e.user_facing_message.clone()),
        ])]));
        self.set_agent_turn_running(false);
        self.streamed_agent_item_ids.clear();
      }
      EventMsg::TurnComplete(_) => {
        self.flush_stream_controllers();
        self.flush_active_cell();

        self.app_event_tx.send(AppEvent::StopCommitAnimation);
        self.add_to_history(TurnCompleteHistoryCell {
          input_tokens: self.token_usage.input_tokens,
          output_tokens: self.token_usage.output_tokens,
        });
        self.set_agent_turn_running(false);
        // 1:1 codex: clear dedup set so next turn starts fresh.
        self.streamed_agent_item_ids.clear();
      }
      EventMsg::TurnAborted(e) => {
        self.app_event_tx.send(AppEvent::StopCommitAnimation);
        self.stream_controller = None;
        self.plan_stream_controller = None;
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("aborted: ").yellow(),
          Span::from(e.reason.clone()),
        ])]));
        self.set_agent_turn_running(false);
        self.streamed_agent_item_ids.clear();
      }
      EventMsg::CollabAgentSpawnBegin(e) => {
        self.add_to_history(multi_agents::spawn_begin(e.clone()));
      }
      EventMsg::CollabAgentSpawnEnd(e) => {
        self.add_to_history(multi_agents::spawn_end(e.clone()));
      }
      EventMsg::CollabAgentInteractionBegin(e) => {
        self.add_to_history(multi_agents::interaction_begin(e.clone()));
      }
      EventMsg::CollabAgentInteractionEnd(e) => {
        self.add_to_history(multi_agents::interaction_end(e.clone()));
      }
      EventMsg::ItemStarted(_) => {
        // 1:1 codex: no-op. Item lifecycle is handled by delta streaming
        // and ItemCompleted; rendering ItemStarted would add noise.
      }
      EventMsg::ItemCompleted(_) => {
        // 1:1 codex: ItemCompleted carries the full assistant text in `result`,
        // but streaming deltas have ALREADY committed that content progressively.
        // Rendering `result` here would duplicate the entire response.
        // In codex, ItemCompleted dispatches to typed handlers for status
        // indicator management only — it never re-renders assistant text.
      }
      EventMsg::ShutdownComplete => {}

      // ---------- Model & Context (display-only or no-op) ----------
      EventMsg::ModelReroute(_) => {
        // No-op: model reroute is informational only
      }
      EventMsg::ContextCompacted(_) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(
          "• Context compacted".dim(),
        )]));
      }
      EventMsg::ThreadRolledBack(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Thread rolled back {n} turn{s}",
          n = e.num_turns,
          s = if e.num_turns == 1 { "" } else { "s" },
        ))]));
      }

      // ---------- Agent Reasoning ----------
      EventMsg::AgentReasoningDelta(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("reasoning: ").dim(),
          Span::from(e.delta.clone()).dim(),
        ])]));
      }
      EventMsg::AgentReasoning(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("reasoning: ").dim(),
          Span::from(e.text.clone()).dim(),
        ])]));
      }
      EventMsg::AgentReasoningRawContent(_)
      | EventMsg::AgentReasoningRawContentDelta(_)
      | EventMsg::AgentReasoningSectionBreak(_) => {
        // No-op: raw reasoning content not displayed in inline TUI
      }

      // ---------- MCP Events ----------
      EventMsg::McpStartupUpdate(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• MCP server {}: {:?}",
          e.server, e.status
        ))]));
      }
      EventMsg::McpStartupComplete(e) => {
        if !e.failed.is_empty() {
          for f in &e.failed {
            self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
              Span::from("MCP failed: ").red(),
              Span::from(format!("{}: {}", f.server, f.error)),
            ])]));
          }
        }
      }
      EventMsg::McpToolCallBegin(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Calling {}.{}",
          e.invocation.server, e.invocation.tool
        ))]));
      }
      EventMsg::McpToolCallEnd(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• {}.{} completed ({}ms)",
          e.invocation.server, e.invocation.tool, e.duration_ms
        ))]));
      }

      // ---------- Web Search Events ----------
      EventMsg::WebSearchBegin(_) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(
          "• Searching the web...".dim(),
        )]));
      }
      EventMsg::WebSearchEnd(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Searched: {}",
          e.query
        ))]));
      }

      // ---------- Terminal Interaction ----------
      EventMsg::TerminalInteraction(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Terminal input: {}",
          e.stdin.trim()
        ))]));
      }

      // ---------- Image ----------
      EventMsg::ViewImageToolCall(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Viewed image: {}",
          e.path.display()
        ))]));
      }

      // ---------- Additional Approval Events ----------
      EventMsg::DynamicToolCallRequest(_) => {
        // No-op: dynamic tool calls not yet rendered
      }
      EventMsg::ElicitationRequest(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• {} requests input: {}",
          e.server_name, e.message
        ))]));
      }
      EventMsg::ApplyPatchApprovalRequest(e) => {
        let file_count = e.changes.len();
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Patch approval requested ({file_count} file{s})",
          s = if file_count == 1 { "" } else { "s" },
        ))]));
        // TODO: show full approval overlay for patches
      }

      // ---------- Notices & Background ----------
      EventMsg::DeprecationNotice(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("deprecated: ").red(),
          Span::from(e.summary.clone()),
        ])]));
      }
      EventMsg::BackgroundEvent(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(
          e.message.clone().dim(),
        )]));
      }

      // ---------- Undo ----------
      EventMsg::UndoStarted(e) => {
        let msg = e.message.as_deref().unwrap_or("Undoing...");
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!("• {msg}"))]));
      }
      EventMsg::UndoCompleted(e) => {
        let msg = e.message.as_deref().unwrap_or(if e.success {
          "Undo completed"
        } else {
          "Undo failed"
        });
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!("• {msg}"))]));
      }

      // ---------- Patch Events ----------
      EventMsg::PatchApplyBegin(e) => {
        let file_count = e.changes.len();
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Applying patch ({file_count} file{s}){auto}",
          s = if file_count == 1 { "" } else { "s" },
          auto = if e.auto_approved {
            " [auto-approved]"
          } else {
            ""
          },
        ))]));
      }
      EventMsg::PatchApplyEnd(e) => {
        let status_str = if e.success { "succeeded" } else { "failed" };
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Patch apply {status_str}"
        ))]));
      }
      EventMsg::TurnDiff(_) => {
        // TODO: render diff in overlay
      }

      // ---------- Query/Response Events ----------
      EventMsg::GetHistoryEntryResponse(_) => {}
      EventMsg::McpListToolsResponse(_) => {}
      EventMsg::ListCustomPromptsResponse(_) => {}
      EventMsg::ListSkillsResponse(_) => {}
      EventMsg::ListRemoteSkillsResponse(_) => {}
      EventMsg::RemoteSkillDownloaded(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Skill downloaded: {}",
          e.name
        ))]));
      }
      EventMsg::SkillsUpdateAvailable => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(
          "• Skills update available".cyan(),
        )]));
      }

      // ---------- Plan ----------
      EventMsg::PlanUpdate(plan) => {
        let mut lines = Vec::new();
        if let Some(expl) = &plan.explanation {
          lines.push(Line::from(expl.clone().dim()));
        }
        for item in &plan.plan {
          let marker = match item.status {
            cokra_protocol::StepStatus::Completed => "✓",
            cokra_protocol::StepStatus::InProgress => "►",
            cokra_protocol::StepStatus::Pending => "○",
          };
          lines.push(Line::from(format!("  {marker} {}", item.step)));
        }
        self.add_to_history(PlainHistoryCell::new(lines));
      }

      // ---------- Review Mode ----------
      EventMsg::EnteredReviewMode(e) => {
        let hint = e
          .user_facing_hint
          .as_deref()
          .unwrap_or("Entered review mode");
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(
          hint.to_string().cyan(),
        )]));
      }
      EventMsg::ExitedReviewMode(_) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(
          "• Exited review mode".dim(),
        )]));
      }

      // ---------- Raw / Item-Based (no-op or minimal display) ----------
      EventMsg::RawResponseItem(_) => {}
      EventMsg::PlanDelta(e) => {
        let is_new = self.plan_stream_controller.is_none();
        let controller = self
          .plan_stream_controller
          .get_or_insert_with(|| PlanStreamController::new(None));
        let _ = controller.push(&e.delta);
        if is_new {
          self.app_event_tx.send(AppEvent::StartCommitAnimation);
        }
      }
      EventMsg::ReasoningContentDelta(_) | EventMsg::ReasoningRawContentDelta(_) => {
        // No-op: item-based reasoning deltas not displayed separately
      }

      // ---------- Additional Collaboration Events ----------
      EventMsg::CollabWaitingBegin(_) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(
          "• Waiting for agents...".dim(),
        )]));
      }
      EventMsg::CollabWaitingEnd(_) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(
          "• Agents completed".dim(),
        )]));
      }
      EventMsg::CollabCloseBegin(_) => {}
      EventMsg::CollabCloseEnd(_) => {}
      EventMsg::CollabResumeBegin(_) => {}
      EventMsg::CollabResumeEnd(_) => {}
    }

    None
  }

  pub(crate) fn on_commit_tick(&mut self) {
    let output = run_commit_tick(
      &mut self.chunking_policy,
      self.stream_controller.as_mut(),
      self.plan_stream_controller.as_mut(),
      CommitTickScope::AnyMode,
      Instant::now(),
    );

    for cell in output.cells {
      self.add_boxed_history(cell);
    }

    if output.all_idle && !self.agent_turn_running {
      if let Some(status) = self.bottom_pane.status_widget_mut() {
        status.pause_timer();
      }
    }
  }

  // 1:1 codex: compose active_cell (flex=1) + bottom_pane (flex=0) using FlexRenderable.
  fn as_renderable(&self) -> RenderableItem<'_> {
    let active_cell_renderable = match &self.active_cell {
      Some(cell) => {
        RenderableItem::Borrowed(cell as &dyn Renderable).inset(Insets::tlbr(1, 0, 0, 0))
      }
      None => RenderableItem::Owned(Box::new(())),
    };
    let mut flex = FlexRenderable::new();
    flex.push(1, active_cell_renderable);
    flex.push(
      0,
      RenderableItem::Borrowed(&self.bottom_pane as &dyn Renderable)
        .inset(Insets::tlbr(1, 0, 0, 0)),
    );
    RenderableItem::Owned(Box::new(flex))
  }

  /// Render the scrollable history + active cell for AltScreen mode.
  /// Status indicator is now rendered by BottomPane, not here.
  pub(crate) fn render_alt_screen(
    &self,
    area: Rect,
    buf: &mut Buffer,
    alt_history_lines: &[Line<'static>],
    scroll_offset: u16,
  ) {
    if area.height == 0 || area.width == 0 {
      return;
    }

    let mut lines = alt_history_lines.to_vec();

    if let Some(active) = &self.active_cell {
      let active_lines = active.display_lines(area.width);
      if !active_lines.is_empty() {
        if !lines.is_empty() {
          lines.push(Line::from(""));
        }
        lines.extend(active_lines);
      }
    }

    let overflow = lines.len().saturating_sub(usize::from(area.height));
    let scroll_y = if scroll_offset == 0 {
      u16::try_from(overflow).unwrap_or(u16::MAX)
    } else {
      let bounded = overflow.saturating_sub(scroll_offset as usize);
      u16::try_from(bounded).unwrap_or(u16::MAX)
    };

    Paragraph::new(Text::from(lines))
      .scroll((scroll_y, 0))
      .render(area, buf);
  }
}

impl Renderable for ChatWidget {
  fn render(&self, area: Rect, buf: &mut Buffer) {
    self.as_renderable().render(area, buf);
  }

  fn desired_height(&self, width: u16) -> u16 {
    self.as_renderable().desired_height(width)
  }

  fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
    self.as_renderable().cursor_pos(area)
  }
}
