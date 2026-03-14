use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use cokra_protocol::AgentStatus;
use cokra_protocol::CollabAgentInteractionEndEvent;
use cokra_protocol::CollabAgentRef;
use cokra_protocol::CollabAgentSpawnEndEvent;
use cokra_protocol::CollabAgentStatusEntry;
use cokra_protocol::CollabCloseEndEvent;
use cokra_protocol::CollabMessagePostedEvent;
use cokra_protocol::CollabMessagesReadEvent;
use cokra_protocol::CollabPlanDecisionEvent;
use cokra_protocol::CollabPlanSubmittedEvent;
use cokra_protocol::CollabTaskUpdatedEvent;
use cokra_protocol::CollabTeamSnapshotEvent;
use cokra_protocol::CollabWaitingBeginEvent;
use cokra_protocol::CollabWaitingEndEvent;
use cokra_protocol::TeamMember;

use crate::history_cell::CollabWaitStatusTreeCell;
use crate::history_cell::CollabWaitStatusTreeEntry;
use crate::history_cell::PlainHistoryCell;
use crate::terminal_palette::light_blue;

const PROMPT_PREVIEW_CHARS: usize = 120;
const STATUS_PREVIEW_CHARS: usize = 160;
const TASK_PREVIEW_CHARS: usize = 96;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WaitingPreview {
  pub(crate) summary: String,
  pub(crate) details: Option<String>,
  pub(crate) receiver_count: usize,
}

pub(crate) fn waiting_preview(ev: &CollabWaitingBeginEvent) -> WaitingPreview {
  let receivers = merge_wait_receivers(&ev.receiver_thread_ids, ev.receiver_agents.clone());

  let summary = match receivers.as_slice() {
    [receiver] => format!(
      "Waiting for {}",
      agent_label_display(agent_label_from_ref(receiver))
    ),
    [] => "Waiting for agents".to_string(),
    _ => format!("Waiting for {} agents", receivers.len()),
  };

  let details = if receivers.len() > 1 {
    Some(
      receivers
        .iter()
        .map(|receiver| agent_label_display(agent_label_from_ref(receiver)))
        .collect::<Vec<_>>()
        .join("\n"),
    )
  } else {
    None
  };

  WaitingPreview {
    summary,
    details,
    receiver_count: receivers.len(),
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentPickerThreadEntry {
  pub(crate) nickname: Option<String>,
  pub(crate) role: Option<String>,
  pub(crate) is_closed: bool,
}

#[derive(Clone, Copy)]
struct AgentLabel<'a> {
  thread_id: &'a str,
  nickname: Option<&'a str>,
  role: Option<&'a str>,
}

pub(crate) fn format_agent_picker_item_name(
  nickname: Option<&str>,
  role: Option<&str>,
  is_primary: bool,
) -> String {
  if is_primary {
    return "@main".to_string();
  }

  let nickname = nickname.map(str::trim).filter(|value| !value.is_empty());
  let role = role
    .map(str::trim)
    .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("default"));
  match (nickname, role) {
    (Some(nickname), Some(role)) => format!("@{nickname} ({role})"),
    (Some(nickname), None) => format!("@{nickname}"),
    (None, Some(role)) => format!("@agent ({role})"),
    (None, None) => "@agent".to_string(),
  }
}

pub(crate) fn sort_agent_picker_threads(
  threads: &mut [(String, AgentPickerThreadEntry)],
  primary_thread_id: &str,
) {
  threads.sort_by(|(left_id, left), (right_id, right)| {
    let left_priority = if left_id == primary_thread_id {
      0
    } else if left.is_closed {
      2
    } else {
      1
    };
    let right_priority = if right_id == primary_thread_id {
      0
    } else if right.is_closed {
      2
    } else {
      1
    };
    left_priority
      .cmp(&right_priority)
      .then_with(|| left_id.cmp(right_id))
  });
}

pub(crate) fn spawn_end(ev: CollabAgentSpawnEndEvent) -> PlainHistoryCell {
  let title = title_with_agent(
    "Spawned",
    AgentLabel {
      thread_id: &ev.agent_id,
      nickname: ev.nickname.as_deref(),
      role: ev.role.as_deref(),
    },
  );

  let mut details = vec![status_summary_line(&ev.status)];
  if let Some(task) = ev
    .task
    .as_deref()
    .and_then(|task| preview_line(task, TASK_PREVIEW_CHARS))
  {
    details.push(task);
  }
  collab_event(title, details)
}

pub(crate) fn interaction_end(ev: CollabAgentInteractionEndEvent) -> PlainHistoryCell {
  let title = title_with_agent(
    "Sent input to",
    AgentLabel {
      thread_id: &ev.agent_id,
      nickname: ev.nickname.as_deref(),
      role: ev.role.as_deref(),
    },
  );

  let mut details = Vec::new();
  if let Some(line) = preview_line(&ev.message, PROMPT_PREVIEW_CHARS) {
    details.push(line);
  }
  details.push(status_summary_line(&ev.status));
  collab_event(title, details)
}

pub(crate) fn waiting_begin(ev: CollabWaitingBeginEvent) -> PlainHistoryCell {
  let receivers = merge_wait_receivers(&ev.receiver_thread_ids, ev.receiver_agents);
  let title = match receivers.as_slice() {
    [receiver] => title_with_agent("Waiting for", agent_label_from_ref(receiver)),
    [] => title_text("Waiting for agents"),
    _ => title_text(format!("Waiting for {} agents", receivers.len())),
  };

  let details = if receivers.len() > 1 {
    receivers
      .iter()
      .map(|receiver| agent_label_line(agent_label_from_ref(receiver)))
      .collect()
  } else {
    Vec::new()
  };

  collab_event(title, details)
}

pub(crate) fn waiting_end(ev: CollabWaitingEndEvent) -> CollabWaitStatusTreeCell {
  CollabWaitStatusTreeCell::new(wait_complete_entries(&ev.statuses, &ev.agent_statuses))
}

pub(crate) fn close_end(ev: CollabCloseEndEvent) -> PlainHistoryCell {
  let title = title_with_agent(
    "Closed",
    AgentLabel {
      thread_id: &ev.receiver_thread_id,
      nickname: ev.receiver_nickname.as_deref(),
      role: ev.receiver_role.as_deref(),
    },
  );
  collab_event(title, vec![status_summary_line(&ev.status)])
}

pub(crate) fn message_posted(ev: CollabMessagePostedEvent) -> PlainHistoryCell {
  let title = title_text("Team message");
  let sender = agent_label_line(AgentLabel {
    thread_id: &ev.sender_thread_id,
    nickname: ev.sender_nickname.as_deref(),
    role: ev.sender_role.as_deref(),
  });
  let recipient = ev
    .recipient_thread_id
    .as_deref()
    .filter(|value| !value.trim().is_empty())
    .map(|thread_id| {
      agent_label_line(AgentLabel {
        thread_id,
        nickname: ev.recipient_nickname.as_deref(),
        role: ev.recipient_role.as_deref(),
      })
    })
    .unwrap_or_else(|| Line::from("Entire team"));
  let mut details = vec![message_flow_line(sender, recipient)];
  if let Some(line) = preview_line(&ev.message, PROMPT_PREVIEW_CHARS) {
    details.push(line);
  }
  collab_event(title, details)
}

pub(crate) fn messages_read(ev: CollabMessagesReadEvent) -> PlainHistoryCell {
  let reader = agent_label_line(AgentLabel {
    thread_id: &ev.reader_thread_id,
    nickname: ev.reader_nickname.as_deref(),
    role: ev.reader_role.as_deref(),
  });
  collab_event(
    title_text("Team messages read"),
    vec![read_summary_line(reader, ev.count)],
  )
}

pub(crate) fn task_updated(ev: CollabTaskUpdatedEvent) -> PlainHistoryCell {
  let title = title_text("Team task updated");
  let assignee = ev
    .task
    .assignee_thread_id
    .as_deref()
    .map(short_thread_id)
    .unwrap_or_else(|| "未分配".to_string());
  let mut details = vec![
    Line::from(format!(
      "#{} {}",
      ev.task.id,
      normalize_preview(&ev.task.title, TASK_PREVIEW_CHARS)
    )),
    Line::from(format!(
      "负责人 {} -> {}",
      short_thread_id(&ev.actor_thread_id),
      assignee
    )),
    status_task_line(&ev.task.status),
  ];
  if let Some(details_preview) = ev
    .task
    .details
    .as_deref()
    .and_then(|details| preview_line(details, TASK_PREVIEW_CHARS))
  {
    details.push(details_preview);
  }
  collab_event(title, details)
}

pub(crate) fn team_snapshot(ev: CollabTeamSnapshotEvent) -> PlainHistoryCell {
  let mut details = Vec::new();
  let teammate_count = ev.snapshot.members.len().saturating_sub(1);
  let unread_members = ev
    .snapshot
    .members
    .iter()
    .filter(|member| member.thread_id != ev.snapshot.root_thread_id)
    .filter_map(|member| {
      let unread = ev
        .snapshot
        .unread_counts
        .get(&member.thread_id)
        .copied()
        .unwrap_or(0);
      (unread > 0).then_some((member, unread))
    })
    .collect::<Vec<_>>();
  let pending_plan_count = ev
    .snapshot
    .plans
    .iter()
    .filter(|plan| {
      matches!(
        plan.status,
        cokra_protocol::TeamPlanStatus::Draft | cokra_protocol::TeamPlanStatus::PendingApproval
      )
    })
    .count();

  details.push(Line::from(format!(
    "Owner @main • {} teammates • {} tasks • {} plans",
    teammate_count,
    ev.snapshot.tasks.len(),
    ev.snapshot.plans.len(),
  )));

  for member in &ev.snapshot.members {
    if member.thread_id == ev.snapshot.root_thread_id {
      continue;
    }
    details.push(member_status_line(
      member,
      &ev.snapshot.root_thread_id,
      &ev.snapshot.unread_counts,
    ));
  }

  if teammate_count == 0 {
    details.push(Line::from("No teammates yet".dim()));
  }

  if !unread_members.is_empty() {
    details.push(unread_summary_line(
      &ev.snapshot.root_thread_id,
      &unread_members,
    ));
  }

  if pending_plan_count > 0 {
    details.push(Line::from(format!(
      "{pending_plan_count} plan(s) pending review"
    )));
  }

  if let Some(run_snapshot) = &ev.snapshot.workflow {
    let open_runs = run_snapshot
      .runs
      .iter()
      .filter(|run| {
        matches!(
          run.status,
          cokra_protocol::WorkflowRunStatus::Pending
            | cokra_protocol::WorkflowRunStatus::Active
            | cokra_protocol::WorkflowRunStatus::WaitingApproval
        )
      })
      .count();
    if !run_snapshot.runs.is_empty() {
      details.push(Line::from(format!(
        "{} resumable run(s), {} currently open",
        run_snapshot.runs.len(),
        open_runs
      )));
    }
  }

  if !ev.snapshot.tasks.is_empty() {
    // Tradeoff: 这里默认折叠整个任务板，而不是尝试猜哪些任务属于“当前轮次”。
    // TeamState 会跨会话持久化，渲染层缺少可靠的会话边界信息；直接折叠能稳定隔离历史遗留任务，
    // 避免主 leader 面板再次被旧任务和底层状态淹没。
    details.push(Line::from(format!(
      "{0} task(s) collapsed by default (historical/background tasks hidden)",
      ev.snapshot.tasks.len()
    )));
  }

  collab_event(title_text("Team status"), details)
}

pub(crate) fn plan_submitted(ev: CollabPlanSubmittedEvent) -> PlainHistoryCell {
  let title = title_text("Team plan submitted");
  let details = vec![
    Line::from(format!("Author {}", short_thread_id(&ev.actor_thread_id))),
    Line::from(normalize_preview(&ev.plan.summary, TASK_PREVIEW_CHARS)),
    Line::from(format!("Status {}", short_plan_status(&ev.plan.status))),
  ];
  collab_event(title, details)
}

pub(crate) fn plan_decision(ev: CollabPlanDecisionEvent) -> PlainHistoryCell {
  let title = title_text("Team plan reviewed");
  let details = vec![
    Line::from(format!("Reviewer {}", short_thread_id(&ev.actor_thread_id))),
    Line::from(normalize_preview(&ev.plan.summary, TASK_PREVIEW_CHARS)),
    Line::from(format!("Status {}", short_plan_status(&ev.plan.status))),
  ];
  collab_event(title, details)
}

fn collab_event(title: Line<'static>, details: Vec<Line<'static>>) -> PlainHistoryCell {
  let mut lines = vec![title];
  if !details.is_empty() {
    let mut is_first = true;
    for detail in details {
      let prefix = if is_first { "  └ " } else { "    " };
      lines.push(prefix_detail_line(prefix, detail));
      is_first = false;
    }
  }
  PlainHistoryCell::new(lines)
}

fn prefix_detail_line(prefix: &str, line: Line<'static>) -> Line<'static> {
  let mut spans = Vec::with_capacity(line.spans.len() + 1);
  spans.push(Span::from(prefix.to_string()).dim());
  spans.extend(line.spans);
  Line::from(spans)
}

fn message_flow_line(sender: Line<'static>, recipient: Line<'static>) -> Line<'static> {
  let mut spans = sender.spans;
  spans.push(Span::from(" -> ").dim());
  spans.extend(recipient.spans);
  Line::from(spans)
}

fn read_summary_line(reader: Line<'static>, count: usize) -> Line<'static> {
  let mut spans = reader.spans;
  spans.push(Span::from(" read ").dim());
  spans.push(Span::from(format!("{count}")));
  spans.push(Span::from(" team message(s)").dim());
  Line::from(spans)
}

fn unread_summary_line(
  root_thread_id: &str,
  unread_members: &[(&TeamMember, usize)],
) -> Line<'static> {
  let mut spans = vec![Span::from("Unread: ").dim()];
  let mut first = true;
  for (member, unread) in unread_members {
    if !first {
      spans.push(Span::from(", ").dim());
    }
    first = false;
    spans.extend(team_member_label_spans(member, root_thread_id));
    spans.push(Span::from(" ").dim());
    spans.push(Span::from(format!("{unread}")));
    spans.push(Span::from(" msg").dim());
  }
  Line::from(spans)
}

fn title_text(title: impl Into<String>) -> Line<'static> {
  Line::from(vec!["● ".dim(), Span::from(title.into()).bold()])
}

fn title_with_agent(prefix: &str, agent: AgentLabel<'_>) -> Line<'static> {
  let mut spans = vec![Span::from(format!("{prefix} ")).bold()];
  spans.extend(agent_label_spans(agent));
  let mut title = vec![Span::from("● ").dim()];
  title.extend(spans);
  Line::from(title)
}

fn agent_label_from_ref(agent: &CollabAgentRef) -> AgentLabel<'_> {
  AgentLabel {
    thread_id: &agent.thread_id,
    nickname: agent.nickname.as_deref(),
    role: agent.role.as_deref(),
  }
}

fn agent_label_line(agent: AgentLabel<'_>) -> Line<'static> {
  Line::from(agent_label_spans(agent))
}

fn agent_label_display(agent: AgentLabel<'_>) -> String {
  let base = if let Some(nickname) = agent
    .nickname
    .map(str::trim)
    .filter(|value| !value.is_empty())
  {
    format!("@{nickname}")
  } else {
    format!("@{}", short_thread_id(agent.thread_id))
  };

  let role = agent.role.map(str::trim).filter(|value| {
    !value.is_empty()
      && !value.eq_ignore_ascii_case("default")
      && !value.eq_ignore_ascii_case("leader")
  });
  if let Some(role) = role {
    format!("{base} [{role}]")
  } else {
    base
  }
}

fn agent_label_spans(agent: AgentLabel<'_>) -> Vec<Span<'static>> {
  let mut spans = Vec::new();
  if let Some(nickname) = agent
    .nickname
    .map(str::trim)
    .filter(|value| !value.is_empty())
  {
    spans.push(
      Span::from(format!("@{nickname}"))
        .style(light_blue())
        .bold(),
    );
  } else {
    spans.push(Span::from(format!("@{}", short_thread_id(agent.thread_id))).style(light_blue()));
  }

  if let Some(role) = agent.role.map(str::trim).filter(|value| {
    !value.is_empty()
      && !value.eq_ignore_ascii_case("default")
      && !value.eq_ignore_ascii_case("leader")
  }) {
    spans.push(Span::from(" ").dim());
    spans.push(Span::from(format!("[{role}]")));
  }

  spans
}

fn merge_wait_receivers(
  receiver_thread_ids: &[String],
  mut receiver_agents: Vec<CollabAgentRef>,
) -> Vec<CollabAgentRef> {
  for thread_id in receiver_thread_ids {
    if receiver_agents
      .iter()
      .any(|agent| agent.thread_id == *thread_id)
    {
      continue;
    }
    receiver_agents.push(CollabAgentRef {
      thread_id: thread_id.clone(),
      nickname: None,
      role: None,
    });
  }
  receiver_agents
}

fn wait_complete_entries(
  statuses: &std::collections::HashMap<String, AgentStatus>,
  agent_statuses: &[CollabAgentStatusEntry],
) -> Vec<CollabWaitStatusTreeEntry> {
  if statuses.is_empty() && agent_statuses.is_empty() {
    return Vec::new();
  }

  let entries = if agent_statuses.is_empty() {
    let mut entries = statuses
      .iter()
      .map(|(thread_id, status)| CollabAgentStatusEntry {
        thread_id: thread_id.clone(),
        nickname: None,
        role: None,
        status: status.clone(),
      })
      .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.thread_id.cmp(&right.thread_id));
    entries
  } else {
    let mut entries = agent_statuses.to_vec();
    entries.sort_by(|left, right| left.thread_id.cmp(&right.thread_id));
    entries
  };

  entries
    .into_iter()
    .map(|entry| CollabWaitStatusTreeEntry {
      label: agent_label_line(AgentLabel {
        thread_id: &entry.thread_id,
        nickname: entry.nickname.as_deref(),
        role: entry.role.as_deref(),
      }),
      summary: status_summary_line(&entry.status),
    })
    .collect()
}

fn status_summary_line(status: &AgentStatus) -> Line<'static> {
  Line::from(status_summary_spans(status))
}

fn status_summary_spans(status: &AgentStatus) -> Vec<Span<'static>> {
  match status {
    AgentStatus::PendingInit => vec![Span::from("Pending init").style(light_blue())],
    AgentStatus::Running => vec![Span::from("Running").yellow().bold()],
    AgentStatus::Completed(message) => {
      let mut spans = vec![Span::from("Completed").green()];
      if let Some(message) = message
        .as_deref()
        .map(|value| normalize_preview(value, STATUS_PREVIEW_CHARS))
        .filter(|value| !value.is_empty())
      {
        spans.push(Span::from(" · ").dim());
        spans.push(Span::from(message));
      }
      spans
    }
    AgentStatus::Errored(message) => vec![
      Span::from("Errored").red(),
      Span::from(" · ").dim(),
      Span::from(normalize_preview(message, STATUS_PREVIEW_CHARS)),
    ],
    AgentStatus::Shutdown => vec![Span::from("Closed").dim()],
    AgentStatus::NotFound => vec![Span::from("Not found").red()],
  }
}

fn member_status_line(
  member: &TeamMember,
  root_thread_id: &str,
  unread_counts: &std::collections::HashMap<String, usize>,
) -> Line<'static> {
  let mut spans = team_member_label_spans(member, root_thread_id);
  spans.push(Span::from(": ").dim());
  spans.extend(status_summary_spans(&member.status));

  let unread = unread_counts.get(&member.thread_id).copied().unwrap_or(0);
  if unread > 0 {
    spans.push(Span::from(" · ").dim());
    spans.push(Span::from(format!("{unread} unread")));
  }

  Line::from(spans)
}

fn team_member_label_spans(member: &TeamMember, root_thread_id: &str) -> Vec<Span<'static>> {
  if member.thread_id == root_thread_id {
    return vec![Span::from("@main").style(light_blue()).bold()];
  }

  agent_label_spans(AgentLabel {
    thread_id: &member.thread_id,
    nickname: member.nickname.as_deref(),
    role: Some(&member.role),
  })
}

fn status_task_line(status: &cokra_protocol::TeamTaskStatus) -> Line<'static> {
  let span = match status {
    cokra_protocol::TeamTaskStatus::Pending => Span::from("pending").dim(),
    cokra_protocol::TeamTaskStatus::InProgress => Span::from("in_progress").yellow(),
    cokra_protocol::TeamTaskStatus::Review => Span::from("review").style(light_blue()),
    cokra_protocol::TeamTaskStatus::Completed => Span::from("completed").green(),
    cokra_protocol::TeamTaskStatus::Failed => Span::from("failed").red(),
    cokra_protocol::TeamTaskStatus::Canceled => Span::from("canceled").dim(),
  };
  Line::from(vec!["Status ".dim(), span])
}

fn short_plan_status(status: &cokra_protocol::TeamPlanStatus) -> &'static str {
  match status {
    cokra_protocol::TeamPlanStatus::Draft => "draft",
    cokra_protocol::TeamPlanStatus::PendingApproval => "pending_approval",
    cokra_protocol::TeamPlanStatus::Approved => "approved",
    cokra_protocol::TeamPlanStatus::Rejected => "rejected",
  }
}

fn preview_line(text: &str, limit: usize) -> Option<Line<'static>> {
  let preview = normalize_preview(text, limit);
  if preview.is_empty() {
    None
  } else {
    Some(Line::from(preview))
  }
}

fn normalize_preview(text: &str, limit: usize) -> String {
  let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
  if collapsed.chars().count() <= limit {
    return collapsed;
  }

  let preview = collapsed.chars().take(limit).collect::<String>();
  format!("{preview}…")
}

fn short_thread_id(thread_id: &str) -> String {
  let short = thread_id.chars().take(8).collect::<String>();
  if short.is_empty() {
    "agent".to_string()
  } else {
    short
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::history_cell::HistoryCell;
  use std::collections::HashMap;

  use cokra_protocol::CollabAgentStatusEntry;
  use cokra_protocol::CollabMessagePostedEvent;
  use cokra_protocol::CollabMessagesReadEvent;
  use cokra_protocol::CollabTeamSnapshotEvent;
  use cokra_protocol::CollabWaitingEndEvent;
  use cokra_protocol::TeamPlan;
  use cokra_protocol::TeamPlanStatus;
  use cokra_protocol::TeamSnapshot;
  use cokra_protocol::TeamTask;
  use cokra_protocol::TeamTaskStatus;

  #[test]
  fn preview_collapses_whitespace_and_truncates() {
    let preview = normalize_preview("alpha   beta\ngamma", 10);
    assert_eq!(preview, "alpha beta…");
  }

  #[test]
  fn agent_label_prefers_nickname_over_thread_id() {
    let line = agent_label_line(AgentLabel {
      thread_id: "12345678-aaaa-bbbb-cccc-111111111111",
      nickname: Some("艾许"),
      role: Some("reviewer"),
    });
    let rendered = line
      .spans
      .iter()
      .map(|span| span.content.as_ref())
      .collect::<String>();
    assert!(rendered.contains("艾许"));
    assert!(rendered.contains("[reviewer]"));
    assert!(!rendered.contains("12345678"));
  }

  #[test]
  fn team_snapshot_renders_compact_summary_card() {
    let cell = team_snapshot(CollabTeamSnapshotEvent {
      actor_thread_id: "root-thread".to_string(),
      snapshot: TeamSnapshot {
        root_thread_id: "root-thread".to_string(),
        members: vec![
          TeamMember {
            thread_id: "root-thread".to_string(),
            nickname: None,
            role: "root".to_string(),
            task: "root session".to_string(),
            depth: 0,
            status: AgentStatus::Running,
          },
          TeamMember {
            thread_id: "ash-thread".to_string(),
            nickname: Some("艾许".to_string()),
            role: "default".to_string(),
            task: "你是团队成员“艾许”。请从架构角度继续深入分析。".to_string(),
            depth: 1,
            status: AgentStatus::Running,
          },
          TeamMember {
            thread_id: "sparrow-thread".to_string(),
            nickname: Some("六雀".to_string()),
            role: "default".to_string(),
            task: "你是团队成员“六雀”。请从测试工具链角度继续深入分析。".to_string(),
            depth: 1,
            status: AgentStatus::Completed(Some("Research summary completed".to_string())),
          },
        ],
        tasks: vec![
          TeamTask {
            id: "old-task".to_string(),
            title: "Project Exploration - Core".to_string(),
            details: Some("历史遗留任务".to_string()),
            status: TeamTaskStatus::Pending,
            assignee_thread_id: None,
            workflow_run_id: None,
            created_at: 1,
            updated_at: 1,
            notes: Vec::new(),
          },
          TeamTask {
            id: "new-task".to_string(),
            title: "当前团队讨论".to_string(),
            details: None,
            status: TeamTaskStatus::InProgress,
            assignee_thread_id: Some("ash-thread".to_string()),
            workflow_run_id: None,
            created_at: 2,
            updated_at: 2,
            notes: Vec::new(),
          },
        ],
        plans: vec![TeamPlan {
          id: "plan-1".to_string(),
          author_thread_id: "ash-thread".to_string(),
          summary: "继续分工探索".to_string(),
          steps: vec!["读取代码".to_string()],
          status: TeamPlanStatus::PendingApproval,
          requires_approval: true,
          reviewer_thread_id: None,
          review_note: None,
          workflow_run_id: None,
          created_at: 1,
          updated_at: 1,
        }],
        unread_counts: HashMap::from([
          ("root-thread".to_string(), 0),
          ("ash-thread".to_string(), 1),
          ("sparrow-thread".to_string(), 0),
        ]),
        workflow: None,
      },
    });

    let rendered = cell
      .lines
      .iter()
      .map(|line| {
        line
          .spans
          .iter()
          .map(|span| span.content.as_ref())
          .collect::<String>()
      })
      .collect::<Vec<_>>()
      .join("\n");

    assert!(rendered.contains("Owner @main • 2 teammates • 2 tasks • 1 plans"));
    assert!(rendered.contains("@艾许: Running · 1 unread"));
    assert!(rendered.contains("@六雀: Completed"));
    assert!(rendered.contains("Unread: @艾许 1 msg"));
    assert!(rendered.contains("1 plan(s) pending review"));
    assert!(rendered.contains("2 task(s) collapsed by default"));
    assert!(!rendered.contains("root session"));
    assert!(!rendered.contains("你是团队成员"));
    assert!(!rendered.contains("Project Exploration - Core"));
  }

  #[test]
  fn messages_read_prefers_agent_nickname() {
    let cell = messages_read(CollabMessagesReadEvent {
      reader_thread_id: "943efa71-aaaa-bbbb-cccc-111111111111".to_string(),
      reader_nickname: Some("main".to_string()),
      reader_role: Some("leader".to_string()),
      count: 1,
    });

    let rendered = cell
      .lines
      .iter()
      .map(|line| {
        line
          .spans
          .iter()
          .map(|span| span.content.as_ref())
          .collect::<String>()
      })
      .collect::<Vec<_>>()
      .join("\n");

    assert!(rendered.contains("@main read 1 team message(s)"));
    assert!(!rendered.contains("943efa71"));
  }

  #[test]
  fn message_posted_prefers_agent_labels() {
    let cell = message_posted(CollabMessagePostedEvent {
      sender_thread_id: "root-thread".to_string(),
      sender_nickname: Some("main".to_string()),
      sender_role: Some("leader".to_string()),
      recipient_thread_id: Some("sparrow-thread".to_string()),
      recipient_nickname: Some("六雀".to_string()),
      recipient_role: Some("default".to_string()),
      message: "请继续同步你的进度".to_string(),
    });

    let rendered = cell
      .lines
      .iter()
      .map(|line| {
        line
          .spans
          .iter()
          .map(|span| span.content.as_ref())
          .collect::<String>()
      })
      .collect::<Vec<_>>()
      .join("\n");

    assert!(rendered.contains("@main -> @六雀"));
    assert!(!rendered.contains("root-thread"));
    assert!(!rendered.contains("sparrow-thread"));
  }

  #[test]
  fn waiting_end_renders_member_statuses_as_tree() {
    let cell = waiting_end(CollabWaitingEndEvent {
      sender_thread_id: "root-thread".to_string(),
      call_id: "call-1".to_string(),
      statuses: HashMap::new(),
      agent_statuses: vec![
        CollabAgentStatusEntry {
          thread_id: "kasumi-thread".to_string(),
          nickname: Some("有村架纯".to_string()),
          role: Some("default".to_string()),
          status: AgentStatus::Completed(Some(
            "哎哎哎——菅田くん，等一下！让我来一条一条给你整理一下！".to_string(),
          )),
        },
        CollabAgentStatusEntry {
          thread_id: "masaki-thread".to_string(),
          nickname: Some("菅田将晖".to_string()),
          role: Some("default".to_string()),
          status: AgentStatus::Completed(Some(
            "哈，架纯你说的也不是没道理……但等等，我有话说。".to_string(),
          )),
        },
      ],
    });

    let rendered = HistoryCell::display_lines(&cell, 52)
      .iter()
      .map(|line| {
        line
          .spans
          .iter()
          .map(|span| span.content.as_ref())
          .collect::<String>()
      })
      .collect::<Vec<_>>()
      .join("\n");

    assert!(rendered.contains("● Finished waiting"));
    assert!(rendered.contains("├─ @有村架纯"));
    assert!(rendered.contains("│  ⎿ Completed"));
    assert!(rendered.contains("└─ @菅田将晖"));
    assert!(rendered.contains("⎿ Completed"));
  }
}

