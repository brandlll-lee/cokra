use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use cokra_protocol::TeamSnapshot;

use crate::multi_agents::TeamDashboardSections;
use crate::multi_agents::team_dashboard_sections;
use crate::render::renderable::Renderable;
use crate::terminal_palette::light_blue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TeamPanelMode {
  Hidden,
  Collapsed,
  Expanded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TeamPanelTab {
  Summary,
  Tasks,
  Mailbox,
  Locks,
}

#[derive(Debug, Clone)]
pub(crate) struct TeamPanel {
  mode: TeamPanelMode,
  tab: TeamPanelTab,
  sections: TeamDashboardSections,
}

impl TeamPanel {
  pub(crate) fn new(
    snapshot: &TeamSnapshot,
    mode: TeamPanelMode,
    tab: TeamPanelTab,
  ) -> Option<Self> {
    if matches!(mode, TeamPanelMode::Hidden) {
      return None;
    }

    Some(Self {
      mode,
      tab,
      sections: team_dashboard_sections(snapshot),
    })
  }

  fn tab_label(tab: TeamPanelTab) -> &'static str {
    match tab {
      TeamPanelTab::Summary => "Summary",
      TeamPanelTab::Tasks => "Tasks",
      TeamPanelTab::Mailbox => "Mailbox",
      TeamPanelTab::Locks => "Locks",
    }
  }

  fn header_line(&self) -> Line<'static> {
    Line::from(vec![
      Span::from("Team Panel").style(light_blue()).bold(),
      Span::from("  ").dim(),
      Span::from("(use /collab to toggle)").dim(),
    ])
  }

  fn tab_line(&self) -> Line<'static> {
    let tabs = [
      TeamPanelTab::Summary,
      TeamPanelTab::Tasks,
      TeamPanelTab::Mailbox,
      TeamPanelTab::Locks,
    ];

    let mut spans = Vec::new();
    for (idx, tab) in tabs.into_iter().enumerate() {
      if idx > 0 {
        spans.push(Span::from("  ").dim());
      }
      let label = Self::tab_label(tab);
      let span = if tab == self.tab {
        Span::from(format!("[{label}]")).style(light_blue()).bold()
      } else {
        Span::from(label).dim()
      };
      spans.push(span);
    }
    Line::from(spans)
  }

  fn selected_lines(&self) -> Vec<Line<'static>> {
    match self.tab {
      TeamPanelTab::Summary => self.sections.summary.clone(),
      TeamPanelTab::Tasks => self.sections.tasks.clone(),
      TeamPanelTab::Mailbox => self.sections.mailbox.clone(),
      TeamPanelTab::Locks => self.sections.locks.clone(),
    }
  }

  fn lines(&self) -> Vec<Line<'static>> {
    match self.mode {
      TeamPanelMode::Hidden => Vec::new(),
      TeamPanelMode::Collapsed => vec![self.sections.folded_header.clone()],
      TeamPanelMode::Expanded => {
        let mut lines = vec![
          self.header_line(),
          self.sections.folded_header.clone(),
          self.tab_line(),
        ];
        let selected = self.selected_lines();
        if !selected.is_empty() {
          lines.push(Line::from(""));
          lines.extend(selected);
        }
        lines
      }
    }
  }
}

impl Renderable for TeamPanel {
  fn render(&self, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
      return;
    }
    Paragraph::new(Text::from(self.lines())).render(area, buf);
  }

  fn desired_height(&self, _width: u16) -> u16 {
    self.lines().len().try_into().unwrap_or(u16::MAX)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use cokra_protocol::CollabAgentLifecycle;
  use cokra_protocol::CollabAgentWaitState;
  use cokra_protocol::CollabTurnOutcome;
  use cokra_protocol::TeamTask;
  use cokra_protocol::TeamTaskReadyState;
  use cokra_protocol::TeamTaskReviewState;
  use cokra_protocol::TeamTaskStatus;
  use std::collections::HashMap;

  fn member(thread_id: &str, nickname: Option<&str>) -> cokra_protocol::TeamMember {
    cokra_protocol::TeamMember {
      thread_id: thread_id.to_string(),
      nickname: nickname.map(ToString::to_string),
      role: "general".to_string(),
      task: "work".to_string(),
      depth: 1,
      state: CollabAgentWaitState {
        lifecycle: CollabAgentLifecycle::Ready,
        turn_outcome: CollabTurnOutcome::Succeeded,
        last_turn_summary: None,
        attention_reason: None,
        pending_wake_count: 0,
      },
    }
  }

  fn task(
    id: &str,
    title: &str,
    status: TeamTaskStatus,
    ready_state: TeamTaskReadyState,
  ) -> TeamTask {
    TeamTask {
      id: id.to_string(),
      title: title.to_string(),
      details: None,
      status,
      ready_state,
      review_state: TeamTaskReviewState::NotRequested,
      owner_thread_id: Some("alpha-thread".to_string()),
      blocked_by_task_ids: Vec::new(),
      blocks_task_ids: Vec::new(),
      blocking_reason: None,
      blockers: Vec::new(),
      requested_scopes: Vec::new(),
      granted_scopes: Vec::new(),
      scope_policy_override: false,
      assignee_thread_id: Some("alpha-thread".to_string()),
      reviewer_thread_id: None,
      workflow_run_id: None,
      created_at: 1,
      updated_at: 1,
      notes: Vec::new(),
    }
  }

  fn snapshot() -> TeamSnapshot {
    TeamSnapshot {
      root_thread_id: "root-thread".to_string(),
      members: vec![
        cokra_protocol::TeamMember {
          thread_id: "root-thread".to_string(),
          nickname: None,
          role: "root".to_string(),
          task: "lead".to_string(),
          depth: 0,
          state: CollabAgentWaitState {
            lifecycle: CollabAgentLifecycle::Ready,
            turn_outcome: CollabTurnOutcome::Succeeded,
            last_turn_summary: None,
            attention_reason: None,
            pending_wake_count: 0,
          },
        },
        member("alpha-thread", Some("alpha")),
      ],
      tasks: vec![
        task(
          "active",
          "Implement feature",
          TeamTaskStatus::InProgress,
          TeamTaskReadyState::Claimed,
        ),
        task(
          "done",
          "Ship release",
          TeamTaskStatus::Completed,
          TeamTaskReadyState::Completed,
        ),
      ],
      task_edges: Vec::new(),
      plans: Vec::new(),
      unread_counts: HashMap::from([
        ("root-thread".to_string(), 0),
        ("alpha-thread".to_string(), 2),
      ]),
      mailbox_version: 0,
      recent_messages: Vec::new(),
      ownership_leases: Vec::new(),
      workflow: None,
    }
  }

  #[test]
  fn collapsed_panel_renders_folded_header_counts() {
    let panel =
      TeamPanel::new(&snapshot(), TeamPanelMode::Collapsed, TeamPanelTab::Summary).expect("panel");
    let lines = panel.lines();
    assert_eq!(lines.len(), 1);
    let rendered = lines[0]
      .spans
      .iter()
      .map(|span| span.content.as_ref())
      .collect::<String>();
    assert!(rendered.contains("Active 1"));
    assert!(rendered.contains("Closed 1"));
    assert!(rendered.contains("Unread 2"));
  }

  #[test]
  fn expanded_panel_renders_tab_specific_content() {
    let panel =
      TeamPanel::new(&snapshot(), TeamPanelMode::Expanded, TeamPanelTab::Tasks).expect("panel");
    let rendered = panel
      .lines()
      .into_iter()
      .map(|line| {
        line
          .spans
          .into_iter()
          .map(|span| span.content)
          .collect::<String>()
      })
      .collect::<Vec<_>>()
      .join("\n");
    assert!(rendered.contains("Team Panel"));
    assert!(rendered.contains("[Tasks]"));
    assert!(rendered.contains("Task graph"));
    assert!(rendered.contains("Implement feature"));
  }
}
