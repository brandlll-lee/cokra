use std::collections::HashMap;
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

use crate::app_event_sender::AppEventSender;
use crate::exec_cell::ExecCall;
use crate::exec_cell::model::CommandOutput;
use crate::history_cell::ApprovalRequestedHistoryCell;
use crate::history_cell::ExecHistoryCell;
use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;
use crate::history_cell::SessionConfiguredCell;
use crate::history_cell::TurnCompleteHistoryCell;
use crate::history_cell::UserHistoryCell;
use crate::multi_agents;
use crate::render::renderable::Renderable;
use crate::status_indicator_widget::StatusIndicatorWidget;
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
}

impl TokenUsage {
  pub fn is_zero(&self) -> bool {
    self.input_tokens == 0 && self.output_tokens == 0
  }
}

pub(crate) struct ChatWidget {
  history: Vec<Box<dyn HistoryCell>>,
  stream_controller: Option<StreamController>,
  plan_stream_controller: Option<PlanStreamController>,
  chunking_policy: AdaptiveChunkingPolicy,
  status_indicator: StatusIndicatorWidget,
  token_usage: TokenUsage,
  model_name: String,
  agent_turn_running: bool,
  pending_exec_calls: HashMap<String, ExecCall>,
  animations_enabled: bool,
}

impl ChatWidget {
  pub(crate) fn new(
    app_event_tx: AppEventSender,
    frame_requester: FrameRequester,
    animations_enabled: bool,
  ) -> Self {
    Self {
      history: Vec::new(),
      stream_controller: None,
      plan_stream_controller: None,
      chunking_policy: AdaptiveChunkingPolicy::default(),
      status_indicator: StatusIndicatorWidget::new(
        app_event_tx,
        frame_requester,
        animations_enabled,
      ),
      token_usage: TokenUsage::default(),
      model_name: String::new(),
      agent_turn_running: false,
      pending_exec_calls: HashMap::new(),
      animations_enabled,
    }
  }

  pub(crate) fn token_usage(&self) -> TokenUsage {
    self.token_usage
  }

  pub(crate) fn set_agent_turn_running(&mut self, running: bool) {
    self.agent_turn_running = running;
    if running {
      self.status_indicator.resume_timer();
      self.status_indicator.update_header("Working".to_string());
    } else {
      self.status_indicator.pause_timer();
      self.status_indicator.update_details(None);
      self.status_indicator.update_inline_message(None);
    }
  }

  pub(crate) fn push_user_input_text(&mut self, text: String) {
    self
      .history
      .push(Box::new(UserHistoryCell::from_text(text)));
  }

  pub(crate) fn handle_event(&mut self, event: &EventMsg) -> Option<ChatWidgetAction> {
    match event {
      EventMsg::UserMessage(e) => {
        let text = e
          .items
          .iter()
          .map(|item| match item {
            cokra_protocol::UserInput::Text { text, .. } => text.clone(),
            cokra_protocol::UserInput::Image { image_url } => format!("[image] {image_url}"),
            cokra_protocol::UserInput::LocalImage { path } => {
              format!("[local_image] {}", path.display())
            }
            cokra_protocol::UserInput::Skill { name, .. } => format!("[skill] {name}"),
            cokra_protocol::UserInput::Mention { name, path } => format!("[@{name}] {path}"),
          })
          .collect::<Vec<_>>()
          .join("\n");
        self
          .history
          .push(Box::new(UserHistoryCell::from_text(text)));
      }
      EventMsg::TurnStarted(_) => {
        self.set_agent_turn_running(true);
      }
      EventMsg::AgentMessageDelta(e) | EventMsg::AgentMessageContentDelta(e) => {
        let controller = self
          .stream_controller
          .get_or_insert_with(|| StreamController::new(None));
        let _ = controller.push(&e.delta);
      }
      EventMsg::AgentMessage(e) => {
        let mut lines = Vec::new();
        for part in &e.content {
          match part {
            AgentMessageContent::Text { text } => lines.push(Line::from(text.clone())),
          }
        }
        if !lines.is_empty() {
          self
            .history
            .push(Box::new(crate::history_cell::AgentMessageCell::new(
              lines, true,
            )));
        }
      }
      EventMsg::TokenCount(e) => {
        self.token_usage.input_tokens = e.input_tokens;
        self.token_usage.output_tokens = e.output_tokens;
      }
      EventMsg::SessionConfigured(e) => {
        self.model_name = e.model.clone();
        self.history.push(Box::new(SessionConfiguredCell {
          model: e.model.clone(),
          approval_policy: e.approval_policy.clone(),
          sandbox_mode: e.sandbox_mode.clone(),
        }));
      }
      EventMsg::ThreadNameUpdated(e) => {
        self
          .history
          .push(Box::new(PlainHistoryCell::new(vec![Line::from(format!(
            "• Thread renamed: {}",
            e.name
          ))])));
      }
      EventMsg::ExecCommandBegin(e) => {
        self
          .status_indicator
          .update_header("Running command".to_string());
        self
          .status_indicator
          .update_details(Some(e.command.clone()));
        self.pending_exec_calls.insert(
          e.command_id.clone(),
          ExecCall {
            command_id: e.command_id.clone(),
            command: e.command.clone(),
            cwd: e.cwd.clone(),
            output: None,
            start_time: Some(Instant::now()),
            duration: None,
          },
        );
      }
      EventMsg::ExecCommandOutputDelta(e) => {
        if let Some(call) = self.pending_exec_calls.get_mut(&e.command_id) {
          let output = call.output.get_or_insert_with(CommandOutput::default);
          output.output.push_str(&e.output);
        }
      }
      EventMsg::ExecCommandEnd(e) => {
        let mut call = self
          .pending_exec_calls
          .remove(&e.command_id)
          .unwrap_or(ExecCall {
            command_id: e.command_id.clone(),
            command: String::from("<unknown>"),
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
        call.output = Some(output);

        self.history.push(Box::new(ExecHistoryCell::from_exec_call(
          call,
          self.animations_enabled,
        )));
      }
      EventMsg::ExecApprovalRequest(e) => {
        self.history.push(Box::new(ApprovalRequestedHistoryCell {
          command: e.command.clone(),
        }));
        return Some(ChatWidgetAction::ShowApproval(e.clone()));
      }
      EventMsg::RequestUserInput(e) => {
        self
          .history
          .push(Box::new(PlainHistoryCell::new(vec![Line::from(format!(
            "• Request user input: {}",
            e.prompt
          ))])));
      }
      EventMsg::Warning(e) => {
        self
          .history
          .push(Box::new(PlainHistoryCell::new(vec![Line::from(vec![
            Span::from("warning: ").yellow(),
            Span::from(e.message.clone()),
          ])])));
      }
      EventMsg::StreamError(e) => {
        self
          .history
          .push(Box::new(PlainHistoryCell::new(vec![Line::from(vec![
            Span::from("stream error: ").red(),
            Span::from(e.error.clone()),
          ])])));
      }
      EventMsg::Error(e) => {
        self
          .history
          .push(Box::new(PlainHistoryCell::new(vec![Line::from(vec![
            Span::from("error: ").red(),
            Span::from(e.user_facing_message.clone()),
          ])])));
        self.set_agent_turn_running(false);
      }
      EventMsg::TurnComplete(_) => {
        if let Some(controller) = self.stream_controller.as_mut()
          && let Some(cell) = controller.finalize()
        {
          self.history.push(cell);
        }
        if let Some(controller) = self.plan_stream_controller.as_mut()
          && let Some(cell) = controller.finalize()
        {
          self.history.push(cell);
        }
        self.history.push(Box::new(TurnCompleteHistoryCell {
          input_tokens: self.token_usage.input_tokens,
          output_tokens: self.token_usage.output_tokens,
        }));
        self.set_agent_turn_running(false);
      }
      EventMsg::TurnAborted(e) => {
        self
          .history
          .push(Box::new(PlainHistoryCell::new(vec![Line::from(vec![
            Span::from("aborted: ").yellow(),
            Span::from(e.reason.clone()),
          ])])));
        self.set_agent_turn_running(false);
      }
      EventMsg::CollabAgentSpawnBegin(e) => {
        self
          .history
          .push(Box::new(multi_agents::spawn_begin(e.clone())));
      }
      EventMsg::CollabAgentSpawnEnd(e) => {
        self
          .history
          .push(Box::new(multi_agents::spawn_end(e.clone())));
      }
      EventMsg::CollabAgentInteractionBegin(e) => {
        self
          .history
          .push(Box::new(multi_agents::interaction_begin(e.clone())));
      }
      EventMsg::CollabAgentInteractionEnd(e) => {
        self
          .history
          .push(Box::new(multi_agents::interaction_end(e.clone())));
      }
      EventMsg::ItemStarted(e) => {
        self
          .history
          .push(Box::new(PlainHistoryCell::new(vec![Line::from(format!(
            "• {} started ({})",
            e.item_type, e.item_id
          ))])));
      }
      EventMsg::ItemCompleted(e) => {
        if !e.result.trim().is_empty() {
          self
            .history
            .push(Box::new(PlainHistoryCell::new(vec![Line::from(
              e.result.clone(),
            )])));
        }
      }
      EventMsg::ShutdownComplete => {}
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

    if !output.cells.is_empty() {
      self.history.extend(output.cells);
    }

    if output.all_idle && !self.agent_turn_running {
      self.status_indicator.pause_timer();
    }
  }

  pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
      return;
    }

    let status_height = if self.agent_turn_running {
      self
        .status_indicator
        .desired_height(area.width)
        .min(area.height)
    } else {
      0
    };

    let history_height = area.height.saturating_sub(status_height);
    let history_area = Rect {
      x: area.x,
      y: area.y,
      width: area.width,
      height: history_height,
    };

    let status_area = Rect {
      x: area.x,
      y: area.y + history_height,
      width: area.width,
      height: status_height,
    };

    let mut lines = Vec::new();
    for (idx, cell) in self.history.iter().enumerate() {
      lines.extend(cell.display_lines(history_area.width));
      if idx + 1 < self.history.len() {
        lines.push(Line::from(""));
      }
    }

    let overflow = lines.len().saturating_sub(usize::from(history_area.height));
    Paragraph::new(Text::from(lines))
      .scroll((u16::try_from(overflow).unwrap_or(u16::MAX), 0))
      .render(history_area, buf);

    if status_height > 0 {
      self.status_indicator.render(status_area, buf);
    }
  }
}
