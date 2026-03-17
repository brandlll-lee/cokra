use std::path::PathBuf;
use std::time::Duration;

use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use cokra_protocol::AgentMessageContent;
use cokra_protocol::CollabSummaryCheckpointEvent;
use cokra_protocol::EventMsg;
use cokra_protocol::ExecCommandBeginEvent;
use cokra_protocol::ExecCommandEndEvent;
use cokra_protocol::ExecCommandOutputDeltaEvent;
use cokra_protocol::TextElement;
use cokra_protocol::TodoItemEvent;
use cokra_protocol::TokenCountEvent;
use cokra_protocol::TurnAbortedEvent;
use cokra_protocol::TurnCompleteEvent;
use cokra_protocol::TurnStartedEvent;
use cokra_protocol::UserInput;

use crate::exec_cell::ExecCall;
use crate::exec_cell::ExecCell;
use crate::exec_cell::model::CommandOutput;
use crate::history_cell::AgentMessageCell;
use crate::history_cell::CollabSummaryHistoryCell;
use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;
use crate::history_cell::TodoUpdateCell;
use crate::history_cell::TurnCompleteHistoryCell;
use crate::history_cell::UserHistoryCell;
use crate::multi_agents;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptRenderMode {
  Live,
  HistoryCollapsed,
  HistoryExpanded,
  Replay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptEntryLifecycle {
  Active,
  Settled,
}

#[derive(Debug, Clone)]
pub(crate) struct TranscriptEntry {
  pub(crate) sequence: u64,
  pub(crate) lifecycle: TranscriptEntryLifecycle,
  pub(crate) kind: TranscriptEntryKind,
}

#[derive(Debug, Clone)]
pub(crate) enum TranscriptEntryKind {
  UserMessage {
    text: String,
    text_elements: Vec<TextElement>,
    remote_image_urls: Vec<String>,
  },
  AgentMessage {
    lines: Vec<Line<'static>>,
    is_first_line: bool,
  },
  PlainLines {
    lines: Vec<Line<'static>>,
  },
  ExecGroup(TranscriptExecGroup),
  TodoSnapshot {
    todos: Vec<TodoItemEvent>,
  },
  CollabSummary {
    plain_lines: Vec<String>,
    fingerprint: u64,
  },
  TurnComplete {
    input_tokens: i64,
    output_tokens: i64,
  },
  TurnAborted {
    reason: String,
  },
}

#[derive(Debug, Clone)]
pub(crate) struct TranscriptExecGroup {
  pub(crate) calls: Vec<TranscriptExecCall>,
}

#[derive(Debug, Clone)]
pub(crate) struct TranscriptExecCall {
  pub(crate) command_id: String,
  pub(crate) tool_name: String,
  pub(crate) command: String,
  pub(crate) cwd: PathBuf,
  pub(crate) output: Option<CommandOutput>,
  pub(crate) duration: Option<Duration>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ThreadTranscriptStore {
  next_sequence: u64,
  settled: Vec<TranscriptEntry>,
  active_agent_preview: Option<ActiveAgentPreview>,
  active_exec_group: Option<ActiveExecGroup>,
  active_collab_summary: Option<ActiveCollabSummary>,
  last_committed_collab_fingerprint: Option<u64>,
  last_token_usage: Option<(i64, i64)>,
}

#[derive(Debug, Clone)]
struct ActiveAgentPreview {
  item_id: String,
  text: String,
}

#[derive(Debug, Clone)]
struct ActiveExecGroup {
  calls: Vec<TranscriptExecCall>,
  exploring: bool,
}

#[derive(Debug, Clone)]
struct ActiveCollabSummary {
  plain_lines: Vec<String>,
  fingerprint: u64,
}

impl ThreadTranscriptStore {
  pub(crate) fn is_empty(&self) -> bool {
    self.settled.is_empty()
      && self.active_agent_preview.is_none()
      && self.active_exec_group.is_none()
      && self.active_collab_summary.is_none()
  }

  pub(crate) fn snapshot(&self) -> Vec<TranscriptEntry> {
    let mut entries = self.settled.clone();
    if let Some(preview) = &self.active_agent_preview {
      entries.push(TranscriptEntry {
        sequence: self.next_sequence,
        lifecycle: TranscriptEntryLifecycle::Active,
        kind: TranscriptEntryKind::AgentMessage {
          lines: text_to_lines(&preview.text),
          is_first_line: true,
        },
      });
    }
    if let Some(exec_group) = &self.active_exec_group {
      entries.push(TranscriptEntry {
        sequence: self.next_sequence.saturating_add(1),
        lifecycle: TranscriptEntryLifecycle::Active,
        kind: TranscriptEntryKind::ExecGroup(TranscriptExecGroup {
          calls: exec_group.calls.clone(),
        }),
      });
    }
    if let Some(collab) = &self.active_collab_summary {
      entries.push(TranscriptEntry {
        sequence: self.next_sequence.saturating_add(2),
        lifecycle: TranscriptEntryLifecycle::Active,
        kind: TranscriptEntryKind::CollabSummary {
          plain_lines: collab.plain_lines.clone(),
          fingerprint: collab.fingerprint,
        },
      });
    }
    entries.sort_by(|left, right| left.sequence.cmp(&right.sequence));
    entries
  }

  pub(crate) fn apply_event(&mut self, event: &EventMsg) {
    match event {
      EventMsg::UserMessage(event) => {
        let (text, text_elements, remote_image_urls) = flatten_user_inputs(&event.items);
        self.push_settled(TranscriptEntryKind::UserMessage {
          text,
          text_elements,
          remote_image_urls,
        });
      }
      EventMsg::AgentMessageDelta(event) => self.update_agent_preview(&event.item_id, &event.delta),
      EventMsg::AgentMessageContentDelta(event) => {
        self.update_agent_preview(&event.item_id, &event.delta)
      }
      EventMsg::AgentMessage(event) => {
        self.clear_agent_preview_if_matches(&event.item_id);
        let mut lines = Vec::new();
        for part in &event.content {
          match part {
            AgentMessageContent::Text { text } if !text.trim().is_empty() => {
              lines.push(Line::from(text.clone()));
            }
            _ => {}
          }
        }
        if !lines.is_empty() {
          self.maybe_settle_exploring_exec_group();
          self.push_settled(TranscriptEntryKind::AgentMessage {
            lines,
            is_first_line: true,
          });
        }
      }
      EventMsg::TokenCount(TokenCountEvent {
        input_tokens,
        output_tokens,
        ..
      }) => {
        self.last_token_usage = Some((*input_tokens, *output_tokens));
      }
      EventMsg::ExecCommandBegin(event) => self.on_exec_begin(event),
      EventMsg::ExecCommandOutputDelta(event) => self.on_exec_output_delta(event),
      EventMsg::ExecCommandEnd(event) => self.on_exec_end(event),
      EventMsg::TodoUpdate(event) => {
        self.push_settled(TranscriptEntryKind::TodoSnapshot {
          todos: event.todos.clone(),
        });
      }
      EventMsg::CollabSummaryCheckpoint(event) => self.on_collab_summary_checkpoint(event),
      EventMsg::CollabAgentSpawnEnd(event) => {
        self.push_plain_history_cell(multi_agents::spawn_end(event.clone()))
      }
      EventMsg::CollabAgentInteractionEnd(event) => {
        self.push_plain_history_cell(multi_agents::interaction_end(event.clone()))
      }
      EventMsg::CollabWaitingBegin(event) => {
        self.push_plain_history_cell(multi_agents::waiting_begin(event.clone()))
      }
      EventMsg::CollabWaitingEnd(event) => self.push_history_cell(multi_agents::waiting_end(
        event.clone(),
      )),
      EventMsg::CollabCloseEnd(event) => {
        self.push_plain_history_cell(multi_agents::close_end(event.clone()))
      }
      EventMsg::CollabMailboxDelivered(event) => self.push_history_cell(
        multi_agents::mailbox_delivered(event.clone()),
      ),
      EventMsg::CollabMessagesRead(event) => {
        self.push_plain_history_cell(multi_agents::messages_read(event.clone()))
      }
      EventMsg::CollabMessagePosted(event) => {
        self.push_plain_history_cell(multi_agents::message_posted(event.clone()))
      }
      EventMsg::CollabTaskUpdated(event) => {
        self.push_plain_history_cell(multi_agents::task_updated(event.clone()))
      }
      EventMsg::CollabTeamSnapshot(event) => {
        self.push_plain_history_cell(multi_agents::team_snapshot(event.clone()))
      }
      EventMsg::CollabPlanSubmitted(event) => {
        self.push_plain_history_cell(multi_agents::plan_submitted(event.clone()))
      }
      EventMsg::CollabPlanDecision(event) => {
        self.push_plain_history_cell(multi_agents::plan_decision(event.clone()))
      }
      EventMsg::Warning(event) => self.push_settled(TranscriptEntryKind::PlainLines {
        lines: vec![Line::from(vec![
          Span::from("warning: ").yellow(),
          Span::from(event.message.clone()),
        ])],
      }),
      EventMsg::StreamError(event) => self.push_settled(TranscriptEntryKind::PlainLines {
        lines: vec![Line::from(vec![
          Span::from("stream error: ").red(),
          Span::from(event.error.clone()),
        ])],
      }),
      EventMsg::Error(event) => {
        self.force_settle_active();
        self.push_settled(TranscriptEntryKind::PlainLines {
          lines: vec![Line::from(vec![
            Span::from("error: ").red(),
            Span::from(event.user_facing_message.clone()),
          ])],
        });
      }
      EventMsg::TurnComplete(event) => self.on_turn_complete(event),
      EventMsg::TurnAborted(event) => self.on_turn_aborted(event),
      EventMsg::ThreadNameUpdated(event) => self.push_settled(TranscriptEntryKind::PlainLines {
        lines: vec![Line::from(format!("Thread renamed: {}", event.name))],
      }),
      EventMsg::TurnStarted(TurnStartedEvent { .. }) => {}
      _ => {}
    }
  }

  pub(crate) fn update_live_collab_summary(&mut self, plain_lines: Vec<String>, fingerprint: u64) {
    self.active_collab_summary = Some(ActiveCollabSummary {
      plain_lines,
      fingerprint,
    });
  }

  pub(crate) fn clear_live_collab_summary(&mut self) {
    self.active_collab_summary = None;
  }

  pub(crate) fn force_settle_collab_summary(&mut self) {
    if let Some(active) = self.active_collab_summary.take()
      && self.last_committed_collab_fingerprint != Some(active.fingerprint)
    {
      self.push_settled(TranscriptEntryKind::CollabSummary {
        plain_lines: active.plain_lines,
        fingerprint: active.fingerprint,
      });
      self.last_committed_collab_fingerprint = Some(active.fingerprint);
    }
  }

  pub(crate) fn render_history_cells(
    &self,
    _mode: TranscriptRenderMode,
  ) -> Vec<Box<dyn HistoryCell>> {
    self
      .snapshot()
      .into_iter()
      .map(render_entry_as_history_cell)
      .collect()
  }

  fn push_plain_history_cell(&mut self, cell: PlainHistoryCell) {
    self.push_settled(TranscriptEntryKind::PlainLines { lines: cell.lines });
  }

  fn push_history_cell(&mut self, cell: impl HistoryCell) {
    self.push_settled(TranscriptEntryKind::PlainLines {
      lines: cell.display_lines(160),
    });
  }

  fn push_settled(&mut self, kind: TranscriptEntryKind) {
    let entry = TranscriptEntry {
      sequence: self.next_sequence,
      lifecycle: TranscriptEntryLifecycle::Settled,
      kind,
    };
    self.next_sequence = self.next_sequence.saturating_add(1);
    self.settled.push(entry);
  }

  fn update_agent_preview(&mut self, item_id: &str, delta: &str) {
    if delta.is_empty() {
      return;
    }
    match self.active_agent_preview.as_mut() {
      Some(preview) if preview.item_id == item_id => preview.text.push_str(delta),
      _ => {
        self.active_agent_preview = Some(ActiveAgentPreview {
          item_id: item_id.to_string(),
          text: delta.to_string(),
        });
      }
    }
  }

  fn clear_agent_preview_if_matches(&mut self, item_id: &str) {
    if self
      .active_agent_preview
      .as_ref()
      .is_some_and(|preview| preview.item_id == item_id)
    {
      self.active_agent_preview = None;
    }
  }

  fn on_exec_begin(&mut self, event: &ExecCommandBeginEvent) {
    let call = TranscriptExecCall {
      command_id: event.command_id.clone(),
      tool_name: event.tool_name.clone(),
      command: event.command.clone(),
      cwd: event.cwd.clone(),
      output: None,
      duration: None,
    };
    let is_exploring = is_exploring_tool_name(&call.tool_name);

    match self.active_exec_group.as_mut() {
      Some(group) if group.exploring && is_exploring => {
        group.calls.push(call);
      }
      Some(group) if !group.exploring && group.calls.iter().any(|call| call.output.is_none()) => {
        group.calls.push(call);
      }
      Some(_) => {
        self.force_settle_active_exec_group();
        self.active_exec_group = Some(ActiveExecGroup {
          calls: vec![call],
          exploring: is_exploring,
        });
      }
      None => {
        self.active_exec_group = Some(ActiveExecGroup {
          calls: vec![call],
          exploring: is_exploring,
        });
      }
    }
  }

  fn on_exec_output_delta(&mut self, event: &ExecCommandOutputDeltaEvent) {
    if let Some(call) = self.active_exec_group.as_mut().and_then(|group| {
      group
        .calls
        .iter_mut()
        .rev()
        .find(|call| call.command_id == event.command_id)
    }) {
      let output = call.output.get_or_insert_with(CommandOutput::default);
      output.output.push_str(&event.output);
    }
  }

  fn on_exec_end(&mut self, event: &ExecCommandEndEvent) {
    let Some(group) = self.active_exec_group.as_mut() else {
      return;
    };

    if let Some(call) = group
      .calls
      .iter_mut()
      .rev()
      .find(|call| call.command_id == event.command_id)
    {
      let output = call.output.get_or_insert_with(CommandOutput::default);
      if !event.output.is_empty() {
        if !output.output.is_empty()
          && event.output.len() > output.output.len()
          && event.output.starts_with(&output.output)
        {
          output.output = event.output.clone();
        } else {
          output.output.push_str(&event.output);
        }
      }
      output.exit_code = event.exit_code;
      if call.duration.is_none() {
        call.duration = Some(Duration::from_millis(0));
      }
    }

    if !group.exploring && group.calls.iter().all(|call| call.output.is_some()) {
      self.force_settle_active_exec_group();
    }
  }

  fn on_collab_summary_checkpoint(&mut self, event: &CollabSummaryCheckpointEvent) {
    if self.last_committed_collab_fingerprint == Some(event.fingerprint) {
      self.active_collab_summary = None;
      return;
    }
    self.active_collab_summary = None;
    self.push_settled(TranscriptEntryKind::CollabSummary {
      plain_lines: event.lines.clone(),
      fingerprint: event.fingerprint,
    });
    self.last_committed_collab_fingerprint = Some(event.fingerprint);
  }

  fn on_turn_complete(&mut self, event: &TurnCompleteEvent) {
    self.force_settle_active();
    if matches!(event.status, cokra_protocol::CompletionStatus::Success) {
      let (input_tokens, output_tokens) = self.last_token_usage.unwrap_or_default();
      self.push_settled(TranscriptEntryKind::TurnComplete {
        input_tokens,
        output_tokens,
      });
    }
  }

  fn on_turn_aborted(&mut self, event: &TurnAbortedEvent) {
    self.force_settle_active();
    self.push_settled(TranscriptEntryKind::TurnAborted {
      reason: event.reason.clone(),
    });
  }

  fn force_settle_active(&mut self) {
    if let Some(preview) = self.active_agent_preview.take()
      && !preview.text.trim().is_empty()
    {
      self.push_settled(TranscriptEntryKind::AgentMessage {
        lines: text_to_lines(&preview.text),
        is_first_line: true,
      });
    }
    self.force_settle_active_exec_group();
    self.force_settle_collab_summary();
  }

  fn maybe_settle_exploring_exec_group(&mut self) {
    if self
      .active_exec_group
      .as_ref()
      .is_some_and(|group| group.exploring && group.calls.iter().all(|call| call.output.is_some()))
    {
      self.force_settle_active_exec_group();
    }
  }

  fn force_settle_active_exec_group(&mut self) {
    if let Some(mut group) = self.active_exec_group.take() {
      for call in &mut group.calls {
        if call.output.is_none() {
          call.output = Some(CommandOutput {
            exit_code: 1,
            output: String::new(),
          });
        }
        if call.duration.is_none() {
          call.duration = Some(Duration::from_millis(0));
        }
      }
      self.push_settled(TranscriptEntryKind::ExecGroup(TranscriptExecGroup {
        calls: group.calls,
      }));
    }
  }
}

fn flatten_user_inputs(items: &[UserInput]) -> (String, Vec<TextElement>, Vec<String>) {
  let mut text_parts = Vec::new();
  let mut text_elements = Vec::new();
  let mut remote_image_urls = Vec::new();
  let mut byte_offset = 0usize;

  for item in items {
    match item {
      UserInput::Text {
        text,
        text_elements: elements,
      } => {
        if !text_parts.is_empty() {
          byte_offset += 1;
        }
        for element in elements {
          text_elements.push(TextElement {
            byte_range: cokra_protocol::ByteRange {
              start: element.byte_range.start + byte_offset,
              end: element.byte_range.end + byte_offset,
            },
            placeholder: element.placeholder.clone(),
          });
        }
        byte_offset += text.len();
        text_parts.push(text.clone());
      }
      UserInput::Image { image_url } => remote_image_urls.push(image_url.clone()),
      UserInput::LocalImage { path } => {
        if !text_parts.is_empty() {
          byte_offset += 1;
        }
        let text = format!("[local_image] {}", path.display());
        byte_offset += text.len();
        text_parts.push(text);
      }
      UserInput::Skill { name, .. } => {
        if !text_parts.is_empty() {
          byte_offset += 1;
        }
        let text = format!("[skill] {name}");
        byte_offset += text.len();
        text_parts.push(text);
      }
      UserInput::Mention { name, path } => {
        if !text_parts.is_empty() {
          byte_offset += 1;
        }
        let text = format!("[@{name}] {path}");
        byte_offset += text.len();
        text_parts.push(text);
      }
    }
  }

  (text_parts.join("\n"), text_elements, remote_image_urls)
}

fn text_to_lines(text: &str) -> Vec<Line<'static>> {
  text
    .lines()
    .filter(|line| !line.trim().is_empty())
    .map(|line| Line::from(line.to_string()))
    .collect()
}

fn render_entry_as_history_cell(entry: TranscriptEntry) -> Box<dyn HistoryCell> {
  match entry.kind {
    TranscriptEntryKind::UserMessage {
      text,
      text_elements,
      remote_image_urls,
    } => Box::new(UserHistoryCell::new(text, text_elements, remote_image_urls)),
    TranscriptEntryKind::AgentMessage {
      lines,
      is_first_line,
    } => Box::new(AgentMessageCell::new(lines, is_first_line)),
    TranscriptEntryKind::PlainLines { lines } => Box::new(PlainHistoryCell::new(lines)),
    TranscriptEntryKind::ExecGroup(group) => Box::new(exec_group_history_cell(group)),
    TranscriptEntryKind::TodoSnapshot { todos } => Box::new(TodoUpdateCell::new(todos)),
    TranscriptEntryKind::CollabSummary { plain_lines, .. } => {
      Box::new(CollabSummaryHistoryCell::from_plain_lines(plain_lines))
    }
    TranscriptEntryKind::TurnComplete {
      input_tokens,
      output_tokens,
    } => Box::new(TurnCompleteHistoryCell {
      elapsed_seconds: None,
      input_tokens,
      output_tokens,
    }),
    TranscriptEntryKind::TurnAborted { reason } => Box::new(PlainHistoryCell::new(vec![
      Line::from(vec![Span::from("aborted: ").yellow(), Span::from(reason)]),
    ])),
  }
}

fn exec_group_history_cell(group: TranscriptExecGroup) -> ExecCell {
  let mut calls = group
    .calls
    .into_iter()
    .map(|call| ExecCall {
      command_id: call.command_id,
      tool_name: call.tool_name,
      command: call.command,
      cwd: call.cwd,
      output: call.output,
      start_time: None,
      duration: call.duration,
    })
    .collect::<Vec<_>>();
  let first = calls.remove(0);
  let mut cell = ExecCell::new(first, false);
  for call in calls {
    cell.push_call(call);
  }
  cell
}

fn is_exploring_tool_name(tool_name: &str) -> bool {
  matches!(
    tool_name,
    "read_file"
      | "list_dir"
      | "grep_files"
      | "search_tool"
      | "code_search"
      | "glob"
      | "read_many_files"
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn force_settle_moves_live_collab_summary_into_history() {
    let mut store = ThreadTranscriptStore::default();
    store.update_live_collab_summary(
      vec![
        "Agent teams working...".to_string(),
        " └─ @alpha".to_string(),
      ],
      42,
    );

    store.force_settle_collab_summary();

    let snapshot = store.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert!(matches!(
      snapshot[0].kind,
      TranscriptEntryKind::CollabSummary { .. }
    ));
  }

  #[test]
  fn exec_group_keeps_full_output_until_settled() {
    let mut store = ThreadTranscriptStore::default();
    store.apply_event(&EventMsg::ExecCommandBegin(ExecCommandBeginEvent {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      command_id: "call-1".to_string(),
      tool_name: "shell".to_string(),
      command: "echo hi".to_string(),
      cwd: PathBuf::from("/tmp/project"),
    }));
    store.apply_event(&EventMsg::ExecCommandOutputDelta(
      ExecCommandOutputDeltaEvent {
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
        command_id: "call-1".to_string(),
        output: "line one\nline two\n".to_string(),
      },
    ));
    store.apply_event(&EventMsg::ExecCommandEnd(ExecCommandEndEvent {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      command_id: "call-1".to_string(),
      exit_code: 0,
      output: String::new(),
    }));

    let snapshot = store.snapshot();
    assert_eq!(snapshot.len(), 1);
    let TranscriptEntryKind::ExecGroup(group) = &snapshot[0].kind else {
      panic!("expected exec group");
    };
    assert_eq!(
      group.calls[0]
        .output
        .as_ref()
        .map(|output| output.output.as_str()),
      Some("line one\nline two\n")
    );
  }

  #[test]
  fn turn_complete_forces_active_entries_to_settle() {
    let mut store = ThreadTranscriptStore::default();
    store.update_live_collab_summary(vec!["Agent teams working...".to_string()], 7);
    store.apply_event(&EventMsg::TurnComplete(TurnCompleteEvent {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      status: cokra_protocol::CompletionStatus::Success,
      end_time: 0,
    }));

    let snapshot = store.snapshot();
    assert!(
      snapshot
        .iter()
        .any(|entry| matches!(entry.kind, TranscriptEntryKind::CollabSummary { .. })),
      "expected turn completion to settle the live collab summary"
    );
  }
}
