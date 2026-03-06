use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use cokra_protocol::AgentStatus;
use cokra_protocol::CollabAgentInteractionBeginEvent;
use cokra_protocol::CollabAgentInteractionEndEvent;
use cokra_protocol::CollabAgentSpawnBeginEvent;
use cokra_protocol::CollabAgentSpawnEndEvent;
use cokra_protocol::CollabCloseBeginEvent;
use cokra_protocol::CollabCloseEndEvent;
use cokra_protocol::CollabMessagePostedEvent;
use cokra_protocol::CollabMessagesReadEvent;
use cokra_protocol::CollabTaskUpdatedEvent;
use cokra_protocol::CollabWaitingBeginEvent;
use cokra_protocol::CollabWaitingEndEvent;

use crate::history_cell::PlainHistoryCell;

pub(crate) fn spawn_begin(ev: CollabAgentSpawnBeginEvent) -> PlainHistoryCell {
  PlainHistoryCell::new(vec![
    Line::from(vec!["• ".dim(), "Spawning agent".bold()]),
    Line::from(format!("  └ thread: {}", ev.thread_id).dim()),
    Line::from(format!("  └ agent: {} ({})", ev.agent_id, ev.role).dim()),
  ])
}

pub(crate) fn spawn_end(ev: CollabAgentSpawnEndEvent) -> PlainHistoryCell {
  PlainHistoryCell::new(vec![
    Line::from(vec!["• ".dim(), "Agent spawned".bold()]),
    Line::from(format!("  └ thread: {}", ev.thread_id).dim()),
    Line::from(format!("  └ agent: {} [{}]", ev.agent_id, ev.status).dim()),
  ])
}

pub(crate) fn interaction_begin(ev: CollabAgentInteractionBeginEvent) -> PlainHistoryCell {
  PlainHistoryCell::new(vec![
    Line::from(vec!["• ".dim(), "Sending input".bold()]),
    Line::from(format!("  └ thread: {}", ev.thread_id).dim()),
    Line::from(format!("  └ agent: {}", ev.agent_id).dim()),
  ])
}

pub(crate) fn interaction_end(ev: CollabAgentInteractionEndEvent) -> PlainHistoryCell {
  PlainHistoryCell::new(vec![
    Line::from(vec!["• ".dim(), "Input sent".bold()]),
    Line::from(format!("  └ thread: {}", ev.thread_id).dim()),
    Line::from(format!("  └ agent: {}", ev.agent_id).dim()),
    Line::from(format!("  └ result: {}", ev.result).dim()),
  ])
}

pub(crate) fn waiting_begin(ev: CollabWaitingBeginEvent) -> PlainHistoryCell {
  let mut lines = vec![Line::from(vec!["• ".dim(), "Waiting for agents".bold()])];
  for thread_id in ev.receiver_thread_ids {
    lines.push(Line::from(format!("  └ agent: {thread_id}").dim()));
  }
  PlainHistoryCell::new(lines)
}

pub(crate) fn waiting_end(ev: CollabWaitingEndEvent) -> PlainHistoryCell {
  let mut lines = vec![Line::from(vec!["• ".dim(), "Agents completed".bold()])];
  let mut statuses = ev.statuses.into_iter().collect::<Vec<_>>();
  statuses.sort_by(|left, right| left.0.cmp(&right.0));
  for (thread_id, status) in statuses {
    lines.push(Line::from(vec![
      Span::from("  └ ").dim(),
      Span::from(thread_id).dim(),
      Span::from(": ").dim(),
      status_span(&status),
    ]));
  }
  PlainHistoryCell::new(lines)
}

pub(crate) fn close_begin(ev: CollabCloseBeginEvent) -> PlainHistoryCell {
  PlainHistoryCell::new(vec![
    Line::from(vec!["• ".dim(), "Closing agent".bold()]),
    Line::from(format!("  └ agent: {}", ev.receiver_thread_id).dim()),
  ])
}

pub(crate) fn close_end(ev: CollabCloseEndEvent) -> PlainHistoryCell {
  PlainHistoryCell::new(vec![
    Line::from(vec!["• ".dim(), "Agent closed".bold()]),
    Line::from(format!("  └ agent: {}", ev.receiver_thread_id).dim()),
    Line::from(vec!["  └ status: ".dim(), status_span(&ev.status)]),
  ])
}

pub(crate) fn message_posted(ev: CollabMessagePostedEvent) -> PlainHistoryCell {
  let recipient = ev
    .recipient_thread_id
    .map(|value| format!("to {value}"))
    .unwrap_or_else(|| "to team".to_string());
  PlainHistoryCell::new(vec![
    Line::from(vec!["• ".dim(), "Mailbox message".bold()]),
    Line::from(format!("  └ from: {} {recipient}", ev.sender_thread_id).dim()),
    Line::from(format!("  └ message: {}", ev.message).dim()),
  ])
}

pub(crate) fn messages_read(ev: CollabMessagesReadEvent) -> PlainHistoryCell {
  PlainHistoryCell::new(vec![
    Line::from(vec!["• ".dim(), "Mailbox read".bold()]),
    Line::from(format!("  └ reader: {}", ev.reader_thread_id).dim()),
    Line::from(format!("  └ count: {}", ev.count).dim()),
  ])
}

pub(crate) fn task_updated(ev: CollabTaskUpdatedEvent) -> PlainHistoryCell {
  PlainHistoryCell::new(vec![
    Line::from(vec!["• ".dim(), "Team task updated".bold()]),
    Line::from(format!("  └ actor: {}", ev.actor_thread_id).dim()),
    Line::from(format!("  └ task: {} [{}]", ev.task.title, ev.task.id).dim()),
    Line::from(vec![
      "  └ status: ".dim(),
      task_status_span(&ev.task.status),
    ]),
  ])
}

fn status_span(status: &AgentStatus) -> Span<'static> {
  match status {
    AgentStatus::PendingInit => "pending".dim(),
    AgentStatus::Running => "running".yellow(),
    AgentStatus::Completed(Some(message)) if !message.is_empty() => {
      format!("completed ({message})").green()
    }
    AgentStatus::Completed(_) => "completed".green(),
    AgentStatus::Errored(message) => format!("errored ({message})").red(),
    AgentStatus::Shutdown => "shutdown".dim(),
    AgentStatus::NotFound => "not_found".red(),
  }
}

fn task_status_span(status: &cokra_protocol::TeamTaskStatus) -> Span<'static> {
  match status {
    cokra_protocol::TeamTaskStatus::Pending => "pending".dim(),
    cokra_protocol::TeamTaskStatus::InProgress => "in_progress".yellow(),
    cokra_protocol::TeamTaskStatus::Completed => "completed".green(),
    cokra_protocol::TeamTaskStatus::Failed => "failed".red(),
    cokra_protocol::TeamTaskStatus::Canceled => "canceled".dim(),
  }
}
