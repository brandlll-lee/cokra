use ratatui::style::Stylize;
use ratatui::text::Line;

use cokra_protocol::CollabAgentInteractionBeginEvent;
use cokra_protocol::CollabAgentInteractionEndEvent;
use cokra_protocol::CollabAgentSpawnBeginEvent;
use cokra_protocol::CollabAgentSpawnEndEvent;

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
