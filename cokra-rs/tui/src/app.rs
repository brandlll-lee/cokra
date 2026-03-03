use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use tokio::sync::broadcast;
use tokio::sync::mpsc;

use cokra_core::Cokra;
use cokra_protocol::Event;
use cokra_protocol::EventMsg;
use cokra_protocol::Op;
use cokra_protocol::ReviewDecision;
use cokra_protocol::UserInput;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::BottomPane;
use crate::bottom_pane::BottomPaneAction;
use crate::bottom_pane::approval_overlay::ApprovalOverlay;
use crate::bottom_pane::approval_overlay::ApprovalRequest;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::ChatWidgetAction;
use crate::chatwidget::TokenUsage;
use crate::tui::FrameRequester;
use crate::tui::Terminal;
use crate::tui::TuiEvent;
use crate::tui::TuiEventStream;

pub struct App {
  cokra: Cokra,
  chat_widget: ChatWidget,
  bottom_pane: BottomPane,
  exit_info: Option<AppExitInfo>,
  app_event_rx: mpsc::UnboundedReceiver<AppEvent>,
  task_running: bool,
  pending_approval: Option<PendingApproval>,
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
  pub fn new(cokra: Cokra, frame_requester: FrameRequester) -> Self {
    let (app_event_tx, app_event_rx) = mpsc::unbounded_channel();
    let app_event_sender = AppEventSender::new(app_event_tx);

    Self {
      cokra,
      chat_widget: ChatWidget::new(app_event_sender, frame_requester, true),
      bottom_pane: BottomPane::new(),
      exit_info: None,
      app_event_rx,
      task_running: false,
      pending_approval: None,
    }
  }

  pub(crate) async fn run(
    &mut self,
    terminal: &mut Terminal,
    events: &mut TuiEventStream,
  ) -> Result<AppExitInfo> {
    loop {
      terminal.draw(|frame| self.render(frame))?;

      if let Some(exit_info) = self.exit_info.take() {
        return Ok(exit_info);
      }

      if self.task_running {
        tokio::select! {
          maybe_event = events.next() => {
            if let Some(event) = maybe_event {
              self.handle_tui_event(event).await?;
            }
          }
          core_event = self.cokra.next_event() => {
            match core_event {
              Ok(event) => self.handle_cokra_event(event).await?,
              Err(err) => {
                self.exit_info = Some(self.build_exit_info(ExitReason::Fatal(err.to_string())));
              }
            }
          }
          app_event = self.app_event_rx.recv() => {
            if let Some(app_event) = app_event {
              self.handle_app_event(app_event).await?;
            }
          }
        }
      } else {
        tokio::select! {
          maybe_event = events.next() => {
            if let Some(event) = maybe_event {
              self.handle_tui_event(event).await?;
            }
          }
          app_event = self.app_event_rx.recv() => {
            if let Some(app_event) = app_event {
              self.handle_app_event(app_event).await?;
            }
          }
        }
      }
    }
  }

  fn build_exit_info(&self, reason: ExitReason) -> AppExitInfo {
    AppExitInfo {
      token_usage: self.chat_widget.token_usage(),
      thread_id: self.cokra.thread_id().map(ToString::to_string),
      exit_reason: reason,
    }
  }

  async fn handle_app_event(&mut self, event: AppEvent) -> Result<()> {
    match event {
      AppEvent::CodexOp(op) => {
        let _ = self.cokra.submit(op).await?;
      }
    }
    Ok(())
  }

  async fn handle_tui_event(&mut self, event: TuiEvent) -> Result<()> {
    match event {
      TuiEvent::Key(key) => self.handle_key_event(key).await?,
      TuiEvent::Paste(text) => self.bottom_pane.handle_paste(text),
      TuiEvent::Resize(_, _) | TuiEvent::Draw => {}
      TuiEvent::Tick => {
        self.chat_widget.on_commit_tick();
      }
    }

    Ok(())
  }

  async fn handle_key_event(&mut self, key: KeyEvent) -> Result<()> {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
      if self.task_running {
        let _ = self.cokra.submit(Op::Interrupt).await?;
        return Ok(());
      }

      self.exit_info = Some(self.build_exit_info(ExitReason::UserRequested));
      return Ok(());
    }

    match self.bottom_pane.handle_key(key) {
      BottomPaneAction::None => {}
      BottomPaneAction::Interrupt => {
        if self.task_running {
          let _ = self.cokra.submit(Op::Interrupt).await?;
        }
      }
      BottomPaneAction::RequestQuit => {
        self.exit_info = Some(self.build_exit_info(ExitReason::UserRequested));
      }
      BottomPaneAction::Submit(submission) => {
        self.submit_user_input(submission.text).await?;
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

  async fn submit_user_input(&mut self, text: String) -> Result<()> {
    if text.trim().is_empty() {
      return Ok(());
    }

    self.chat_widget.push_user_input_text(text.clone());

    let _ = self
      .cokra
      .submit(Op::UserInput {
        items: vec![UserInput::Text {
          text,
          text_elements: Vec::new(),
        }],
        final_output_json_schema: None,
      })
      .await?;

    self.task_running = true;
    self.bottom_pane.set_task_running(true);
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
            .bottom_pane
            .open_approval(ApprovalOverlay::new(ApprovalRequest {
              call_id: req.id,
              tool_name: "shell".to_string(),
              command: format!("{}  (cwd: {})", req.command, req.cwd.display()),
            }));
        }
      }
    }

    if turn_finished {
      self.task_running = false;
      self.bottom_pane.set_task_running(false);
      self.chat_widget.set_agent_turn_running(false);
    }

    Ok(())
  }

  fn render(&mut self, frame: &mut Frame) {
    let area = frame.area();
    let bottom_height = self.bottom_pane.desired_height(area.width).min(area.height);

    let chunks =
      Layout::vertical([Constraint::Min(1), Constraint::Length(bottom_height)]).split(area);

    self.chat_widget.render(chunks[0], frame.buffer_mut());
    self
      .bottom_pane
      .render(chunks[1], frame.buffer_mut(), self.task_running);
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
