use std::collections::HashMap;
use std::collections::HashSet;

use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

use cokra_protocol::AgentStatus;
use cokra_protocol::ScopeRequest;
use cokra_protocol::TaskBlocker;
use cokra_protocol::TaskBlockerKind;
use cokra_protocol::TaskEdge;
use cokra_protocol::TaskEdgeKind;
use cokra_protocol::TeamMember;
use cokra_protocol::TeamMessage;
use cokra_protocol::TeamMessageAckState;
use cokra_protocol::TeamMessageDeliveryMode;
use cokra_protocol::TeamMessageKind;
use cokra_protocol::TeamMessagePriority;
use cokra_protocol::TeamPlan;
use cokra_protocol::TeamPlanStatus;
use cokra_protocol::TeamSnapshot;
use cokra_protocol::TeamTask;
use cokra_protocol::TeamTaskReadyState;
use cokra_protocol::TeamTaskReviewState;
use cokra_protocol::TeamTaskStatus;
use cokra_protocol::WorkflowRuntimeSnapshot;

use crate::thread_manager::ThreadInfo;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMessage {
  id: String,
  sender_thread_id: String,
  recipient_thread_id: Option<String>,
  #[serde(default)]
  kind: TeamMessageKind,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  route_key: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  claimed_by_thread_id: Option<String>,
  #[serde(default)]
  delivery_mode: TeamMessageDeliveryMode,
  #[serde(default)]
  priority: TeamMessagePriority,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  correlation_id: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  task_id: Option<String>,
  #[serde(default)]
  ack_state: TeamMessageAckState,
  message: String,
  created_at: i64,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  expires_at: Option<i64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  acknowledged_at: Option<i64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  acknowledged_by_thread_id: Option<String>,
  #[serde(default)]
  seen_by: HashSet<String>,
}

impl StoredMessage {
  fn is_expired(&self, now: i64) -> bool {
    self
      .expires_at
      .is_some_and(|expires_at| expires_at <= now)
      && matches!(self.delivery_mode, TeamMessageDeliveryMode::EphemeralNudge)
  }

  fn is_visible_to(&self, thread_id: &str) -> bool {
    match &self.recipient_thread_id {
      Some(recipient_thread_id) => {
        recipient_thread_id == thread_id || self.sender_thread_id == thread_id
      }
      None => match self.kind {
        TeamMessageKind::Queue => {
          self.claimed_by_thread_id.as_deref() == Some(thread_id)
            || self.sender_thread_id == thread_id
        }
        _ => true,
      },
    }
  }

  fn can_ack(&self, thread_id: &str) -> bool {
    match self.kind {
      TeamMessageKind::Queue => self.claimed_by_thread_id.as_deref() == Some(thread_id),
      _ => self.recipient_thread_id.as_deref() == Some(thread_id),
    }
  }

  fn to_team_message(&self, thread_id: &str) -> TeamMessage {
    TeamMessage {
      id: self.id.clone(),
      sender_thread_id: self.sender_thread_id.clone(),
      recipient_thread_id: self.recipient_thread_id.clone(),
      kind: self.kind.clone(),
      route_key: self.route_key.clone(),
      claimed_by_thread_id: self.claimed_by_thread_id.clone(),
      delivery_mode: self.delivery_mode.clone(),
      priority: self.priority.clone(),
      correlation_id: self.correlation_id.clone(),
      task_id: self.task_id.clone(),
      ack_state: self.ack_state.clone(),
      message: self.message.clone(),
      created_at: self.created_at,
      expires_at: self.expires_at,
      acknowledged_at: self.acknowledged_at,
      acknowledged_by_thread_id: self.acknowledged_by_thread_id.clone(),
      unread: !self.seen_by.contains(thread_id),
    }
  }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct TeamState {
  #[serde(default)]
  tasks: HashMap<String, TeamTask>,
  #[serde(default)]
  task_edges: Vec<TaskEdge>,
  #[serde(default)]
  plans: HashMap<String, TeamPlan>,
  #[serde(default)]
  messages: Vec<StoredMessage>,
  #[serde(default)]
  mailbox_version: u64,
}

impl TeamState {
  pub(crate) fn snapshot(
    &mut self,
    root_thread_id: String,
    threads: Vec<ThreadInfo>,
    statuses: HashMap<String, AgentStatus>,
    run_state: Option<WorkflowRuntimeSnapshot>,
  ) -> TeamSnapshot {
    self.recompute_task_graph();
    let members = threads
      .into_iter()
      .map(|thread| TeamMember {
        thread_id: thread.thread_id.to_string(),
        nickname: thread.nickname,
        role: thread.role,
        task: thread.task,
        depth: thread.depth,
        status: statuses
          .get(&thread.thread_id.to_string())
          .cloned()
          .unwrap_or(AgentStatus::NotFound),
      })
      .collect::<Vec<_>>();

    let tasks = self.sorted_tasks();
    let plans = self.sorted_plans();
    let unread_counts = members
      .iter()
      .map(|member| {
        (
          member.thread_id.clone(),
          self.unread_count_for(&member.thread_id),
        )
      })
      .collect();

    TeamSnapshot {
      root_thread_id,
      members,
      tasks,
      task_edges: self.sorted_task_edges(),
      plans,
      unread_counts,
      mailbox_version: self.mailbox_version,
      workflow: run_state,
    }
  }

  pub(crate) fn mailbox_version(&self) -> u64 {
    self.mailbox_version
  }

  pub(crate) fn submit_plan(
    &mut self,
    author_thread_id: String,
    summary: String,
    steps: Vec<String>,
    requires_approval: bool,
    workflow_run_id: Option<String>,
  ) -> TeamPlan {
    let now = Utc::now().timestamp();
    let plan = TeamPlan {
      id: Uuid::new_v4().to_string(),
      author_thread_id,
      summary,
      steps,
      status: if requires_approval {
        TeamPlanStatus::PendingApproval
      } else {
        TeamPlanStatus::Approved
      },
      requires_approval,
      reviewer_thread_id: None,
      review_note: None,
      workflow_run_id,
      created_at: now,
      updated_at: now,
    };
    self.plans.insert(plan.id.clone(), plan.clone());
    plan
  }

  pub(crate) fn decide_plan(
    &mut self,
    plan_id: &str,
    reviewer_thread_id: String,
    approved: bool,
    note: Option<String>,
  ) -> Option<TeamPlan> {
    let plan = self.plans.get_mut(plan_id)?;
    plan.status = if approved {
      TeamPlanStatus::Approved
    } else {
      TeamPlanStatus::Rejected
    };
    plan.reviewer_thread_id = Some(reviewer_thread_id);
    plan.review_note = note;
    plan.updated_at = Utc::now().timestamp();
    Some(plan.clone())
  }

  pub(crate) fn requires_plan_approval(&self, thread_id: &str) -> bool {
    self
      .plans
      .values()
      .filter(|plan| plan.author_thread_id == thread_id && plan.requires_approval)
      .any(|plan| {
        matches!(
          plan.status,
          TeamPlanStatus::PendingApproval | TeamPlanStatus::Rejected
        )
      })
  }

  pub(crate) fn create_task(
    &mut self,
    title: String,
    details: Option<String>,
    owner_thread_id: Option<String>,
    assignee_thread_id: Option<String>,
    workflow_run_id: Option<String>,
    requested_scopes: Vec<ScopeRequest>,
    blocking_reason: Option<String>,
  ) -> TeamTask {
    let now = Utc::now().timestamp();
    let task_id = Uuid::new_v4().to_string();
    let mut task = TeamTask {
      id: task_id.clone(),
      title,
      details,
      status: TeamTaskStatus::Pending,
      ready_state: TeamTaskReadyState::Ready,
      review_state: TeamTaskReviewState::NotRequested,
      owner_thread_id: owner_thread_id.or_else(|| assignee_thread_id.clone()),
      blocked_by_task_ids: Vec::new(),
      blocks_task_ids: Vec::new(),
      blocking_reason: None,
      blockers: Vec::new(),
      requested_scopes,
      granted_scopes: Vec::new(),
      assignee_thread_id,
      workflow_run_id,
      created_at: now,
      updated_at: now,
      notes: Vec::new(),
    };
    if let Some(blocking_reason) = blocking_reason.filter(|value| !value.trim().is_empty()) {
      task.blockers.push(TaskBlocker {
        id: Uuid::new_v4().to_string(),
        kind: TaskBlockerKind::Manual,
        blocking_task_id: None,
        reason: blocking_reason,
        active: true,
        created_at: now,
        cleared_at: None,
      });
    }
    self.tasks.insert(task_id.clone(), task);
    self.recompute_task_graph();
    self
      .tasks
      .get(&task_id)
      .cloned()
      .expect("created task should be readable")
  }

  pub(crate) fn update_task(
    &mut self,
    task_id: &str,
    status: Option<TeamTaskStatus>,
    assignee_thread_id: Option<Option<String>>,
    owner_thread_id: Option<Option<String>>,
    note: Option<String>,
    requested_scopes: Option<Vec<ScopeRequest>>,
    granted_scopes: Option<Vec<ScopeRequest>>,
    review_state: Option<TeamTaskReviewState>,
  ) -> Option<TeamTask> {
    {
      let task = self.tasks.get_mut(task_id)?;
      let granted_scopes_changed = granted_scopes.is_some();
      let review_state_changed = review_state.is_some();
      if let Some(status) = status {
        task.status = status;
      }
      if let Some(assignee_thread_id) = assignee_thread_id {
        task.assignee_thread_id = assignee_thread_id;
      }
      if let Some(owner_thread_id) = owner_thread_id {
        task.owner_thread_id = owner_thread_id;
      }
      if let Some(requested_scopes) = requested_scopes {
        task.requested_scopes = requested_scopes;
      }
      if let Some(granted_scopes) = granted_scopes {
        task.granted_scopes = granted_scopes;
      }
      if let Some(review_state) = review_state {
        task.review_state = review_state;
      }
      if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
        task.notes.push(note);
      }
      if !granted_scopes_changed {
        match task.status {
          TeamTaskStatus::Pending => {
            task.granted_scopes.clear();
          }
          TeamTaskStatus::InProgress | TeamTaskStatus::Review => {
            task.granted_scopes = task.requested_scopes.clone();
          }
          TeamTaskStatus::Completed | TeamTaskStatus::Failed | TeamTaskStatus::Canceled => {
            task.granted_scopes.clear();
          }
        }
      }
      if !review_state_changed && matches!(task.status, TeamTaskStatus::Pending) {
        task.review_state = TeamTaskReviewState::NotRequested;
      }
      task.updated_at = Utc::now().timestamp();
    }
    self.recompute_task_graph();
    self.tasks.get(task_id).cloned()
  }

  pub(crate) fn assign_task(
    &mut self,
    task_id: &str,
    assignee_thread_id: String,
    note: Option<String>,
  ) -> Option<TeamTask> {
    {
      let task = self.tasks.get_mut(task_id)?;
      task.assignee_thread_id = Some(assignee_thread_id.clone());
      task.owner_thread_id = Some(assignee_thread_id);
      task.status = TeamTaskStatus::Pending;
      task.review_state = TeamTaskReviewState::NotRequested;
      task.granted_scopes.clear();
      if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
        task.notes.push(note);
      }
      task.updated_at = Utc::now().timestamp();
    }
    self.recompute_task_graph();
    self.tasks.get(task_id).cloned()
  }

  pub(crate) fn handoff_task(
    &mut self,
    task_id: &str,
    to_thread_id: String,
    note: Option<String>,
    review_mode: bool,
  ) -> Option<TeamTask> {
    {
      let task = self.tasks.get_mut(task_id)?;
      task.assignee_thread_id = Some(to_thread_id.clone());
      task.owner_thread_id = Some(to_thread_id);
      task.status = if review_mode {
        TeamTaskStatus::Review
      } else {
        TeamTaskStatus::Pending
      };
      task.review_state = if review_mode {
        TeamTaskReviewState::Requested
      } else {
        TeamTaskReviewState::NotRequested
      };
      if !review_mode {
        task.granted_scopes.clear();
      } else {
        task.granted_scopes = task.requested_scopes.clone();
      }
      if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
        task.notes.push(note);
      }
      task.updated_at = Utc::now().timestamp();
    }
    self.recompute_task_graph();
    self.tasks.get(task_id).cloned()
  }

  pub(crate) fn claim_task(
    &mut self,
    task_id: &str,
    claimer_thread_id: String,
    note: Option<String>,
  ) -> Option<TeamTask> {
    self.claim_task_internal(task_id, Some((claimer_thread_id, note)), true)
  }

  pub(crate) fn list_ready_tasks(&mut self, claimer_thread_id: &str) -> Vec<TeamTask> {
    self.recompute_task_graph();
    let mut tasks = self
      .tasks
      .values()
      .filter(|task| {
        matches!(task.ready_state, TeamTaskReadyState::Ready)
          && task
            .assignee_thread_id
            .as_deref()
            .is_none_or(|assignee| assignee == claimer_thread_id)
      })
      .cloned()
      .collect::<Vec<_>>();
    tasks.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    tasks
  }

  pub(crate) fn claim_ready_task(
    &mut self,
    task_id: &str,
    claimer_thread_id: String,
    note: Option<String>,
  ) -> Option<TeamTask> {
    if !self.is_task_ready(task_id) {
      return None;
    }
    self.claim_task_internal(task_id, Some((claimer_thread_id, note)), true)
  }

  pub(crate) fn claim_next_task(&mut self, claimer_thread_id: &str) -> Option<TeamTask> {
    let next_id = self
      .list_ready_tasks(claimer_thread_id)
      .first()
      .map(|task| task.id.clone())?;
    self.claim_ready_task(&next_id, claimer_thread_id.to_string(), None)
  }

  pub(crate) fn add_task_dependency(
    &mut self,
    task_id: &str,
    dependency_task_id: &str,
    reason: Option<String>,
  ) -> Option<TeamTask> {
    if task_id == dependency_task_id
      || !self.tasks.contains_key(task_id)
      || !self.tasks.contains_key(dependency_task_id)
    {
      return None;
    }
    let mut frontier = vec![task_id.to_string()];
    let mut visited = HashSet::new();
    while let Some(current_task_id) = frontier.pop() {
      if !visited.insert(current_task_id.clone()) {
        continue;
      }
      if current_task_id == dependency_task_id {
        return None;
      }
      frontier.extend(
        self
          .task_edges
          .iter()
          .filter(|edge| edge.from_task_id == current_task_id)
          .map(|edge| edge.to_task_id.clone()),
      );
    }
    if !self.task_edges.iter().any(|edge| {
      edge.from_task_id == dependency_task_id && edge.to_task_id == task_id
    }) {
      self.task_edges.push(TaskEdge {
        from_task_id: dependency_task_id.to_string(),
        to_task_id: task_id.to_string(),
        kind: TaskEdgeKind::Blocks,
        reason,
        created_at: Utc::now().timestamp(),
      });
    }
    self.recompute_task_graph();
    self.tasks.get(task_id).cloned()
  }

  pub(crate) fn remove_task_dependency(
    &mut self,
    task_id: &str,
    dependency_task_id: &str,
  ) -> Option<TeamTask> {
    let original_len = self.task_edges.len();
    self.task_edges.retain(|edge| {
      !(edge.from_task_id == dependency_task_id && edge.to_task_id == task_id)
    });
    if original_len == self.task_edges.len() {
      return None;
    }
    self.recompute_task_graph();
    self.tasks.get(task_id).cloned()
  }

  pub(crate) fn block_task(&mut self, task_id: &str, reason: String) -> Option<TeamTask> {
    let reason = reason.trim();
    if reason.is_empty() {
      return None;
    }
    {
      let task = self.tasks.get_mut(task_id)?;
      task.blockers.push(TaskBlocker {
        id: Uuid::new_v4().to_string(),
        kind: TaskBlockerKind::Manual,
        blocking_task_id: None,
        reason: reason.to_string(),
        active: true,
        created_at: Utc::now().timestamp(),
        cleared_at: None,
      });
      task.updated_at = Utc::now().timestamp();
    }
    self.recompute_task_graph();
    self.tasks.get(task_id).cloned()
  }

  pub(crate) fn unblock_task(
    &mut self,
    task_id: &str,
    blocker_id: Option<&str>,
  ) -> Option<TeamTask> {
    {
      let task = self.tasks.get_mut(task_id)?;
      let now = Utc::now().timestamp();
      let mut changed = false;
      for blocker in &mut task.blockers {
        if blocker.kind != TaskBlockerKind::Manual || !blocker.active {
          continue;
        }
        if blocker_id.is_none_or(|value| blocker.id == value) {
          blocker.active = false;
          blocker.cleared_at = Some(now);
          changed = true;
        }
      }
      if !changed {
        return None;
      }
      task.updated_at = now;
    }
    self.recompute_task_graph();
    self.tasks.get(task_id).cloned()
  }

  #[allow(clippy::too_many_arguments)]
  pub(crate) fn post_message(
    &mut self,
    sender_thread_id: String,
    recipient_thread_id: Option<String>,
    kind: TeamMessageKind,
    route_key: Option<String>,
    delivery_mode: TeamMessageDeliveryMode,
    priority: TeamMessagePriority,
    correlation_id: Option<String>,
    task_id: Option<String>,
    message: String,
    expires_at: Option<i64>,
  ) -> TeamMessage {
    self.expire_messages();
    let mut seen_by = HashSet::new();
    seen_by.insert(sender_thread_id.clone());
    let ack_state = if recipient_thread_id.is_some() || matches!(kind, TeamMessageKind::Queue) {
      TeamMessageAckState::Pending
    } else {
      TeamMessageAckState::NotRequired
    };
    let stored = StoredMessage {
      id: Uuid::new_v4().to_string(),
      sender_thread_id,
      recipient_thread_id,
      kind: kind.clone(),
      route_key,
      claimed_by_thread_id: None,
      delivery_mode,
      priority,
      correlation_id,
      task_id,
      ack_state,
      message,
      created_at: Utc::now().timestamp(),
      expires_at,
      acknowledged_at: None,
      acknowledged_by_thread_id: None,
      seen_by,
    };
    let out = stored.to_team_message(&stored.sender_thread_id);
    self.messages.push(stored);
    self.bump_mailbox_version();
    out
  }

  pub(crate) fn peek_messages(&mut self, reader_thread_id: &str, unread_only: bool) -> Vec<TeamMessage> {
    self.collect_messages(reader_thread_id, unread_only, false)
  }

  pub(crate) fn read_messages(
    &mut self,
    reader_thread_id: &str,
    unread_only: bool,
  ) -> Vec<TeamMessage> {
    self.collect_messages(reader_thread_id, unread_only, true)
  }

  pub(crate) fn claim_queue_messages(
    &mut self,
    claimer_thread_id: &str,
    queue_name: &str,
    limit: usize,
  ) -> Vec<TeamMessage> {
    self.expire_messages();
    let now = Utc::now().timestamp();
    let mut eligible_indexes = self
      .messages
      .iter()
      .enumerate()
      .filter_map(|(index, message)| {
        (!message.is_expired(now)
          && message.kind == TeamMessageKind::Queue
          && message.route_key.as_deref() == Some(queue_name)
          && message.claimed_by_thread_id.is_none())
        .then_some((index, Self::message_priority_rank(&message.priority), message.created_at))
      })
      .collect::<Vec<_>>();
    eligible_indexes.sort_by(|left, right| {
      right
        .1
        .cmp(&left.1)
        .then_with(|| left.2.cmp(&right.2))
    });
    let messages = eligible_indexes
      .into_iter()
      .take(limit)
      .filter_map(|(index, _, _)| {
        let message = self.messages.get_mut(index)?;
        message.claimed_by_thread_id = Some(claimer_thread_id.to_string());
        message.seen_by.insert(claimer_thread_id.to_string());
        Some(message.to_team_message(claimer_thread_id))
      })
      .collect::<Vec<_>>();
    if !messages.is_empty() {
      self.bump_mailbox_version();
    }
    messages
  }

  pub(crate) fn ack_message(
    &mut self,
    acker_thread_id: &str,
    message_id: &str,
  ) -> Option<TeamMessage> {
    self.expire_messages();
    let message = {
      let message = self.messages.iter_mut().find(|message| {
        message.id == message_id
          && message.can_ack(acker_thread_id)
          && matches!(message.ack_state, TeamMessageAckState::Pending)
      })?;
      let now = Utc::now().timestamp();
      message.ack_state = TeamMessageAckState::Acknowledged;
      message.acknowledged_at = Some(now);
      message.acknowledged_by_thread_id = Some(acker_thread_id.to_string());
      message.seen_by.insert(acker_thread_id.to_string());
      message.to_team_message(acker_thread_id)
    };
    self.bump_mailbox_version();
    Some(message)
  }

  pub(crate) fn clear(&mut self) {
    self.tasks.clear();
    self.task_edges.clear();
    self.plans.clear();
    self.messages.clear();
    self.mailbox_version = 0;
  }

  pub(crate) fn likely_root_thread_id(&self) -> Option<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    for message in &self.messages {
      *counts.entry(message.sender_thread_id.clone()).or_default() += 2;
      if let Some(recipient) = &message.recipient_thread_id {
        *counts.entry(recipient.clone()).or_default() += 1;
      }
      if let Some(claimer) = &message.claimed_by_thread_id {
        *counts.entry(claimer.clone()).or_default() += 1;
      }
      for seen in &message.seen_by {
        *counts.entry(seen.clone()).or_default() += 1;
      }
    }

    counts
      .into_iter()
      .max_by_key(|(_id, count)| *count)
      .map(|(id, _)| id)
  }

  fn claim_task_internal(
    &mut self,
    task_id: &str,
    claim: Option<(String, Option<String>)>,
    requires_ready: bool,
  ) -> Option<TeamTask> {
    if requires_ready && !self.is_task_ready(task_id) {
      return None;
    }
    {
      let task = self.tasks.get_mut(task_id)?;
      if let Some((claimer_thread_id, note)) = claim {
        task.assignee_thread_id = Some(claimer_thread_id.clone());
        task.owner_thread_id = Some(claimer_thread_id);
        if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
          task.notes.push(note);
        }
      }
      task.status = TeamTaskStatus::InProgress;
      task.review_state = TeamTaskReviewState::NotRequested;
      task.granted_scopes = task.requested_scopes.clone();
      task.updated_at = Utc::now().timestamp();
    }
    self.recompute_task_graph();
    self.tasks.get(task_id).cloned()
  }

  fn collect_messages(
    &mut self,
    reader_thread_id: &str,
    unread_only: bool,
    mark_seen: bool,
  ) -> Vec<TeamMessage> {
    self.expire_messages();
    let now = Utc::now().timestamp();
    let mut changed = false;
    let mut messages = self
      .messages
      .iter_mut()
      .filter(|message| {
        message.is_visible_to(reader_thread_id)
          && !message.is_expired(now)
          && (!unread_only || !message.seen_by.contains(reader_thread_id))
      })
      .map(|message| {
        if mark_seen && message.seen_by.insert(reader_thread_id.to_string()) {
          changed = true;
        }
        message.to_team_message(reader_thread_id)
      })
      .collect::<Vec<_>>();
    messages.sort_by(|left, right| {
      Self::message_priority_rank(&right.priority)
        .cmp(&Self::message_priority_rank(&left.priority))
        .then_with(|| left.created_at.cmp(&right.created_at))
    });
    if changed {
      self.bump_mailbox_version();
    }
    messages
  }

  fn expire_messages(&mut self) {
    let now = Utc::now().timestamp();
    let mut changed = false;
    for message in &mut self.messages {
      if message.is_expired(now) && !matches!(message.ack_state, TeamMessageAckState::Expired) {
        message.ack_state = TeamMessageAckState::Expired;
        changed = true;
      }
    }
    if changed {
      self.bump_mailbox_version();
    }
  }

  fn bump_mailbox_version(&mut self) {
    self.mailbox_version = self.mailbox_version.saturating_add(1);
  }

  fn message_priority_rank(priority: &TeamMessagePriority) -> u8 {
    match priority {
      TeamMessagePriority::Urgent => 3,
      TeamMessagePriority::High => 2,
      TeamMessagePriority::Normal => 1,
      TeamMessagePriority::Low => 0,
    }
  }

  fn is_task_ready(&mut self, task_id: &str) -> bool {
    self.recompute_task_graph();
    self
      .tasks
      .get(task_id)
      .is_some_and(|task| matches!(task.ready_state, TeamTaskReadyState::Ready))
  }

  fn recompute_task_graph(&mut self) {
    let now = Utc::now().timestamp();
    let status_by_id = self
      .tasks
      .iter()
      .map(|(task_id, task)| (task_id.clone(), task.status.clone()))
      .collect::<HashMap<_, _>>();
    let existing_task_ids = self.tasks.keys().cloned().collect::<HashSet<_>>();

    let mut blocked_by = HashMap::<String, Vec<(String, Option<String>)>>::new();
    let mut blocks = HashMap::<String, Vec<String>>::new();
    self.task_edges.retain(|edge| {
      existing_task_ids.contains(&edge.from_task_id) && existing_task_ids.contains(&edge.to_task_id)
    });
    for edge in &self.task_edges {
      blocked_by
        .entry(edge.to_task_id.clone())
        .or_default()
        .push((edge.from_task_id.clone(), edge.reason.clone()));
      blocks
        .entry(edge.from_task_id.clone())
        .or_default()
        .push(edge.to_task_id.clone());
    }

    for (task_id, task) in &mut self.tasks {
      let manual_blockers = task
        .blockers
        .iter()
        .filter(|blocker| blocker.kind == TaskBlockerKind::Manual)
        .cloned()
        .collect::<Vec<_>>();
      task.blocked_by_task_ids = blocked_by
        .get(task_id)
        .map(|items| {
          items
            .iter()
            .map(|(dependency_task_id, _)| dependency_task_id.clone())
            .collect::<Vec<_>>()
        })
        .unwrap_or_default();
      task.blocks_task_ids = blocks.get(task_id).cloned().unwrap_or_default();
      let mut blockers = manual_blockers;
      if let Some(dependencies) = blocked_by.get(task_id) {
        for (dependency_task_id, reason) in dependencies {
          let dependency_status = status_by_id.get(dependency_task_id);
          let active = dependency_status
            .is_some_and(|status| !matches!(status, TeamTaskStatus::Completed));
          let blocker_reason = match dependency_status {
            Some(TeamTaskStatus::Failed) => {
              format!("dependency {dependency_task_id} failed")
            }
            Some(TeamTaskStatus::Canceled) => {
              format!("dependency {dependency_task_id} was canceled")
            }
            Some(_) => reason
              .clone()
              .unwrap_or_else(|| format!("blocked by task {dependency_task_id}")),
            None => format!("dependency {dependency_task_id} is missing"),
          };
          blockers.push(TaskBlocker {
            id: format!("dependency:{task_id}:{dependency_task_id}"),
            kind: TaskBlockerKind::Dependency,
            blocking_task_id: Some(dependency_task_id.clone()),
            reason: blocker_reason,
            active,
            created_at: now,
            cleared_at: if active { None } else { Some(now) },
          });
        }
      }
      task.blockers = blockers;
      task.blocking_reason = task
        .blockers
        .iter()
        .find(|blocker| blocker.active)
        .map(|blocker| blocker.reason.clone());
      task.ready_state = match task.status {
        TeamTaskStatus::Completed => TeamTaskReadyState::Completed,
        TeamTaskStatus::Failed => TeamTaskReadyState::Failed,
        TeamTaskStatus::Canceled => TeamTaskReadyState::Canceled,
        TeamTaskStatus::Review => TeamTaskReadyState::Review,
        TeamTaskStatus::InProgress => TeamTaskReadyState::Claimed,
        TeamTaskStatus::Pending => {
          if task.blockers.iter().any(|blocker| blocker.active) {
            TeamTaskReadyState::Blocked
          } else {
            TeamTaskReadyState::Ready
          }
        }
      };
      if matches!(task.status, TeamTaskStatus::Review)
        && matches!(task.review_state, TeamTaskReviewState::NotRequested)
      {
        task.review_state = TeamTaskReviewState::Requested;
      }
      if matches!(task.status, TeamTaskStatus::Completed)
        && matches!(task.review_state, TeamTaskReviewState::Requested)
      {
        task.review_state = TeamTaskReviewState::Approved;
      }
    }
  }

  fn sorted_tasks(&self) -> Vec<TeamTask> {
    let mut tasks = self.tasks.values().cloned().collect::<Vec<_>>();
    tasks.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    tasks
  }

  fn sorted_task_edges(&self) -> Vec<TaskEdge> {
    let mut task_edges = self.task_edges.clone();
    task_edges.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    task_edges
  }

  fn sorted_plans(&self) -> Vec<TeamPlan> {
    let mut plans = self.plans.values().cloned().collect::<Vec<_>>();
    plans.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    plans
  }

  fn unread_count_for(&self, thread_id: &str) -> usize {
    let now = Utc::now().timestamp();
    self
      .messages
      .iter()
      .filter(|message| {
        message.is_visible_to(thread_id)
          && !message.is_expired(now)
          && !message.seen_by.contains(thread_id)
      })
      .count()
  }
}
