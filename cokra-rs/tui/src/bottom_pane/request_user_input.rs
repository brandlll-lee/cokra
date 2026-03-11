use std::cell::RefCell;
use std::collections::HashMap;

use cokra_protocol::Op;
use cokra_protocol::RequestUserInputEvent;
use cokra_protocol::RequestUserInputQuestion;
use cokra_protocol::RequestUserInputQuestionOption;
use cokra_protocol::user_input::RequestUserInputAnswer;
use cokra_protocol::user_input::RequestUserInputResponse;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::prelude::Stylize;
use ratatui::style::Color;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use ratatui::widgets::StatefulWidgetRef;
use ratatui::widgets::Widget;
use textwrap::Options;

use super::bottom_pane_view::BottomPaneView;
use super::selection_popup_common::menu_surface_inset;
use super::selection_popup_common::menu_surface_padding_height;
use super::selection_popup_common::render_menu_surface;
use super::textarea::TextArea;
use super::textarea::TextAreaState;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::history_cell::RequestUserInputResultCell;
use crate::render::renderable::Renderable;

const TITLE: &str = "User Input Required";
const ANSWER_PLACEHOLDER: &str = "Type your answer (optional)";
const NOTES_PLACEHOLDER: &str = "Add notes (optional)";
const OTHER_OPTION_LABEL: &str = "None of the above";
const MIN_INPUT_HEIGHT: u16 = 3;

#[derive(Debug, Clone, Default)]
struct AnswerState {
  selected_option: Option<usize>,
  note: String,
}

pub(crate) struct RequestUserInputView {
  app_event_tx: AppEventSender,
  request: RequestUserInputEvent,
  textarea: TextArea,
  textarea_state: RefCell<TextAreaState>,
  answers: Vec<AnswerState>,
  current_idx: usize,
  complete: bool,
}

impl RequestUserInputView {
  pub(crate) fn new(request: RequestUserInputEvent, app_event_tx: AppEventSender) -> Self {
    let mut view = Self {
      app_event_tx,
      answers: vec![AnswerState::default(); request.questions.len()],
      request,
      textarea: TextArea::new(),
      textarea_state: RefCell::new(TextAreaState::default()),
      current_idx: 0,
      complete: false,
    };
    view.sync_textarea_from_current();
    view
  }

  fn content_width(&self, width: u16) -> u16 {
    menu_surface_inset(Rect::new(0, 0, width, 1)).width.max(1)
  }

  fn current_question(&self) -> Option<&RequestUserInputQuestion> {
    self.request.questions.get(self.current_idx)
  }

  fn current_answer_mut(&mut self) -> Option<&mut AnswerState> {
    self.answers.get_mut(self.current_idx)
  }

  fn current_answer(&self) -> Option<&AnswerState> {
    self.answers.get(self.current_idx)
  }

  fn has_options(&self) -> bool {
    self
      .current_question()
      .and_then(|question| question.options.as_ref())
      .is_some_and(|options| !options.is_empty())
  }

  fn question_count(&self) -> usize {
    self.request.questions.len()
  }

  fn option_count(&self) -> usize {
    let Some(question) = self.current_question() else {
      return 0;
    };
    let base = question.options.as_ref().map_or(0, Vec::len);
    base + usize::from(question.is_other)
  }

  fn option_label(question: &RequestUserInputQuestion, idx: usize) -> Option<String> {
    if let Some(options) = &question.options
      && let Some(option) = options.get(idx)
    {
      return Some(option.label.clone());
    }
    let other_idx = question.options.as_ref().map_or(0, Vec::len);
    (question.is_other && idx == other_idx).then(|| OTHER_OPTION_LABEL.to_string())
  }

  fn select_option(&mut self, idx: usize) {
    if idx >= self.option_count() {
      return;
    }
    if let Some(answer) = self.current_answer_mut() {
      answer.selected_option = Some(idx);
    }
  }

  fn save_current_note(&mut self) {
    let note = self.textarea.text().to_string();
    if let Some(answer) = self.current_answer_mut() {
      answer.note = note;
    }
  }

  fn sync_textarea_from_current(&mut self) {
    let note = self
      .current_answer()
      .map(|answer| answer.note.clone())
      .unwrap_or_default();
    self.textarea.set_text_clearing_elements(&note);
    self.textarea.set_cursor(note.len());
  }

  fn question_height(&self, width: u16) -> u16 {
    let question = self
      .current_question()
      .map(|question| question.question.as_str())
      .unwrap_or_default();
    textwrap::wrap(question, Options::new(self.content_width(width) as usize)).len() as u16
  }

  fn option_lines(&self, width: u16) -> Vec<Line<'static>> {
    let Some(question) = self.current_question() else {
      return Vec::new();
    };
    let selected = self
      .current_answer()
      .and_then(|answer| answer.selected_option);
    let wrap_width = self.content_width(width).saturating_sub(4).max(1) as usize;

    let mut lines = Vec::new();
    if let Some(options) = &question.options {
      for (idx, option) in options.iter().enumerate() {
        lines.extend(self.option_row_lines(idx, option, selected == Some(idx), wrap_width));
      }
    }
    if question.is_other {
      let idx = question.options.as_ref().map_or(0, Vec::len);
      let option = RequestUserInputQuestionOption {
        label: OTHER_OPTION_LABEL.to_string(),
        description: "Optionally add details in notes below.".to_string(),
      };
      lines.extend(self.option_row_lines(idx, &option, selected == Some(idx), wrap_width));
    }
    lines
  }

  fn option_row_lines(
    &self,
    idx: usize,
    option: &RequestUserInputQuestionOption,
    selected: bool,
    width: usize,
  ) -> Vec<Line<'static>> {
    let prefix = if selected { '>' } else { ' ' };
    let label_prefix = format!("{prefix} {}. ", idx + 1);
    let indent = " ".repeat(label_prefix.len());

    let mut lines = Vec::new();
    let wrapped_label = textwrap::wrap(&option.label, width.max(1));
    for (line_idx, segment) in wrapped_label.into_iter().enumerate() {
      let prefix_text = if line_idx == 0 {
        label_prefix.clone()
      } else {
        indent.clone()
      };
      let content = segment.to_string();
      let content_span = if selected {
        Span::from(content).fg(Color::Cyan)
      } else {
        Span::from(content)
      };
      lines.push(Line::from(vec![Span::from(prefix_text), content_span]));
    }

    let desc_wrap_width = width.saturating_sub(2).max(1);
    for segment in textwrap::wrap(&option.description, desc_wrap_width) {
      lines.push(Line::from(vec![
        Span::from("    "),
        Span::from(segment.to_string()).dim(),
      ]));
    }
    lines
  }

  fn option_height(&self, width: u16) -> u16 {
    self.option_lines(width).len() as u16
  }

  fn input_outer_height(&self, width: u16) -> u16 {
    let inner_width = self.content_width(width).saturating_sub(2).max(1);
    self
      .textarea
      .desired_height(inner_width)
      .max(MIN_INPUT_HEIGHT)
      + 2
  }

  fn footer_lines(&self, width: u16) -> Vec<Line<'static>> {
    let is_last = self.current_idx + 1 >= self.question_count();
    let enter_tip = if is_last {
      "enter submit all"
    } else {
      "enter next"
    };
    let footer =
      format!("{enter_tip} | left/right navigate | up/down select option | esc submit now");
    textwrap::wrap(&footer, self.content_width(width) as usize)
      .into_iter()
      .map(|line| Line::from(line.to_string()).dim())
      .collect()
  }

  fn submit_current_and_continue(&mut self) {
    self.save_current_note();
    if self.current_idx + 1 < self.question_count() {
      self.current_idx += 1;
      self.sync_textarea_from_current();
    } else {
      self.submit_all(false);
    }
  }

  fn submit_all(&mut self, interrupted: bool) {
    self.save_current_note();
    let response = self.build_response();
    self
      .app_event_tx
      .insert_history_cell(RequestUserInputResultCell {
        questions: self.request.questions.clone(),
        answers: response.answers.clone(),
        interrupted,
      });
    self
      .app_event_tx
      .send(AppEvent::CodexOp(Op::UserInputAnswer {
        id: self.request.turn_id.clone(),
        response,
      }));
    self.complete = true;
  }

  fn build_response(&self) -> RequestUserInputResponse {
    let mut answers = HashMap::new();

    for (idx, question) in self.request.questions.iter().enumerate() {
      let state = self.answers.get(idx).cloned().unwrap_or_default();
      let note = state.note.trim();
      let mut entries = Vec::new();

      if let Some(selected_idx) = state.selected_option
        && let Some(label) = Self::option_label(question, selected_idx)
      {
        entries.push(label);
      } else if question.options.is_some() && question.is_other && !note.is_empty() {
        entries.push(OTHER_OPTION_LABEL.to_string());
      }

      if !note.is_empty() {
        if question.options.is_some() {
          entries.push(format!("user_note: {note}"));
        } else {
          entries.push(note.to_string());
        }
      }

      answers.insert(
        question.id.clone(),
        RequestUserInputAnswer { answers: entries },
      );
    }

    RequestUserInputResponse { answers }
  }

  fn progress_line(&self) -> Line<'static> {
    let header = self
      .current_question()
      .map(|question| question.header.clone())
      .unwrap_or_else(|| "Question".to_string());
    Line::from(vec![
      Span::from(format!(
        "Question {}/{}",
        self.current_idx + 1,
        self.question_count().max(1)
      ))
      .bold(),
      Span::from("  ").dim(),
      Span::from(header).dim(),
    ])
  }

  fn notes_title(&self) -> &'static str {
    if self.has_options() {
      "Notes"
    } else {
      "Answer"
    }
  }

  fn placeholder(&self) -> &'static str {
    if self.has_options() {
      NOTES_PLACEHOLDER
    } else {
      ANSWER_PLACEHOLDER
    }
  }
}

impl BottomPaneView for RequestUserInputView {
  fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
    self
  }

  fn handle_key_event(&mut self, key_event: KeyEvent) {
    match key_event {
      KeyEvent {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::NONE,
        ..
      } => self.submit_current_and_continue(),
      KeyEvent {
        code: KeyCode::Left,
        modifiers: KeyModifiers::NONE,
        ..
      }
      | KeyEvent {
        code: KeyCode::Char('p'),
        modifiers: KeyModifiers::CONTROL,
        ..
      } => {
        self.save_current_note();
        if self.current_idx > 0 {
          self.current_idx -= 1;
          self.sync_textarea_from_current();
        }
      }
      KeyEvent {
        code: KeyCode::Right,
        modifiers: KeyModifiers::NONE,
        ..
      }
      | KeyEvent {
        code: KeyCode::Char('n'),
        modifiers: KeyModifiers::CONTROL,
        ..
      } => {
        self.save_current_note();
        if self.current_idx + 1 < self.question_count() {
          self.current_idx += 1;
          self.sync_textarea_from_current();
        }
      }
      KeyEvent {
        code: KeyCode::Up,
        modifiers: KeyModifiers::NONE,
        ..
      } => {
        if let Some(answer) = self.current_answer_mut() {
          answer.selected_option = Some(answer.selected_option.unwrap_or(0).saturating_sub(1));
        }
      }
      KeyEvent {
        code: KeyCode::Down,
        modifiers: KeyModifiers::NONE,
        ..
      } => {
        let option_count = self.option_count();
        if option_count > 0 {
          let next = self
            .current_answer()
            .and_then(|answer| answer.selected_option)
            .map(|idx| (idx + 1).min(option_count - 1))
            .unwrap_or(0);
          self.select_option(next);
        }
      }
      KeyEvent {
        code: KeyCode::Char(ch),
        modifiers: KeyModifiers::NONE,
        ..
      } if ch.is_ascii_digit() && ch != '0' => {
        if let Some(idx) = ch.to_digit(10).map(|value| value as usize - 1)
          && idx < self.option_count()
        {
          self.select_option(idx);
          return;
        }
        self.textarea.input(key_event);
      }
      _ => self.textarea.input(key_event),
    }
  }

  fn handle_paste(&mut self, text: String) -> bool {
    self.textarea.insert_str(&text);
    true
  }

  fn is_complete(&self) -> bool {
    self.complete
  }

  fn on_cancel(&mut self) -> bool {
    self.submit_all(true);
    true
  }
}

impl Renderable for RequestUserInputView {
  fn render(&self, area: Rect, buf: &mut Buffer) {
    if area.is_empty() {
      return;
    }
    let outer = render_menu_surface(area, buf);
    if outer.is_empty() {
      return;
    }

    let progress_height = 1;
    let question_height = self.question_height(area.width).max(1);
    let options_height = self.option_height(area.width);
    let input_height = self.input_outer_height(area.width);
    let footer_lines = self.footer_lines(area.width);
    let footer_height = footer_lines.len() as u16;

    let chunks = Layout::vertical([
      Constraint::Length(1),
      Constraint::Length(progress_height),
      Constraint::Length(question_height),
      Constraint::Length(options_height),
      Constraint::Length(input_height),
      Constraint::Length(footer_height.max(1)),
    ])
    .split(outer);

    Paragraph::new(Line::from(TITLE.bold())).render(chunks[0], buf);
    Paragraph::new(self.progress_line()).render(chunks[1], buf);
    Paragraph::new(
      self
        .current_question()
        .map(|question| question.question.clone())
        .unwrap_or_default(),
    )
    .wrap(ratatui::widgets::Wrap { trim: false })
    .render(chunks[2], buf);

    if options_height > 0 {
      Paragraph::new(self.option_lines(area.width))
        .wrap(ratatui::widgets::Wrap { trim: false })
        .render(chunks[3], buf);
    }

    let input_inner = Block::default().borders(Borders::ALL).inner(chunks[4]);
    Block::default()
      .borders(Borders::ALL)
      .title(self.notes_title())
      .render(chunks[4], buf);
    if !input_inner.is_empty() {
      if self.textarea.is_empty() {
        Paragraph::new(Line::from(self.placeholder().dim())).render(input_inner, buf);
      } else if self
        .current_question()
        .is_some_and(|question| question.is_secret)
      {
        self.textarea.render_ref_masked(
          input_inner,
          buf,
          &mut self.textarea_state.borrow_mut(),
          '•',
        );
      } else {
        StatefulWidgetRef::render_ref(
          &&self.textarea,
          input_inner,
          buf,
          &mut self.textarea_state.borrow_mut(),
        );
      }
    }

    Paragraph::new(footer_lines).render(chunks[5], buf);
  }

  fn desired_height(&self, width: u16) -> u16 {
    menu_surface_padding_height()
      + 1
      + 1
      + self.question_height(width).max(1)
      + self.option_height(width)
      + self.input_outer_height(width)
      + self.footer_lines(width).len() as u16
  }

  fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
    if area.is_empty() {
      return None;
    }
    let outer = menu_surface_inset(area);
    let chunks = Layout::vertical([
      Constraint::Length(1),
      Constraint::Length(1),
      Constraint::Length(self.question_height(area.width).max(1)),
      Constraint::Length(self.option_height(area.width)),
      Constraint::Length(self.input_outer_height(area.width)),
      Constraint::Length(self.footer_lines(area.width).len() as u16),
    ])
    .split(outer);
    let input_inner = Block::default().borders(Borders::ALL).inner(chunks[4]);
    if input_inner.is_empty() {
      return None;
    }
    self
      .textarea
      .cursor_pos_with_state(input_inner, *self.textarea_state.borrow())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use tokio::sync::mpsc;

  fn sample_request() -> RequestUserInputEvent {
    RequestUserInputEvent {
      thread_id: "thread".to_string(),
      turn_id: "turn-1".to_string(),
      call_id: "call-1".to_string(),
      questions: vec![
        RequestUserInputQuestion {
          id: "confirm".to_string(),
          header: "Confirm".to_string(),
          question: "Proceed with the plan?".to_string(),
          is_other: true,
          is_secret: false,
          options: Some(vec![
            RequestUserInputQuestionOption {
              label: "Yes".to_string(),
              description: "Continue the plan.".to_string(),
            },
            RequestUserInputQuestionOption {
              label: "No".to_string(),
              description: "Stop and revisit.".to_string(),
            },
          ]),
        },
        RequestUserInputQuestion {
          id: "note".to_string(),
          header: "Note".to_string(),
          question: "Add any note".to_string(),
          is_other: false,
          is_secret: false,
          options: None,
        },
      ],
    }
  }

  #[test]
  fn enter_submits_structured_answers() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let sender = AppEventSender { app_event_tx: tx };
    let mut view = RequestUserInputView::new(sample_request(), sender);

    view.handle_key_event(KeyEvent::from(KeyCode::Char('1')));
    view.handle_paste("because it is ready".to_string());
    view.handle_key_event(KeyEvent::from(KeyCode::Enter));
    view.handle_paste("ship it".to_string());
    view.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert!(matches!(rx.try_recv(), Ok(AppEvent::InsertHistoryCell(_))));
    assert!(matches!(
      rx.try_recv(),
      Ok(AppEvent::CodexOp(Op::UserInputAnswer { id, response }))
        if id == "turn-1"
          && response.answers.get("confirm").is_some_and(|answer| {
            answer.answers == vec!["Yes".to_string(), "user_note: because it is ready".to_string()]
          })
          && response.answers.get("note").is_some_and(|answer| answer.answers == vec!["ship it".to_string()])
    ));
    assert!(view.is_complete());
  }
}
