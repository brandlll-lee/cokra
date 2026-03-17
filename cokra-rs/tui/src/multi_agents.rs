use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;

use cokra_protocol::CollabAgentInteractionEndEvent;
use cokra_protocol::CollabAgentLifecycle;
use cokra_protocol::CollabAgentRef;
use cokra_protocol::CollabAgentSpawnEndEvent;
use cokra_protocol::CollabAgentStatusEntry;
use cokra_protocol::CollabAgentWaitState;
use cokra_protocol::CollabCloseEndEvent;
use cokra_protocol::CollabMailboxDeliveredEvent;
use cokra_protocol::CollabMessagePostedEvent;
use cokra_protocol::CollabMessagesReadEvent;
use cokra_protocol::CollabPlanDecisionEvent;
use cokra_protocol::CollabPlanSubmittedEvent;
use cokra_protocol::CollabSummaryCheckpointEvent;
use cokra_protocol::CollabTaskUpdatedEvent;
use cokra_protocol::CollabTeamSnapshotEvent;
use cokra_protocol::CollabTurnOutcome;
use cokra_protocol::CollabWaitingBeginEvent;
use cokra_protocol::CollabWaitingEndEvent;
use cokra_protocol::OwnershipAccessMode;
use cokra_protocol::OwnershipLease;
use cokra_protocol::ScopeRequest;
use cokra_protocol::TeamMember;
use cokra_protocol::TeamMessage;
use cokra_protocol::TeamSnapshot;
use cokra_protocol::TeamTaskReadyState;
use cokra_protocol::TeamTaskReviewState;
use cokra_protocol::TeamTaskStatus;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CollabSummaryPhase {
  Working,
  Attention,
  Done,
}

#[derive(Debug, Clone)]
pub(crate) struct CollabSummarySnapshot {
  pub(crate) phase: CollabSummaryPhase,
  pub(crate) lines: Vec<Line<'static>>,
  pub(crate) plain_lines: Vec<String>,
  pub(crate) fingerprint: u64,
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

pub(crate) fn working_summary_lines(snapshot: &TeamSnapshot) -> Option<Vec<Line<'static>>> {
  working_summary_snapshot(snapshot).map(|summary| summary.lines)
}

pub(crate) fn working_summary_snapshot(snapshot: &TeamSnapshot) -> Option<CollabSummarySnapshot> {
  let members = snapshot
    .members
    .iter()
    .filter(|member| member.thread_id != snapshot.root_thread_id)
    .filter(|member| {
      !matches!(
        member.state.lifecycle.clone(),
        CollabAgentLifecycle::Shutdown | CollabAgentLifecycle::NotFound
      )
    })
    .collect::<Vec<_>>();
  if members.is_empty() {
    return None;
  }

  let has_active_members = members.iter().any(|member| {
    matches!(
      member.state.lifecycle.clone(),
      CollabAgentLifecycle::PendingInit | CollabAgentLifecycle::Busy
    )
  });
  let has_pending_wakes = members
    .iter()
    .any(|member| member.state.pending_wake_count > 0);
  let open_tasks = snapshot
    .tasks
    .iter()
    .filter(|task| {
      !matches!(
        task.status,
        TeamTaskStatus::Completed | TeamTaskStatus::Failed | TeamTaskStatus::Canceled
      ) || !matches!(
        task.ready_state,
        TeamTaskReadyState::Completed | TeamTaskReadyState::Failed | TeamTaskReadyState::Canceled
      )
    })
    .collect::<Vec<_>>();
  let has_errored_members = members
    .iter()
    .any(|member| matches!(member.state.lifecycle.clone(), CollabAgentLifecycle::Error));

  let phase = if has_errored_members {
    CollabSummaryPhase::Attention
  } else if !has_active_members && !has_pending_wakes && open_tasks.is_empty() {
    CollabSummaryPhase::Done
  } else {
    CollabSummaryPhase::Working
  };

  let mut lines = if matches!(phase, CollabSummaryPhase::Done) {
    vec![Line::from(vec![
      Span::from("✓ ").green(),
      Span::from("Agent Teams Done").green().bold(),
    ])]
  } else if matches!(phase, CollabSummaryPhase::Attention) {
    vec![Line::from(vec![
      Span::from("! ").yellow(),
      Span::from("Agent Teams Attention").yellow().bold(),
    ])]
  } else {
    vec![Line::from("Agent teams working...".bold())]
  };
  for (idx, member) in members.iter().enumerate() {
    let is_last = idx + 1 == members.len();
    let branch = if is_last { " └─ " } else { " ├─ " };
    let continuation = if is_last { "    " } else { " │  " };

    let mut member_line = vec![Span::from(branch).dim()];
    member_line.extend(agent_label_spans(AgentLabel {
      thread_id: &member.thread_id,
      nickname: member.nickname.as_deref(),
      role: Some(&member.role),
    }));
    lines.push(Line::from(member_line));

    lines.push(Line::from(vec![
      Span::from(format!("{continuation}⎿ ")).dim(),
      Span::from(member_activity_summary(snapshot, member)),
    ]));
  }

  if !snapshot.tasks.is_empty() {
    let in_progress = open_tasks
      .iter()
      .filter(|task| {
        matches!(task.status, TeamTaskStatus::InProgress)
          || matches!(task.ready_state, TeamTaskReadyState::Claimed)
      })
      .count();
    let review = open_tasks
      .iter()
      .filter(|task| {
        matches!(task.status, TeamTaskStatus::Review)
          || matches!(task.ready_state, TeamTaskReadyState::Review)
      })
      .count();
    let blocked = open_tasks
      .iter()
      .filter(|task| matches!(task.ready_state, TeamTaskReadyState::Blocked))
      .count();
    let ready = open_tasks
      .iter()
      .filter(|task| matches!(task.ready_state, TeamTaskReadyState::Ready))
      .count();

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
      Span::from("Tasks").bold(),
      Span::from(format!("  {open}/{total}", open = open_tasks.len(), total = snapshot.tasks.len())).dim(),
    ]));

    let open = open_tasks.len();
    let mut buckets: Vec<Span<'static>> = Vec::new();
    if in_progress > 0 {
      if !buckets.is_empty() {
        buckets.push(Span::from(" · ").dim());
      }
      buckets.push(Span::from(format!("{in_progress} working")).green());
    }
    if review > 0 {
      if !buckets.is_empty() {
        buckets.push(Span::from(" · ").dim());
      }
      buckets.push(Span::from(format!("{review} review")).yellow());
    }
    if blocked > 0 {
      if !buckets.is_empty() {
        buckets.push(Span::from(" · ").dim());
      }
      buckets.push(Span::from(format!("{blocked} blocked")).red());
    }
    if ready > 0 {
      if !buckets.is_empty() {
        buckets.push(Span::from(" · ").dim());
      }
      buckets.push(Span::from(format!("{ready} ready")).dim());
    }
    if !buckets.is_empty() {
      let mut summary_line = vec![Span::from(" ├─ ").dim()];
      summary_line.extend(buckets);
      lines.push(Line::from(summary_line));
    }

    if open > 0 {
      let label_for = |thread_id: &str| {
        if thread_id == snapshot.root_thread_id {
          return "@main".to_string();
        }
        let member = snapshot
          .members
          .iter()
          .find(|member| member.thread_id == thread_id);
        if let Some(member) = member {
          return agent_label_display(AgentLabel {
            thread_id,
            nickname: member.nickname.as_deref(),
            role: Some(member.role.as_str()),
          });
        }
        format!("@{}", short_thread_id(thread_id))
      };

      let mut sorted = open_tasks.clone();
      sorted.sort_by(|left, right| {
        fn rank(task: &cokra_protocol::TeamTask) -> u8 {
          if matches!(task.status, TeamTaskStatus::InProgress)
            || matches!(task.ready_state, TeamTaskReadyState::Claimed)
          {
            0
          } else if matches!(task.status, TeamTaskStatus::Review)
            || matches!(task.ready_state, TeamTaskReadyState::Review)
          {
            1
          } else if matches!(task.ready_state, TeamTaskReadyState::Blocked) {
            2
          } else if matches!(task.ready_state, TeamTaskReadyState::Ready) {
            3
          } else {
            4
          }
        }

        rank(left)
          .cmp(&rank(right))
          .then_with(|| right.updated_at.cmp(&left.updated_at))
          .then_with(|| left.id.cmp(&right.id))
      });

      const VISIBLE_TASKS: usize = 3;
      let visible = open.min(VISIBLE_TASKS);
      for (idx, task) in sorted.iter().take(visible).enumerate() {
        let is_last = idx + 1 == visible && open <= visible;
        let branch = if is_last { " └─ " } else { " ├─ " };
        let owner = task
          .owner_thread_id
          .as_deref()
          .map(&label_for)
          .unwrap_or_else(|| "unassigned".to_string());
        let status_label = task_status_label(&task.status, &task.ready_state);
        let short_id: String = task.id.chars().take(8).collect();
        let title = normalize_preview(&task.title, TASK_PREVIEW_CHARS);

        lines.push(Line::from(vec![
          Span::from(branch).dim(),
          task_status_span(&status_label),
          Span::from(format!(" #{short_id}")).dim(),
          Span::from(format!(" {owner}")).dim(),
          Span::from(format!(" {title}")),
        ]));
      }
      if open > visible {
        lines.push(Line::from(vec![
          Span::from(" └─ ").dim(),
          Span::from(format!("+{} more", open - visible)).dim(),
          Span::from("  (/collab)").dim(),
        ]));
      }
    }
  }

  // Latest mailbox message preview — surfaces key inter-agent communication
  // in the live summary so compact mode doesn't lose critical context.
  if let Some(msg) = snapshot.recent_messages.last() {
    let sender_label = snapshot
      .members
      .iter()
      .find(|m| m.thread_id == msg.sender_thread_id)
      .and_then(|m| m.nickname.as_deref())
      .map(|n| format!("@{n}"))
      .unwrap_or_else(|| {
        if msg.sender_thread_id == snapshot.root_thread_id {
          "@main".to_string()
        } else {
          format!("@{}", short_thread_id(&msg.sender_thread_id))
        }
      });
    let recipient_label = msg
      .recipient_thread_id
      .as_deref()
      .map(|tid| {
        if tid == snapshot.root_thread_id {
          "@main".to_string()
        } else {
          snapshot
            .members
            .iter()
            .find(|m| m.thread_id == tid)
            .and_then(|m| m.nickname.as_deref())
            .map(|n| format!("@{n}"))
            .unwrap_or_else(|| format!("@{}", short_thread_id(tid)))
        }
      })
      .unwrap_or_else(|| "@team".to_string());
    let preview = normalize_preview(&msg.message, 50);
    lines.push(Line::from(vec![
      Span::from(" ⎿ ").dim(),
      Span::from(format!("{sender_label} → {recipient_label}: ")).dim(),
      Span::from(preview).dim(),
    ]));
  }

  let plain_lines = lines_to_plain_strings(&lines);
  let mut hasher = DefaultHasher::new();
  phase.hash(&mut hasher);
  plain_lines.hash(&mut hasher);

  Some(CollabSummarySnapshot {
    phase,
    lines,
    plain_lines,
    fingerprint: hasher.finish(),
  })
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

  let state = CollabAgentWaitState {
    lifecycle: ev.lifecycle,
    turn_outcome: ev.turn_outcome,
    last_turn_summary: ev.last_turn_summary,
    attention_reason: ev.attention_reason,
    pending_wake_count: ev.pending_wake_count,
  };
  let mut details = vec![status_summary_line(&state)];
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
  details.push(status_summary_line(&CollabAgentWaitState {
    lifecycle: ev.lifecycle,
    turn_outcome: ev.turn_outcome,
    last_turn_summary: ev.last_turn_summary,
    attention_reason: ev.attention_reason,
    pending_wake_count: ev.pending_wake_count,
  }));
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
  collab_event(
    title,
    vec![status_summary_line(&CollabAgentWaitState {
      lifecycle: ev.lifecycle,
      ..Default::default()
    })],
  )
}

pub(crate) fn mailbox_delivered(
  ev: CollabMailboxDeliveredEvent,
) -> crate::history_cell::PeerMailboxHistoryCell {
  let sender_label = if ev
    .sender_role
    .as_deref()
    .is_some_and(|role| role.eq_ignore_ascii_case("root"))
  {
    "@main".to_string()
  } else if let Some(nickname) = ev
    .sender_nickname
    .as_deref()
    .map(str::trim)
    .filter(|value| !value.is_empty())
  {
    format!("@{nickname}")
  } else {
    format!("@{}", short_thread_id(&ev.sender_thread_id))
  };
  crate::history_cell::PeerMailboxHistoryCell::new(sender_label, ev.sender_thread_id, ev.message)
}

pub(crate) fn summary_checkpoint(
  ev: CollabSummaryCheckpointEvent,
) -> crate::history_cell::CollabSummaryHistoryCell {
  crate::history_cell::CollabSummaryHistoryCell::from_plain_lines(ev.lines)
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

#[derive(Debug, Clone)]
pub(crate) struct TeamDashboardSections {
  pub(crate) folded_header: Line<'static>,
  pub(crate) summary: Vec<Line<'static>>,
  pub(crate) tasks: Vec<Line<'static>>,
  pub(crate) mailbox: Vec<Line<'static>>,
  pub(crate) locks: Vec<Line<'static>>,
}

pub(crate) fn team_dashboard_sections(snapshot: &TeamSnapshot) -> TeamDashboardSections {
  let labels = snapshot_label_lookup(snapshot);
  let teammate_count = snapshot.members.len().saturating_sub(1);
  let unread_members = snapshot
    .members
    .iter()
    .filter(|member| member.thread_id != snapshot.root_thread_id)
    .filter_map(|member| {
      let unread = snapshot
        .unread_counts
        .get(&member.thread_id)
        .copied()
        .unwrap_or(0);
      (unread > 0).then_some((member, unread))
    })
    .collect::<Vec<_>>();
  let pending_plan_count = snapshot
    .plans
    .iter()
    .filter(|plan| {
      matches!(
        plan.status,
        cokra_protocol::TeamPlanStatus::Draft | cokra_protocol::TeamPlanStatus::PendingApproval
      )
    })
    .count();

  let (active_tasks, backlog_tasks, closed_tasks) = snapshot.tasks.iter().fold(
    (0usize, 0usize, 0usize),
    |(active, backlog, closed), task| {
      if matches!(
        task.status,
        TeamTaskStatus::Completed | TeamTaskStatus::Failed | TeamTaskStatus::Canceled
      ) || matches!(
        task.ready_state,
        TeamTaskReadyState::Completed | TeamTaskReadyState::Failed | TeamTaskReadyState::Canceled
      ) {
        (active, backlog, closed + 1)
      } else if matches!(
        task.status,
        TeamTaskStatus::InProgress | TeamTaskStatus::Review
      ) || matches!(
        task.ready_state,
        TeamTaskReadyState::Claimed | TeamTaskReadyState::Review | TeamTaskReadyState::Blocked
      ) {
        (active + 1, backlog, closed)
      } else {
        (active, backlog + 1, closed)
      }
    },
  );
  let unread_total = snapshot.unread_counts.values().copied().sum::<usize>();
  let folded_header = Line::from(format!(
    "Active {active_tasks} | Backlog {backlog_tasks} | Closed {closed_tasks} | Locks {} | Unread {unread_total}",
    snapshot.ownership_leases.len()
  ));

  let mut summary = vec![
    Line::from(format!(
      "Owner @main • {} teammates • {} tasks • {} plans",
      teammate_count,
      snapshot.tasks.len(),
      snapshot.plans.len(),
    )),
    Line::from(format!(
      "Live locks: {} | Recent mailbox items: {}",
      snapshot.ownership_leases.len(),
      snapshot.recent_messages.len()
    )),
  ];

  for member in &snapshot.members {
    if member.thread_id == snapshot.root_thread_id {
      continue;
    }
    summary.push(member_dashboard_line(snapshot, member));
  }

  if teammate_count == 0 {
    summary.push(Line::from("No teammates yet".dim()));
  }

  if !unread_members.is_empty() {
    summary.push(unread_summary_line(
      &snapshot.root_thread_id,
      &unread_members,
    ));
  }

  if pending_plan_count > 0 {
    summary.push(Line::from(format!(
      "{pending_plan_count} plan(s) pending review"
    )));
  }

  if let Some(run_snapshot) = &snapshot.workflow {
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
      summary.push(Line::from(format!(
        "{} resumable run(s), {} currently open",
        run_snapshot.runs.len(),
        open_runs
      )));
    }
  }

  let mut mailbox = Vec::new();
  if !snapshot.recent_messages.is_empty() {
    mailbox.push(Line::from("Mailbox flows".bold()));
    let visible = snapshot.recent_messages.len().min(5);
    for message in snapshot.recent_messages.iter().take(visible) {
      mailbox.push(snapshot_message_line_with_labels(message, &labels));
    }
    if snapshot.recent_messages.len() > visible {
      mailbox.push(Line::from(format!(
        "+{} older mailbox item(s)",
        snapshot.recent_messages.len() - visible
      )));
    }
  }

  let mut tasks = Vec::new();
  if !snapshot.tasks.is_empty() {
    tasks.push(Line::from("Task graph".bold()));
    for task in &snapshot.tasks {
      tasks.push(snapshot_task_line(task, &labels));
      if let Some(blocker_line) = snapshot_task_blocker_line(task, &labels) {
        tasks.push(blocker_line);
      }
    }
  }

  let mut locks = Vec::new();
  if !snapshot.ownership_leases.is_empty() {
    locks.push(Line::from("Active locks".bold()));
    for lease in &snapshot.ownership_leases {
      locks.push(snapshot_lease_line(lease, &labels));
    }
  }

  TeamDashboardSections {
    folded_header,
    summary,
    tasks,
    mailbox,
    locks,
  }
}

pub(crate) fn team_snapshot(ev: CollabTeamSnapshotEvent) -> PlainHistoryCell {
  let sections = team_dashboard_sections(&ev.snapshot);
  let mut details = Vec::new();
  details.push(sections.folded_header.clone());
  details.push(Line::from(
    "(use /collab to open Team Panel: Summary / Tasks / Mailbox / Locks)".dim(),
  ));
  details.push(Line::from(""));
  details.extend(sections.summary);
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

fn snapshot_message_line_with_labels(
  message: &TeamMessage,
  labels: &HashMap<String, String>,
) -> Line<'static> {
  let destination = message
    .recipient_thread_id
    .as_deref()
    .map(|thread_id| {
      labels
        .get(thread_id)
        .cloned()
        .unwrap_or_else(|| format!("{} ({thread_id})", short_thread_id(thread_id)))
    })
    .or_else(|| message.route_key.clone())
    .unwrap_or_else(|| "team".to_string());
  Line::from(format!(
    "{} -> {} | {:?} {:?} | {}",
    labels
      .get(&message.sender_thread_id)
      .cloned()
      .unwrap_or_else(|| short_thread_id(&message.sender_thread_id)),
    destination,
    message.delivery_mode,
    message.kind,
    normalize_preview(&message.message, TASK_PREVIEW_CHARS)
  ))
}

fn snapshot_task_line(
  task: &cokra_protocol::TeamTask,
  labels: &HashMap<String, String>,
) -> Line<'static> {
  let status_label = task_status_label(&task.status, &task.ready_state);
  let short_id: String = task.id.chars().take(8).collect();
  let owner = task
    .owner_thread_id
    .as_deref()
    .and_then(|thread_id| labels.get(thread_id).cloned())
    .unwrap_or_else(|| "unassigned".to_string());
  let title = normalize_preview(&task.title, TASK_PREVIEW_CHARS);

  Line::from(vec![
    task_status_span(&status_label),
    Span::from(format!(" #{short_id}")).dim(),
    Span::from(format!(" {owner}")).dim(),
    Span::from(format!(" {title}")),
  ])
}

fn snapshot_task_blocker_line(
  task: &cokra_protocol::TeamTask,
  labels: &HashMap<String, String>,
) -> Option<Line<'static>> {
  if matches!(
    task.status,
    TeamTaskStatus::Completed | TeamTaskStatus::Failed | TeamTaskStatus::Canceled
  ) {
    return None;
  }
  let blockers = task
    .blockers
    .iter()
    .filter(|blocker| blocker.active)
    .map(|blocker| {
      if let Some(blocking_task_id) = blocker.blocking_task_id.as_deref() {
        for (thread_id, label) in labels {
          if blocker.reason.contains(thread_id) {
            return blocker.reason.replace(thread_id, label);
          }
        }
        return blocker
          .reason
          .replace(blocking_task_id, &format!("#{blocking_task_id}"));
      }
      let mut reason = blocker.reason.clone();
      for (thread_id, label) in labels {
        if reason.contains(thread_id) {
          reason = reason.replace(thread_id, label);
        }
      }
      reason
    })
    .collect::<Vec<_>>();
  (!blockers.is_empty()).then(|| {
    Line::from(format!(
      "blocked: {}",
      normalize_preview(&blockers.join(" | "), STATUS_PREVIEW_CHARS)
    ))
  })
}

fn snapshot_lease_line(lease: &OwnershipLease, labels: &HashMap<String, String>) -> Line<'static> {
  Line::from(format!(
    "{} | {:?} {:?} | {} | task={}",
    labels
      .get(&lease.owner_thread_id)
      .cloned()
      .unwrap_or_else(|| short_thread_id(&lease.owner_thread_id)),
    lease.access,
    lease.scope.kind,
    lease.scope.path,
    lease.task_id
  ))
}

fn member_activity_summary(snapshot: &TeamSnapshot, member: &TeamMember) -> String {
  if let Some(reason) = member
    .state
    .attention_reason
    .as_deref()
    .map(|reason| normalize_preview(reason, STATUS_PREVIEW_CHARS))
    .filter(|reason| !reason.is_empty())
  {
    return format!("attention: {reason}");
  }

  if matches!(member.state.lifecycle.clone(), CollabAgentLifecycle::Busy)
    && let Some(task) = active_member_task(snapshot, &member.thread_id)
  {
    return format!("working {}", task_subject(task));
  }

  if let Some(task) = active_member_task(snapshot, &member.thread_id) {
    if let Some(reason) = active_blocker_reason(task) {
      return format!("blocked: {reason}");
    }

    if matches!(task.status, TeamTaskStatus::Review)
      || matches!(task.ready_state, TeamTaskReadyState::Review)
      || matches!(task.review_state, TeamTaskReviewState::Requested)
    {
      return format!("reviewing {}", task_subject(task));
    }
    if member.state.pending_wake_count > 0 {
      return format!("queued for {}", task_subject(task));
    }
  }

  if matches!(member.state.lifecycle.clone(), CollabAgentLifecycle::Error) {
    return "attention needed".to_string();
  }

  if member.state.pending_wake_count > 0 {
    return format!("queued {} follow-up(s)", member.state.pending_wake_count);
  }

  let unread = snapshot
    .unread_counts
    .get(&member.thread_id)
    .copied()
    .unwrap_or(0);
  if unread > 0 {
    return format!("idle ({unread} unread)");
  }

  match member.state.lifecycle.clone() {
    CollabAgentLifecycle::PendingInit => "starting up".to_string(),
    CollabAgentLifecycle::Ready => "idle".to_string(),
    CollabAgentLifecycle::Busy => "working".to_string(),
    CollabAgentLifecycle::Error => "attention needed".to_string(),
    CollabAgentLifecycle::Shutdown => "closed".to_string(),
    CollabAgentLifecycle::NotFound => "unavailable".to_string(),
  }
}

fn active_member_task<'a>(
  snapshot: &'a TeamSnapshot,
  member_thread_id: &str,
) -> Option<&'a cokra_protocol::TeamTask> {
  snapshot
    .tasks
    .iter()
    .filter(|task| {
      task.owner_thread_id.as_deref() == Some(member_thread_id)
        || task.assignee_thread_id.as_deref() == Some(member_thread_id)
        || task.reviewer_thread_id.as_deref() == Some(member_thread_id)
    })
    .max_by_key(|task| (task_activity_rank(task), task.updated_at))
}

fn task_activity_rank(task: &cokra_protocol::TeamTask) -> i32 {
  match (&task.status, &task.ready_state) {
    (TeamTaskStatus::Completed, _) | (_, TeamTaskReadyState::Completed) => 10,
    (TeamTaskStatus::Failed, _) | (_, TeamTaskReadyState::Failed) => 5,
    (TeamTaskStatus::Canceled, _) | (_, TeamTaskReadyState::Canceled) => 0,
    (TeamTaskStatus::Review, _) | (_, TeamTaskReadyState::Review) => 60,
    (TeamTaskStatus::InProgress, TeamTaskReadyState::Claimed) => 50,
    (TeamTaskStatus::InProgress, _) => 45,
    (_, TeamTaskReadyState::Blocked) => 40,
    (TeamTaskStatus::Pending, TeamTaskReadyState::Ready) => 35,
    (TeamTaskStatus::Pending, _) => 30,
  }
}

fn active_blocker_reason(task: &cokra_protocol::TeamTask) -> Option<String> {
  task
    .blockers
    .iter()
    .find(|blocker| blocker.active)
    .map(|blocker| normalize_preview(&blocker.reason, STATUS_PREVIEW_CHARS))
    .or_else(|| {
      task
        .blocking_reason
        .as_deref()
        .map(|reason| normalize_preview(reason, STATUS_PREVIEW_CHARS))
    })
}

fn task_activity_verb(task: &cokra_protocol::TeamTask) -> &'static str {
  if matches!(task.status, TeamTaskStatus::Completed)
    || matches!(task.ready_state, TeamTaskReadyState::Completed)
  {
    return "completed";
  }
  if matches!(task.status, TeamTaskStatus::Failed)
    || matches!(task.ready_state, TeamTaskReadyState::Failed)
  {
    return "failed";
  }
  if matches!(task.status, TeamTaskStatus::Canceled)
    || matches!(task.ready_state, TeamTaskReadyState::Canceled)
  {
    return "canceled";
  }

  if matches!(task.status, TeamTaskStatus::Review)
    || matches!(task.ready_state, TeamTaskReadyState::Review)
    || matches!(task.review_state, TeamTaskReviewState::Requested)
  {
    return "reviewing";
  }

  match primary_scope_access(task) {
    Some(OwnershipAccessMode::SharedRead) => "reading",
    Some(OwnershipAccessMode::Review) => "reviewing",
    Some(OwnershipAccessMode::ExclusiveWrite) | None => match task.ready_state {
      TeamTaskReadyState::Ready => "ready to start",
      TeamTaskReadyState::Blocked => "blocked on",
      TeamTaskReadyState::Claimed => "implementing",
      TeamTaskReadyState::Review => "reviewing",
      TeamTaskReadyState::Completed => "completed",
      TeamTaskReadyState::Failed => "failed",
      TeamTaskReadyState::Canceled => "canceled",
    },
  }
}

fn task_subject(task: &cokra_protocol::TeamTask) -> String {
  primary_scope_request(task)
    .map(scope_subject)
    .filter(|subject| !subject.is_empty())
    .unwrap_or_else(|| normalize_preview(&task.title, TASK_PREVIEW_CHARS))
}

fn primary_scope_access(task: &cokra_protocol::TeamTask) -> Option<OwnershipAccessMode> {
  primary_scope_request(task).map(|scope| scope.access.clone())
}

fn primary_scope_request(task: &cokra_protocol::TeamTask) -> Option<&ScopeRequest> {
  task
    .granted_scopes
    .first()
    .or_else(|| task.requested_scopes.first())
}

fn scope_subject(scope: &ScopeRequest) -> String {
  match scope.kind {
    cokra_protocol::OwnershipScopeKind::Glob | cokra_protocol::OwnershipScopeKind::Module => {
      normalize_preview(&scope.path, TASK_PREVIEW_CHARS)
    }
    cokra_protocol::OwnershipScopeKind::File | cokra_protocol::OwnershipScopeKind::Directory => {
      path_leaf(&scope.path)
    }
  }
}

fn path_leaf(path: &str) -> String {
  path
    .rsplit(['/', '\\'])
    .find(|segment| !segment.trim().is_empty())
    .map(ToString::to_string)
    .unwrap_or_else(|| normalize_preview(path, TASK_PREVIEW_CHARS))
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
  statuses: &std::collections::HashMap<String, CollabAgentWaitState>,
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
        state: status.clone(),
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
      summary: status_summary_line(&entry.state),
    })
    .collect()
}

fn status_summary_line(state: &CollabAgentWaitState) -> Line<'static> {
  Line::from(status_summary_spans(state))
}

fn status_summary_spans(state: &CollabAgentWaitState) -> Vec<Span<'static>> {
  let mut spans = match state.lifecycle.clone() {
    CollabAgentLifecycle::PendingInit => vec![Span::from("Pending init").style(light_blue())],
    CollabAgentLifecycle::Ready => vec![Span::from("Idle").green()],
    CollabAgentLifecycle::Busy => vec![Span::from("Busy").yellow().bold()],
    CollabAgentLifecycle::Error => vec![Span::from("Errored").red()],
    CollabAgentLifecycle::Shutdown => vec![Span::from("Closed").dim()],
    CollabAgentLifecycle::NotFound => vec![Span::from("Not found").red()],
  };
  let outcome = match state.turn_outcome.clone() {
    CollabTurnOutcome::NoneYet => None,
    CollabTurnOutcome::Succeeded => Some("last turn ok"),
    CollabTurnOutcome::Errored => Some("last turn failed"),
    CollabTurnOutcome::Interrupted => Some("last turn interrupted"),
  };
  if let Some(outcome) = outcome {
    spans.push(Span::from(" · ").dim());
    spans.push(Span::from(outcome).dim());
  }
  if let Some(summary) = state
    .last_turn_summary
    .as_deref()
    .map(|value| normalize_preview(value, STATUS_PREVIEW_CHARS))
    .filter(|value| !value.is_empty())
  {
    spans.push(Span::from(" · ").dim());
    spans.push(Span::from(summary));
  }
  if state.pending_wake_count > 0 {
    spans.push(Span::from(" · ").dim());
    spans.push(Span::from(format!("{} queued", state.pending_wake_count)));
  }
  spans
}

fn member_dashboard_line(snapshot: &TeamSnapshot, member: &TeamMember) -> Line<'static> {
  let mut spans = team_member_label_spans(member, &snapshot.root_thread_id);
  spans.push(Span::from(": ").dim());

  spans.push(match member.state.lifecycle.clone() {
    CollabAgentLifecycle::PendingInit => Span::from("Pending init").style(light_blue()),
    CollabAgentLifecycle::Ready => Span::from("Idle").green(),
    CollabAgentLifecycle::Busy => Span::from("Busy").yellow().bold(),
    CollabAgentLifecycle::Error => Span::from("Errored").red(),
    CollabAgentLifecycle::Shutdown => Span::from("Closed").dim(),
    CollabAgentLifecycle::NotFound => Span::from("Not found").red(),
  });

  let activity = member_activity_summary(snapshot, member);
  let fallback = match member.state.lifecycle.clone() {
    CollabAgentLifecycle::PendingInit => "starting up",
    CollabAgentLifecycle::Ready => "idle",
    CollabAgentLifecycle::Busy => "working",
    CollabAgentLifecycle::Error => "attention needed",
    CollabAgentLifecycle::Shutdown => "closed",
    CollabAgentLifecycle::NotFound => "unavailable",
  };
  if activity != fallback {
    spans.push(Span::from(" · ").dim());
    spans.push(Span::from(activity));
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

fn task_status_label(
  status: &TeamTaskStatus,
  ready_state: &TeamTaskReadyState,
) -> String {
  if matches!(status, TeamTaskStatus::InProgress) || matches!(ready_state, TeamTaskReadyState::Claimed) {
    "Working".to_string()
  } else if matches!(status, TeamTaskStatus::Review) || matches!(ready_state, TeamTaskReadyState::Review) {
    "Review".to_string()
  } else if matches!(ready_state, TeamTaskReadyState::Blocked) {
    "Blocked".to_string()
  } else if matches!(ready_state, TeamTaskReadyState::Ready) {
    "Ready".to_string()
  } else if matches!(status, TeamTaskStatus::Completed) || matches!(ready_state, TeamTaskReadyState::Completed) {
    "Done".to_string()
  } else if matches!(status, TeamTaskStatus::Failed) || matches!(ready_state, TeamTaskReadyState::Failed) {
    "Failed".to_string()
  } else if matches!(status, TeamTaskStatus::Canceled) || matches!(ready_state, TeamTaskReadyState::Canceled) {
    "Canceled".to_string()
  } else {
    "Pending".to_string()
  }
}

fn task_status_span(label: &str) -> Span<'static> {
  match label {
    "Working" => Span::from("Working").green().bold(),
    "Review" => Span::from("Review").yellow().bold(),
    "Blocked" => Span::from("Blocked").red().bold(),
    "Ready" => Span::from("Ready").dim(),
    "Done" => Span::from("Done").green(),
    "Failed" => Span::from("Failed").red(),
    "Canceled" => Span::from("Canceled").dim(),
    _ => Span::from("Pending").dim(),
  }
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

fn lines_to_plain_strings(lines: &[Line<'static>]) -> Vec<String> {
  lines
    .iter()
    .map(|line| {
      line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
    })
    .collect()
}

fn short_thread_id(thread_id: &str) -> String {
  let short = thread_id.chars().take(8).collect::<String>();
  if short.is_empty() {
    "agent".to_string()
  } else {
    short
  }
}

fn snapshot_label_lookup(snapshot: &TeamSnapshot) -> HashMap<String, String> {
  let mut labels = HashMap::new();
  labels.insert(
    snapshot.root_thread_id.clone(),
    "@main [root]".to_string() + &format!(" ({})", short_thread_id(&snapshot.root_thread_id)),
  );
  for member in &snapshot.members {
    if member.thread_id == snapshot.root_thread_id {
      continue;
    }
    labels.insert(
      member.thread_id.clone(),
      snapshot_thread_display_label(
        &member.thread_id,
        member.nickname.as_deref(),
        Some(member.role.as_str()),
      ),
    );
  }
  labels
}

fn snapshot_thread_display_label(
  thread_id: &str,
  nickname: Option<&str>,
  role: Option<&str>,
) -> String {
  snapshot_thread_display_label_inline(thread_id, nickname, role, None)
}

fn snapshot_thread_display_label_inline(
  thread_id: &str,
  nickname: Option<&str>,
  role: Option<&str>,
  recipient_thread_id: Option<&str>,
) -> String {
  if role.is_some_and(|value| value.eq_ignore_ascii_case("root")) {
    return format!("@main [root] ({})", short_thread_id(thread_id));
  }
  let name = nickname
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(|value| format!("@{value}"))
    .unwrap_or_else(|| {
      if recipient_thread_id.is_some_and(|recipient| recipient == thread_id) {
        "@teammate".to_string()
      } else {
        "@agent".to_string()
      }
    });
  let role = role
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(|value| {
      if value.eq_ignore_ascii_case("default") {
        "general"
      } else {
        value
      }
    })
    .unwrap_or("general");
  format!("{name} [{role}] ({})", short_thread_id(thread_id))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::history_cell::HistoryCell;
  use std::collections::HashMap;

  use cokra_protocol::CollabAgentLifecycle;
  use cokra_protocol::CollabAgentStatusEntry;
  use cokra_protocol::CollabAgentWaitState;
  use cokra_protocol::CollabMessagePostedEvent;
  use cokra_protocol::CollabMessagesReadEvent;
  use cokra_protocol::CollabTeamSnapshotEvent;
  use cokra_protocol::CollabTurnOutcome;
  use cokra_protocol::CollabWaitingEndEvent;
  use cokra_protocol::TeamPlan;
  use cokra_protocol::TeamPlanStatus;
  use cokra_protocol::TeamSnapshot;
  use cokra_protocol::TeamTask;
  use cokra_protocol::TeamTaskStatus;

  fn member_state(
    lifecycle: CollabAgentLifecycle,
    turn_outcome: CollabTurnOutcome,
    last_turn_summary: Option<&str>,
  ) -> CollabAgentWaitState {
    CollabAgentWaitState {
      lifecycle,
      turn_outcome,
      last_turn_summary: last_turn_summary.map(ToString::to_string),
      attention_reason: None,
      pending_wake_count: 0,
    }
  }

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
  fn working_summary_lines_switches_to_done_when_team_is_idle() {
    let snapshot = TeamSnapshot {
      root_thread_id: "root-thread".to_string(),
      members: vec![
        TeamMember {
          thread_id: "root-thread".to_string(),
          nickname: None,
          role: "root".to_string(),
          task: "root session".to_string(),
          depth: 0,
          state: member_state(CollabAgentLifecycle::Busy, CollabTurnOutcome::NoneYet, None),
        },
        TeamMember {
          thread_id: "alpha-thread".to_string(),
          nickname: Some("alpha".to_string()),
          role: "codex".to_string(),
          task: "implement a".to_string(),
          depth: 1,
          state: member_state(
            CollabAgentLifecycle::Ready,
            CollabTurnOutcome::Succeeded,
            None,
          ),
        },
        TeamMember {
          thread_id: "beta-thread".to_string(),
          nickname: Some("beta".to_string()),
          role: "codex".to_string(),
          task: "implement b".to_string(),
          depth: 1,
          state: member_state(
            CollabAgentLifecycle::Ready,
            CollabTurnOutcome::Succeeded,
            None,
          ),
        },
      ],
      tasks: vec![
        TeamTask {
          id: "task-a".to_string(),
          title: "implement a.txt".to_string(),
          details: None,
          status: TeamTaskStatus::Completed,
          ready_state: TeamTaskReadyState::Completed,
          review_state: TeamTaskReviewState::NotRequested,
          owner_thread_id: Some("alpha-thread".to_string()),
          blocked_by_task_ids: Vec::new(),
          blocks_task_ids: Vec::new(),
          blocking_reason: None,
          blockers: Vec::new(),
          requested_scopes: vec![ScopeRequest {
            kind: cokra_protocol::OwnershipScopeKind::File,
            path: "a.txt".to_string(),
            access: OwnershipAccessMode::ExclusiveWrite,
            reason: None,
          }],
          granted_scopes: Vec::new(),
          scope_policy_override: false,
          assignee_thread_id: Some("alpha-thread".to_string()),
          reviewer_thread_id: None,
          workflow_run_id: None,
          created_at: 1,
          updated_at: 2,
          notes: Vec::new(),
        },
        TeamTask {
          id: "task-b".to_string(),
          title: "implement b.txt".to_string(),
          details: None,
          status: TeamTaskStatus::Completed,
          ready_state: TeamTaskReadyState::Completed,
          review_state: TeamTaskReviewState::NotRequested,
          owner_thread_id: Some("beta-thread".to_string()),
          blocked_by_task_ids: Vec::new(),
          blocks_task_ids: Vec::new(),
          blocking_reason: None,
          blockers: Vec::new(),
          requested_scopes: vec![ScopeRequest {
            kind: cokra_protocol::OwnershipScopeKind::File,
            path: "b.txt".to_string(),
            access: OwnershipAccessMode::ExclusiveWrite,
            reason: None,
          }],
          granted_scopes: Vec::new(),
          scope_policy_override: false,
          assignee_thread_id: Some("beta-thread".to_string()),
          reviewer_thread_id: None,
          workflow_run_id: None,
          created_at: 1,
          updated_at: 2,
          notes: Vec::new(),
        },
      ],
      task_edges: Vec::new(),
      plans: Vec::new(),
      unread_counts: HashMap::new(),
      mailbox_version: 0,
      recent_messages: Vec::new(),
      ownership_leases: Vec::new(),
      workflow: None,
    };

    let lines = working_summary_lines(&snapshot).expect("summary");
    let rendered = lines
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

    assert!(rendered.contains("Agent Teams Done"));
    assert!(rendered.contains("idle"));
    assert!(!rendered.contains("Agent teams working..."));
  }

  #[test]
  fn working_summary_lines_stays_working_when_open_tasks_exist() {
    let snapshot = TeamSnapshot {
      root_thread_id: "root-thread".to_string(),
      members: vec![
        TeamMember {
          thread_id: "root-thread".to_string(),
          nickname: None,
          role: "root".to_string(),
          task: "root session".to_string(),
          depth: 0,
          state: member_state(CollabAgentLifecycle::Busy, CollabTurnOutcome::NoneYet, None),
        },
        TeamMember {
          thread_id: "alpha-thread".to_string(),
          nickname: Some("alpha".to_string()),
          role: "codex".to_string(),
          task: "waiting".to_string(),
          depth: 1,
          state: member_state(
            CollabAgentLifecycle::Ready,
            CollabTurnOutcome::Succeeded,
            None,
          ),
        },
      ],
      tasks: vec![TeamTask {
        id: "task-a".to_string(),
        title: "implement a.txt".to_string(),
        details: None,
        status: TeamTaskStatus::InProgress,
        ready_state: TeamTaskReadyState::Claimed,
        review_state: TeamTaskReviewState::NotRequested,
        owner_thread_id: Some("alpha-thread".to_string()),
        blocked_by_task_ids: Vec::new(),
        blocks_task_ids: Vec::new(),
        blocking_reason: None,
        blockers: Vec::new(),
        requested_scopes: vec![ScopeRequest {
          kind: cokra_protocol::OwnershipScopeKind::File,
          path: "a.txt".to_string(),
          access: OwnershipAccessMode::ExclusiveWrite,
          reason: None,
        }],
        granted_scopes: Vec::new(),
        scope_policy_override: false,
        assignee_thread_id: Some("alpha-thread".to_string()),
        reviewer_thread_id: None,
        workflow_run_id: None,
        created_at: 1,
        updated_at: 2,
        notes: Vec::new(),
      }],
      task_edges: Vec::new(),
      plans: Vec::new(),
      unread_counts: HashMap::new(),
      mailbox_version: 0,
      recent_messages: Vec::new(),
      ownership_leases: Vec::new(),
      workflow: None,
    };

    let lines = working_summary_lines(&snapshot).expect("summary");
    let rendered = lines
      .iter()
      .next()
      .map(|line| {
        line
          .spans
          .iter()
          .map(|span| span.content.as_ref())
          .collect::<String>()
      })
      .unwrap_or_default();

    assert!(rendered.contains("Agent teams working..."));
    assert!(!rendered.contains("Agent Teams Done"));
  }

  #[test]
  fn working_summary_lines_stays_working_when_only_pending_tasks_remain() {
    let snapshot = TeamSnapshot {
      root_thread_id: "root-thread".to_string(),
      members: vec![
        TeamMember {
          thread_id: "root-thread".to_string(),
          nickname: None,
          role: "root".to_string(),
          task: "root session".to_string(),
          depth: 0,
          state: member_state(CollabAgentLifecycle::Busy, CollabTurnOutcome::NoneYet, None),
        },
        TeamMember {
          thread_id: "alpha-thread".to_string(),
          nickname: Some("alpha".to_string()),
          role: "codex".to_string(),
          task: "waiting".to_string(),
          depth: 1,
          state: member_state(
            CollabAgentLifecycle::Ready,
            CollabTurnOutcome::Succeeded,
            None,
          ),
        },
      ],
      tasks: vec![TeamTask {
        id: "task-a".to_string(),
        title: "follow-up backlog".to_string(),
        details: None,
        status: TeamTaskStatus::Pending,
        ready_state: TeamTaskReadyState::Ready,
        review_state: TeamTaskReviewState::NotRequested,
        owner_thread_id: None,
        blocked_by_task_ids: Vec::new(),
        blocks_task_ids: Vec::new(),
        blocking_reason: None,
        blockers: Vec::new(),
        requested_scopes: Vec::new(),
        granted_scopes: Vec::new(),
        scope_policy_override: false,
        assignee_thread_id: None,
        reviewer_thread_id: None,
        workflow_run_id: None,
        created_at: 1,
        updated_at: 1,
        notes: Vec::new(),
      }],
      task_edges: Vec::new(),
      plans: Vec::new(),
      unread_counts: HashMap::new(),
      mailbox_version: 0,
      recent_messages: Vec::new(),
      ownership_leases: Vec::new(),
      workflow: None,
    };

    let lines = working_summary_lines(&snapshot).expect("summary");
    let rendered = lines
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

    assert!(rendered.contains("Agent teams working..."));
    assert!(rendered.contains("Tasks"));
    assert!(rendered.contains("1/1"));
  }

  #[test]
  fn working_summary_lines_stays_working_when_pending_wake_exists() {
    let mut snapshot = TeamSnapshot {
      root_thread_id: "root-thread".to_string(),
      members: vec![
        TeamMember {
          thread_id: "root-thread".to_string(),
          nickname: None,
          role: "root".to_string(),
          task: "root session".to_string(),
          depth: 0,
          state: member_state(CollabAgentLifecycle::Busy, CollabTurnOutcome::NoneYet, None),
        },
        TeamMember {
          thread_id: "alpha-thread".to_string(),
          nickname: Some("alpha".to_string()),
          role: "codex".to_string(),
          task: "follow up".to_string(),
          depth: 1,
          state: CollabAgentWaitState {
            pending_wake_count: 1,
            ..member_state(
              CollabAgentLifecycle::Ready,
              CollabTurnOutcome::Succeeded,
              Some("finished first pass"),
            )
          },
        },
      ],
      tasks: Vec::new(),
      task_edges: Vec::new(),
      plans: Vec::new(),
      unread_counts: HashMap::new(),
      mailbox_version: 0,
      recent_messages: Vec::new(),
      ownership_leases: Vec::new(),
      workflow: None,
    };
    snapshot.members[0].state = member_state(
      CollabAgentLifecycle::Ready,
      CollabTurnOutcome::Succeeded,
      None,
    );

    let lines = working_summary_lines(&snapshot).expect("summary");
    let rendered = lines
      .iter()
      .next()
      .map(|line| {
        line
          .spans
          .iter()
          .map(|span| span.content.as_ref())
          .collect::<String>()
      })
      .unwrap_or_default();

    assert!(rendered.contains("Agent teams working..."));
    assert!(!rendered.contains("Agent Teams Done"));
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
            state: member_state(CollabAgentLifecycle::Busy, CollabTurnOutcome::NoneYet, None),
          },
          TeamMember {
            thread_id: "ash-thread".to_string(),
            nickname: Some("艾许".to_string()),
            role: "default".to_string(),
            task: "你是团队成员“艾许”。请从架构角度继续深入分析。".to_string(),
            depth: 1,
            state: member_state(CollabAgentLifecycle::Busy, CollabTurnOutcome::NoneYet, None),
          },
          TeamMember {
            thread_id: "sparrow-thread".to_string(),
            nickname: Some("六雀".to_string()),
            role: "default".to_string(),
            task: "你是团队成员“六雀”。请从测试工具链角度继续深入分析。".to_string(),
            depth: 1,
            state: member_state(
              CollabAgentLifecycle::Ready,
              CollabTurnOutcome::Succeeded,
              Some("Research summary completed"),
            ),
          },
        ],
        tasks: vec![
          TeamTask {
            id: "old-task".to_string(),
            title: "Project Exploration - Core".to_string(),
            details: Some("历史遗留任务".to_string()),
            status: TeamTaskStatus::Pending,
            ready_state: TeamTaskReadyState::Ready,
            review_state: TeamTaskReviewState::NotRequested,
            owner_thread_id: None,
            blocked_by_task_ids: Vec::new(),
            blocks_task_ids: Vec::new(),
            blocking_reason: None,
            blockers: Vec::<cokra_protocol::TaskBlocker>::new(),
            requested_scopes: Vec::<ScopeRequest>::new(),
            granted_scopes: Vec::<ScopeRequest>::new(),
            scope_policy_override: false,
            assignee_thread_id: None,
            reviewer_thread_id: None,
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
            ready_state: TeamTaskReadyState::Claimed,
            review_state: TeamTaskReviewState::NotRequested,
            owner_thread_id: Some("ash-thread".to_string()),
            blocked_by_task_ids: Vec::new(),
            blocks_task_ids: Vec::new(),
            blocking_reason: None,
            blockers: Vec::<cokra_protocol::TaskBlocker>::new(),
            requested_scopes: Vec::<ScopeRequest>::new(),
            granted_scopes: Vec::<ScopeRequest>::new(),
            scope_policy_override: false,
            assignee_thread_id: Some("ash-thread".to_string()),
            reviewer_thread_id: None,
            workflow_run_id: None,
            created_at: 2,
            updated_at: 2,
            notes: Vec::new(),
          },
        ],
        task_edges: Vec::<cokra_protocol::TaskEdge>::new(),
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
        mailbox_version: 2,
        recent_messages: vec![TeamMessage {
          id: "msg-1".to_string(),
          sender_thread_id: "ash-thread".to_string(),
          recipient_thread_id: Some("sparrow-thread".to_string()),
          kind: cokra_protocol::TeamMessageKind::Direct,
          route_key: None,
          claimed_by_thread_id: None,
          delivery_mode: cokra_protocol::TeamMessageDeliveryMode::DurableMail,
          priority: cokra_protocol::TeamMessagePriority::Normal,
          correlation_id: None,
          task_id: Some("new-task".to_string()),
          ack_state: cokra_protocol::TeamMessageAckState::Pending,
          message: "please review the lock handoff".to_string(),
          created_at: 3,
          expires_at: None,
          acknowledged_at: None,
          acknowledged_by_thread_id: None,
          unread: true,
        }],
        ownership_leases: vec![OwnershipLease {
          id: "lease-1".to_string(),
          task_id: "new-task".to_string(),
          owner_thread_id: "ash-thread".to_string(),
          scope: cokra_protocol::OwnershipScope {
            kind: cokra_protocol::OwnershipScopeKind::File,
            path: "/repo/src/lib.rs".to_string(),
          },
          access: cokra_protocol::OwnershipAccessMode::ExclusiveWrite,
          acquired_at: 2,
          heartbeat_at: 3,
          expires_at: None,
        }],
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
    assert!(rendered.contains("@艾许: Busy"));
    assert!(rendered.contains("@六雀: Idle"));
    assert!(rendered.contains("Unread: @艾许 1 msg"));
    assert!(rendered.contains("1 plan(s) pending review"));
    assert!(rendered.contains("Live locks: 1 | Recent mailbox items: 1"));
    assert!(rendered.contains("(use /collab to open Team Panel"));
    assert!(!rendered.contains("Mailbox flows"));
    assert!(!rendered.contains("Task graph"));
    assert!(!rendered.contains("Active locks"));
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
  fn mailbox_delivered_uses_simple_sender_prompt_label() {
    let cell = mailbox_delivered(CollabMailboxDeliveredEvent {
      thread_id: "review-thread".to_string(),
      sender_thread_id: "root-thread".to_string(),
      sender_nickname: Some("main".to_string()),
      sender_role: Some("root".to_string()),
      recipient_thread_id: "review-thread".to_string(),
      recipient_nickname: Some("reviewer".to_string()),
      recipient_role: Some("general".to_string()),
      message: "Please run the second review pass and report back.".to_string(),
      task_id: None,
      delivery_mode: cokra_protocol::TeamMessageDeliveryMode::DurableMail,
      kind: cokra_protocol::TeamMessageKind::Direct,
      created_at: 0,
    });

    let rendered = cell
      .display_lines(80)
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

    assert!(rendered.contains("@main > Please run the second review pass and report back."));
    assert!(!rendered.contains("[root]"));
    assert!(!rendered.contains("(root-thread)"));
  }

  #[test]
  fn team_snapshot_hides_blocker_lines_for_completed_tasks() {
    let cell = team_snapshot(CollabTeamSnapshotEvent {
      actor_thread_id: "root-thread".to_string(),
      snapshot: TeamSnapshot {
        root_thread_id: "root-thread".to_string(),
        members: vec![TeamMember {
          thread_id: "root-thread".to_string(),
          nickname: None,
          role: "root".to_string(),
          task: "root session".to_string(),
          depth: 0,
          state: member_state(
            CollabAgentLifecycle::Ready,
            CollabTurnOutcome::Succeeded,
            None,
          ),
        }],
        tasks: vec![TeamTask {
          id: "task-1".to_string(),
          title: "Finish review".to_string(),
          details: None,
          status: TeamTaskStatus::Completed,
          ready_state: TeamTaskReadyState::Completed,
          review_state: TeamTaskReviewState::NotRequested,
          owner_thread_id: Some("root-thread".to_string()),
          blocked_by_task_ids: Vec::new(),
          blocks_task_ids: Vec::new(),
          blocking_reason: Some("should stay hidden".to_string()),
          blockers: vec![cokra_protocol::TaskBlocker {
            id: "blocker-1".to_string(),
            kind: cokra_protocol::TaskBlockerKind::Manual,
            blocking_task_id: None,
            reason: "blocked by thread-xyz".to_string(),
            active: true,
            created_at: 1,
            cleared_at: None,
          }],
          requested_scopes: Vec::new(),
          granted_scopes: Vec::new(),
          scope_policy_override: false,
          assignee_thread_id: Some("root-thread".to_string()),
          reviewer_thread_id: None,
          workflow_run_id: None,
          created_at: 1,
          updated_at: 1,
          notes: Vec::new(),
        }],
        task_edges: Vec::new(),
        plans: Vec::new(),
        unread_counts: HashMap::new(),
        mailbox_version: 0,
        recent_messages: Vec::new(),
        ownership_leases: Vec::new(),
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

    assert!(rendered.contains("(use /collab to open Team Panel"));
    assert!(!rendered.contains("blocked:"));
    assert!(!rendered.contains("should stay hidden"));
  }

  #[test]
  fn snapshot_message_line_uses_names_with_short_codes() {
    let mut labels = HashMap::new();
    labels.insert(
      "root-thread".to_string(),
      "@main [root] (root1234)".to_string(),
    );
    labels.insert(
      "review-thread".to_string(),
      "@reviewer [general] (revu1234)".to_string(),
    );

    let line = snapshot_message_line_with_labels(
      &cokra_protocol::TeamMessage {
        id: "msg-1".to_string(),
        sender_thread_id: "review-thread".to_string(),
        recipient_thread_id: Some("root-thread".to_string()),
        kind: cokra_protocol::TeamMessageKind::Direct,
        route_key: None,
        claimed_by_thread_id: None,
        delivery_mode: cokra_protocol::TeamMessageDeliveryMode::DurableMail,
        priority: cokra_protocol::TeamMessagePriority::Normal,
        correlation_id: None,
        task_id: None,
        ack_state: cokra_protocol::TeamMessageAckState::Pending,
        message: "review is complete".to_string(),
        created_at: 1,
        expires_at: None,
        acknowledged_at: None,
        acknowledged_by_thread_id: None,
        unread: true,
      },
      &labels,
    );

    let rendered = line
      .spans
      .iter()
      .map(|span| span.content.as_ref())
      .collect::<String>();

    assert!(rendered.contains("@reviewer [general] (revu1234) -> @main [root] (root1234)"));
    assert!(rendered.contains("review is complete"));
  }

  #[test]
  fn snapshot_task_line_uses_display_labels_for_owner() {
    let mut labels = HashMap::new();
    labels.insert(
      "owner-thread".to_string(),
      "@impl [general] (impl1234)".to_string(),
    );
    labels.insert(
      "review-thread".to_string(),
      "@reviewer [general] (revu1234)".to_string(),
    );

    let line = snapshot_task_line(
      &TeamTask {
        id: "task-1".to_string(),
        title: "Implement feature".to_string(),
        details: None,
        status: TeamTaskStatus::Review,
        ready_state: TeamTaskReadyState::Review,
        review_state: TeamTaskReviewState::Requested,
        owner_thread_id: Some("owner-thread".to_string()),
        blocked_by_task_ids: Vec::new(),
        blocks_task_ids: Vec::new(),
        blocking_reason: None,
        blockers: Vec::new(),
        requested_scopes: Vec::new(),
        granted_scopes: Vec::new(),
        scope_policy_override: false,
        assignee_thread_id: Some("owner-thread".to_string()),
        reviewer_thread_id: Some("review-thread".to_string()),
        workflow_run_id: None,
        created_at: 1,
        updated_at: 2,
        notes: Vec::new(),
      },
      &labels,
    );

    let rendered = line
      .spans
      .iter()
      .map(|span| span.content.as_ref())
      .collect::<String>();

    // New format: status label, short ID, owner label, title
    assert!(
      rendered.contains("Review"),
      "should contain status label: {rendered}"
    );
    assert!(
      rendered.contains("#task-1"),
      "should contain short task ID: {rendered}"
    );
    assert!(
      rendered.contains("@impl [general] (impl1234)"),
      "should contain owner label: {rendered}"
    );
    assert!(
      rendered.contains("Implement feature"),
      "should contain task title: {rendered}"
    );
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
          state: member_state(
            CollabAgentLifecycle::Ready,
            CollabTurnOutcome::Succeeded,
            Some("哎哎哎——菅田くん，等一下！让我来一条一条给你整理一下！"),
          ),
        },
        CollabAgentStatusEntry {
          thread_id: "masaki-thread".to_string(),
          nickname: Some("菅田将晖".to_string()),
          role: Some("default".to_string()),
          state: member_state(
            CollabAgentLifecycle::Ready,
            CollabTurnOutcome::Succeeded,
            Some("哈，架纯你说的也不是没道理……但等等，我有话说。"),
          ),
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
    assert!(rendered.contains("│  ⎿ Idle"));
    assert!(rendered.contains("└─ @菅田将晖"));
    assert!(rendered.contains("⎿ Idle"));
  }
}
