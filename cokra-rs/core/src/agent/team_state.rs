use std::collections::HashMap;
use std::collections::HashSet;

use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

use cokra_protocol::CollabAgentWaitState;
use cokra_protocol::OwnershipAccessMode;
use cokra_protocol::OwnershipLease;
use cokra_protocol::OwnershipScope;
use cokra_protocol::OwnershipScopeKind;
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
      .is_some_and(|expires_at| expires_at > 0 && expires_at <= now)
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
  task_scope_memory: HashMap<String, Vec<ScopeRequest>>,
  #[serde(default)]
  ownership_leases: HashMap<String, OwnershipLease>,
  #[serde(default)]
  plans: HashMap<String, TeamPlan>,
  #[serde(default)]
  messages: Vec<StoredMessage>,
  #[serde(default)]
  mailbox_version: u64,
}

#[derive(Debug, Clone)]
pub(crate) enum WriteOwnershipFailure {
  Blocked {
    path: String,
    lease: OwnershipLease,
  },
  MissingClaim {
    path: String,
  },
  ClaimedByOther {
    path: String,
    task_id: String,
    owner_thread_id: String,
  },
  AmbiguousClaim {
    path: String,
    task_ids: Vec<String>,
  },
}

impl TeamState {
  pub(crate) fn snapshot(
    &mut self,
    root_thread_id: String,
    threads: Vec<ThreadInfo>,
    statuses: HashMap<String, CollabAgentWaitState>,
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
        state: statuses
          .get(&thread.thread_id.to_string())
          .cloned()
          .unwrap_or_default(),
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
    let recent_messages = self.sorted_recent_messages(&root_thread_id, 12);
    let ownership_leases = self.sorted_ownership_leases();

    TeamSnapshot {
      root_thread_id,
      members,
      tasks,
      task_edges: self.sorted_task_edges(),
      plans,
      unread_counts,
      mailbox_version: self.mailbox_version,
      recent_messages,
      ownership_leases,
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
    scope_policy_override: bool,
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
      scope_policy_override,
      assignee_thread_id,
      reviewer_thread_id: None,
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
    self.remember_task_scopes_from_task(&task_id);
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
    reviewer_thread_id: Option<Option<String>>,
    note: Option<String>,
    requested_scopes: Option<Vec<ScopeRequest>>,
    granted_scopes: Option<Vec<ScopeRequest>>,
    review_state: Option<TeamTaskReviewState>,
    scope_policy_override: Option<bool>,
  ) -> Option<TeamTask> {
    let status_changed = status.is_some();
    let assignee_changed = assignee_thread_id.is_some();
    let owner_changed = owner_thread_id.is_some();
    let reviewer_changed = reviewer_thread_id.is_some();
    let requested_scopes_changed = requested_scopes.is_some();
    let granted_scopes_changed = granted_scopes.is_some();
    let review_state_changed = review_state.is_some();
    {
      let task = self.tasks.get_mut(task_id)?;
      if let Some(status) = status {
        task.status = status;
      }
      if let Some(assignee_thread_id) = assignee_thread_id {
        task.assignee_thread_id = assignee_thread_id;
      }
      if let Some(owner_thread_id) = owner_thread_id {
        task.owner_thread_id = owner_thread_id;
      }
      if let Some(reviewer_thread_id) = reviewer_thread_id {
        task.reviewer_thread_id = reviewer_thread_id;
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
      if let Some(scope_policy_override) = scope_policy_override {
        task.scope_policy_override = scope_policy_override;
      }
      if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
        task.notes.push(note);
      }
      task.updated_at = Utc::now().timestamp();
    }
    self.remember_task_scopes_from_task(task_id);
    if !granted_scopes_changed
      && let Some(task) = self.tasks.get_mut(task_id)
      && !review_state_changed
      && matches!(task.status, TeamTaskStatus::Pending)
    {
      task.review_state = TeamTaskReviewState::NotRequested;
    }
    if status_changed
      || assignee_changed
      || owner_changed
      || reviewer_changed
      || requested_scopes_changed
    {
      self.refresh_leases_after_update(task_id);
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
      task.reviewer_thread_id = None;
      task.status = TeamTaskStatus::Pending;
      task.review_state = TeamTaskReviewState::NotRequested;
      task.granted_scopes.clear();
      if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
        task.notes.push(note);
      }
      task.updated_at = Utc::now().timestamp();
    }
    self.release_task_leases_internal(task_id);
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
    let effective_scopes = self.effective_scope_requests(task_id);
    self.remember_task_scopes(task_id, &effective_scopes);
    {
      let task = self.tasks.get_mut(task_id)?;
      task.assignee_thread_id = Some(to_thread_id.clone());
      task.owner_thread_id = Some(to_thread_id.clone());
      task.reviewer_thread_id = review_mode.then_some(to_thread_id.clone());
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
        task.granted_scopes.clear();
      }
      if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
        task.notes.push(note);
      }
      task.updated_at = Utc::now().timestamp();
    }
    self.apply_task_leases(
      task_id,
      &to_thread_id,
      review_mode.then_some(OwnershipAccessMode::Review),
    );
    self.recompute_task_graph();
    self.tasks.get(task_id).cloned()
  }

  pub(crate) fn claim_task(
    &mut self,
    task_id: &str,
    claimer_thread_id: String,
    note: Option<String>,
  ) -> Option<TeamTask> {
    if !self.can_claim_task(task_id, &claimer_thread_id, true) {
      return None;
    }
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
    if !self.can_claim_task(task_id, &claimer_thread_id, true) {
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
    if !self
      .task_edges
      .iter()
      .any(|edge| edge.from_task_id == dependency_task_id && edge.to_task_id == task_id)
    {
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
    self
      .task_edges
      .retain(|edge| !(edge.from_task_id == dependency_task_id && edge.to_task_id == task_id));
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
    let expires_at = expires_at.filter(|expires_at| *expires_at > 0);
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

  pub(crate) fn peek_messages(
    &mut self,
    reader_thread_id: &str,
    unread_only: bool,
  ) -> Vec<TeamMessage> {
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
        .then_some((
          index,
          Self::message_priority_rank(&message.priority),
          message.created_at,
        ))
      })
      .collect::<Vec<_>>();
    eligible_indexes.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.2.cmp(&right.2)));
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

  pub(crate) fn release_task_leases(&mut self, task_id: &str) -> Option<TeamTask> {
    let now = Utc::now().timestamp();
    {
      let task = self.tasks.get_mut(task_id)?;
      task.updated_at = now;
    }
    self.release_task_leases_internal(task_id);
    self.recompute_task_graph();
    self.tasks.get(task_id).cloned()
  }

  pub(crate) fn force_release_lease(&mut self, lease_id: &str) -> Option<OwnershipLease> {
    let lease = self.ownership_leases.remove(lease_id)?;
    if let Some(task) = self.tasks.get_mut(&lease.task_id) {
      task.updated_at = Utc::now().timestamp();
    }
    self.recompute_task_graph();
    Some(lease)
  }

  pub(crate) fn touch_thread_leases(&mut self, thread_id: &str) -> usize {
    let now = Utc::now().timestamp();
    let mut touched = 0usize;
    for lease in self.ownership_leases.values_mut() {
      if lease.owner_thread_id == thread_id {
        lease.heartbeat_at = now;
        touched += 1;
      }
    }
    touched
  }

  pub(crate) fn release_thread_leases(&mut self, thread_id: &str) -> usize {
    let lease_ids = self
      .ownership_leases
      .values()
      .filter(|lease| lease.owner_thread_id == thread_id)
      .map(|lease| lease.id.clone())
      .collect::<Vec<_>>();
    let mut released = 0usize;
    let now = Utc::now().timestamp();
    for lease_id in lease_ids {
      if let Some(lease) = self.ownership_leases.remove(&lease_id) {
        if let Some(task) = self.tasks.get_mut(&lease.task_id) {
          task.updated_at = now;
        }
        released += 1;
      }
    }
    if released > 0 {
      self.recompute_task_graph();
    }
    released
  }

  pub(crate) fn cleanup_stale_leases(
    &mut self,
    active_thread_ids: &HashSet<String>,
    stale_before: i64,
  ) -> Vec<OwnershipLease> {
    let now = Utc::now().timestamp();
    let stale_ids = self
      .ownership_leases
      .values()
      .filter(|lease| {
        lease.expires_at.is_some_and(|expires_at| expires_at <= now)
          || (!active_thread_ids.contains(&lease.owner_thread_id)
            && lease.heartbeat_at <= stale_before)
      })
      .map(|lease| lease.id.clone())
      .collect::<Vec<_>>();
    let mut removed = Vec::new();
    for lease_id in stale_ids {
      if let Some(lease) = self.ownership_leases.remove(&lease_id) {
        if let Some(task) = self.tasks.get_mut(&lease.task_id) {
          task.updated_at = now;
        }
        removed.push(lease);
      }
    }
    if !removed.is_empty() {
      self.recompute_task_graph();
    }
    removed
  }

  pub(crate) fn validate_write_paths(
    &self,
    thread_id: &str,
    paths: &[String],
  ) -> Result<(), Vec<(String, Option<OwnershipLease>)>> {
    let mut conflicts = Vec::new();
    for path in paths {
      let owned = self.ownership_leases.values().any(|lease| {
        lease.owner_thread_id == thread_id
          && matches!(lease.access, OwnershipAccessMode::ExclusiveWrite)
          && Self::scopes_overlap(
            &OwnershipScopeKind::File,
            path,
            &lease.scope.kind,
            &lease.scope.path,
          )
      });
      if owned {
        continue;
      }
      let blocking_lease = self
        .ownership_leases
        .values()
        .find(|lease| {
          lease.owner_thread_id != thread_id
            && Self::scopes_overlap(
              &OwnershipScopeKind::File,
              path,
              &lease.scope.kind,
              &lease.scope.path,
            )
        })
        .cloned();
      conflicts.push((path.clone(), blocking_lease));
    }
    if conflicts.is_empty() {
      Ok(())
    } else {
      Err(conflicts)
    }
  }

  pub(crate) fn ensure_write_paths_owned(
    &mut self,
    thread_id: &str,
    paths: &[String],
  ) -> Result<usize, Vec<WriteOwnershipFailure>> {
    let mut grants = Vec::<(String, String)>::new();
    let mut failures = Vec::<WriteOwnershipFailure>::new();

    for path in paths {
      let already_owned = self.ownership_leases.values().any(|lease| {
        lease.owner_thread_id == thread_id
          && matches!(lease.access, OwnershipAccessMode::ExclusiveWrite)
          && Self::scopes_overlap(
            &OwnershipScopeKind::File,
            path,
            &lease.scope.kind,
            &lease.scope.path,
          )
      });
      if already_owned {
        continue;
      }

      if let Some(lease) = self
        .ownership_leases
        .values()
        .find(|lease| {
          lease.owner_thread_id != thread_id
            && Self::scopes_overlap(
              &OwnershipScopeKind::File,
              path,
              &lease.scope.kind,
              &lease.scope.path,
            )
        })
        .cloned()
      {
        failures.push(WriteOwnershipFailure::Blocked {
          path: path.clone(),
          lease,
        });
        continue;
      }

      match self.select_write_task_for_path(thread_id, path) {
        Ok(Some(task_id)) => grants.push((path.clone(), task_id)),
        Ok(None) => {
          let mut candidates = self
            .tasks
            .values()
            .filter(|task| matches!(task.status, TeamTaskStatus::InProgress))
            .filter(|task| {
              task
                .owner_thread_id
                .as_deref()
                .is_some_and(|owner_thread_id| owner_thread_id != thread_id)
            })
            .filter_map(|task| {
              let owner_thread_id = task.owner_thread_id.as_deref()?;
              let best_rank = self
                .effective_scope_requests(&task.id)
                .into_iter()
                .filter(|scope| matches!(scope.access, OwnershipAccessMode::ExclusiveWrite))
                .filter(|scope| {
                  Self::scopes_overlap(&scope.kind, &scope.path, &OwnershipScopeKind::File, path)
                })
                .map(|scope| Self::write_scope_rank(&scope, path))
                .max()?;
              Some((
                task.id.clone(),
                owner_thread_id.to_string(),
                best_rank,
                task.updated_at,
              ))
            })
            .collect::<Vec<_>>();

          if candidates.is_empty() {
            failures.push(WriteOwnershipFailure::MissingClaim { path: path.clone() });
          } else {
            candidates.sort_by(|left, right| {
              right
                .2
                .cmp(&left.2)
                .then_with(|| right.3.cmp(&left.3))
                .then_with(|| left.0.cmp(&right.0))
            });
            let best_rank = candidates[0].2;
            let top = candidates
              .into_iter()
              .filter(|candidate| candidate.2 == best_rank)
              .collect::<Vec<_>>();
            if top.len() == 1 {
              failures.push(WriteOwnershipFailure::ClaimedByOther {
                path: path.clone(),
                task_id: top[0].0.clone(),
                owner_thread_id: top[0].1.clone(),
              });
            } else {
              failures.push(WriteOwnershipFailure::AmbiguousClaim {
                path: path.clone(),
                task_ids: top.into_iter().map(|candidate| candidate.0).collect(),
              });
            }
          }
        }
        Err(task_ids) => failures.push(WriteOwnershipFailure::AmbiguousClaim {
          path: path.clone(),
          task_ids,
        }),
      }
    }

    if !failures.is_empty() {
      return Err(failures);
    }

    let now = Utc::now().timestamp();
    let mut inserted = 0usize;
    for (path, task_id) in grants {
      let duplicate = self.ownership_leases.values().any(|lease| {
        lease.task_id == task_id
          && lease.owner_thread_id == thread_id
          && matches!(lease.access, OwnershipAccessMode::ExclusiveWrite)
          && lease.scope.kind == OwnershipScopeKind::File
          && lease.scope.path == path
      });
      if duplicate {
        continue;
      }
      let lease = OwnershipLease {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.clone(),
        owner_thread_id: thread_id.to_string(),
        scope: OwnershipScope {
          kind: OwnershipScopeKind::File,
          path,
        },
        access: OwnershipAccessMode::ExclusiveWrite,
        acquired_at: now,
        heartbeat_at: now,
        expires_at: None,
      };
      self.ownership_leases.insert(lease.id.clone(), lease);
      if let Some(task) = self.tasks.get_mut(&task_id) {
        task.updated_at = now;
      }
      inserted += 1;
    }

    if inserted > 0 {
      self.recompute_task_graph();
    }

    Ok(inserted)
  }

  pub(crate) fn clear(&mut self) {
    self.tasks.clear();
    self.task_edges.clear();
    self.task_scope_memory.clear();
    self.ownership_leases.clear();
    self.plans.clear();
    self.messages.clear();
    self.mailbox_version = 0;
  }

  pub(crate) fn open_task_count_for_thread(&self, thread_id: &str) -> usize {
    self
      .tasks
      .values()
      .filter(|task| {
        (task.owner_thread_id.as_deref() == Some(thread_id)
          || task.assignee_thread_id.as_deref() == Some(thread_id)
          || task.reviewer_thread_id.as_deref() == Some(thread_id))
          && !matches!(
            task.status,
            TeamTaskStatus::Completed | TeamTaskStatus::Failed | TeamTaskStatus::Canceled
          )
      })
      .count()
  }

  pub(crate) fn task(&self, task_id: &str) -> Option<TeamTask> {
    self.tasks.get(task_id).cloned()
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
    let effective_scopes = self.effective_scope_requests(task_id);
    self.remember_task_scopes(task_id, &effective_scopes);
    {
      let task = self.tasks.get_mut(task_id)?;
      if let Some((claimer_thread_id, note)) = claim {
        task.assignee_thread_id = Some(claimer_thread_id.clone());
        task.owner_thread_id = Some(claimer_thread_id);
        if !matches!(task.status, TeamTaskStatus::Review) {
          task.reviewer_thread_id = None;
        }
        if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
          task.notes.push(note);
        }
      }
      task.status = TeamTaskStatus::InProgress;
      task.review_state = TeamTaskReviewState::NotRequested;
      task.granted_scopes.clear();
      task.updated_at = Utc::now().timestamp();
    }
    self.recompute_task_graph();
    self.tasks.get(task_id).cloned()
  }

  fn refresh_leases_after_update(&mut self, task_id: &str) {
    let Some(task) = self.tasks.get(task_id).cloned() else {
      return;
    };
    match task.status {
      TeamTaskStatus::InProgress => {
        self.release_task_leases_internal(task_id);
      }
      TeamTaskStatus::Review => {
        if let Some(owner_thread_id) = task.owner_thread_id {
          self.apply_task_leases(task_id, &owner_thread_id, Some(OwnershipAccessMode::Review));
        } else {
          self.release_task_leases_internal(task_id);
        }
      }
      TeamTaskStatus::Pending => {
        self.release_task_leases_internal(task_id);
      }
      TeamTaskStatus::Completed | TeamTaskStatus::Failed | TeamTaskStatus::Canceled => {
        self.release_task_leases_internal(task_id);
      }
    }
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

  fn apply_task_leases(
    &mut self,
    task_id: &str,
    owner_thread_id: &str,
    access_override: Option<OwnershipAccessMode>,
  ) {
    let effective_scopes = self.effective_scope_requests(task_id);
    self.remember_task_scopes(task_id, &effective_scopes);
    self.release_task_leases_internal(task_id);
    let now = Utc::now().timestamp();
    for request in effective_scopes {
      let lease = OwnershipLease {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_string(),
        owner_thread_id: owner_thread_id.to_string(),
        scope: OwnershipScope {
          kind: request.kind,
          path: request.path,
        },
        access: access_override.clone().unwrap_or(request.access),
        acquired_at: now,
        heartbeat_at: now,
        expires_at: None,
      };
      self.ownership_leases.insert(lease.id.clone(), lease);
    }
  }

  fn release_task_leases_internal(&mut self, task_id: &str) -> Vec<OwnershipLease> {
    let lease_ids = self
      .ownership_leases
      .values()
      .filter(|lease| lease.task_id == task_id)
      .map(|lease| lease.id.clone())
      .collect::<Vec<_>>();
    let mut removed = Vec::new();
    for lease_id in lease_ids {
      if let Some(lease) = self.ownership_leases.remove(&lease_id) {
        removed.push(lease);
      }
    }
    removed
  }

  fn task_has_active_leases(&self, task_id: &str) -> bool {
    self
      .ownership_leases
      .values()
      .any(|lease| lease.task_id == task_id)
  }

  fn effective_scope_requests(&self, task_id: &str) -> Vec<ScopeRequest> {
    let Some(task) = self.tasks.get(task_id) else {
      return Vec::new();
    };
    Self::resolve_effective_scope_requests(
      &task.requested_scopes,
      self.task_scope_memory.get(task_id).map(Vec::as_slice),
      Some(task.granted_scopes.as_slice()),
    )
  }

  fn remember_task_scopes_from_task(&mut self, task_id: &str) {
    let remembered = self.effective_scope_requests(task_id);
    self.remember_task_scopes(task_id, &remembered);
  }

  fn remember_task_scopes(&mut self, task_id: &str, scopes: &[ScopeRequest]) {
    if scopes.is_empty() {
      return;
    }
    self.task_scope_memory.insert(
      task_id.to_string(),
      Self::scope_templates_from_requests(scopes),
    );
  }

  fn resolve_effective_scope_requests(
    requested_scopes: &[ScopeRequest],
    remembered_scopes: Option<&[ScopeRequest]>,
    granted_scopes: Option<&[ScopeRequest]>,
  ) -> Vec<ScopeRequest> {
    if !requested_scopes.is_empty() {
      return Self::scope_templates_from_requests(requested_scopes);
    }
    if let Some(scopes) = remembered_scopes.filter(|scopes| !scopes.is_empty()) {
      return Self::scope_templates_from_requests(scopes);
    }
    granted_scopes
      .filter(|scopes| !scopes.is_empty())
      .map(Self::scope_templates_from_requests)
      .unwrap_or_default()
  }

  fn scope_templates_from_requests(scopes: &[ScopeRequest]) -> Vec<ScopeRequest> {
    scopes
      .iter()
      .map(|scope| ScopeRequest {
        kind: scope.kind.clone(),
        path: scope.path.clone(),
        access: scope.access.clone(),
        reason: scope
          .reason
          .as_deref()
          .map(str::trim)
          .filter(|reason| !reason.is_empty() && !reason.starts_with("lease "))
          .map(ToString::to_string),
      })
      .collect()
  }

  fn scopes_with_access(scopes: &[ScopeRequest], access: OwnershipAccessMode) -> Vec<ScopeRequest> {
    scopes
      .iter()
      .map(|scope| ScopeRequest {
        kind: scope.kind.clone(),
        path: scope.path.clone(),
        access: access.clone(),
        reason: scope.reason.clone(),
      })
      .collect()
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

  fn can_claim_task(
    &mut self,
    task_id: &str,
    claimer_thread_id: &str,
    requires_ready: bool,
  ) -> bool {
    if requires_ready && !self.is_task_ready(task_id) {
      return false;
    }
    self.tasks.get(task_id).is_some_and(|task| {
      task
        .assignee_thread_id
        .as_deref()
        .is_none_or(|assignee| assignee == claimer_thread_id)
    })
  }

  fn select_write_task_for_path(
    &self,
    thread_id: &str,
    path: &str,
  ) -> Result<Option<String>, Vec<String>> {
    let mut candidates = self
      .tasks
      .values()
      .filter(|task| {
        matches!(task.status, TeamTaskStatus::InProgress)
          && task.owner_thread_id.as_deref() == Some(thread_id)
      })
      .filter_map(|task| {
        let best_rank = self
          .effective_scope_requests(&task.id)
          .into_iter()
          .filter(|scope| matches!(scope.access, OwnershipAccessMode::ExclusiveWrite))
          .filter(|scope| {
            Self::scopes_overlap(&scope.kind, &scope.path, &OwnershipScopeKind::File, path)
          })
          .map(|scope| Self::write_scope_rank(&scope, path))
          .max()?;
        Some((task.id.clone(), best_rank, task.updated_at))
      })
      .collect::<Vec<_>>();

    if candidates.is_empty() {
      return Ok(None);
    }

    candidates.sort_by(|left, right| {
      right
        .1
        .cmp(&left.1)
        .then_with(|| right.2.cmp(&left.2))
        .then_with(|| left.0.cmp(&right.0))
    });
    let best_rank = candidates[0].1;
    let top = candidates
      .into_iter()
      .filter(|candidate| candidate.1 == best_rank)
      .collect::<Vec<_>>();
    if top.len() == 1 {
      return Ok(Some(top[0].0.clone()));
    }

    Err(top.into_iter().map(|candidate| candidate.0).collect())
  }

  fn write_scope_rank(scope: &ScopeRequest, path: &str) -> usize {
    match scope.kind {
      OwnershipScopeKind::File => 10_000,
      OwnershipScopeKind::Directory => 5_000 + scope.path.len(),
      OwnershipScopeKind::Glob => 1_000 + scope.path.len(),
      OwnershipScopeKind::Module => {
        if scope.path == path {
          10_000
        } else {
          0
        }
      }
    }
  }

  fn recompute_task_graph(&mut self) {
    let now = Utc::now().timestamp();
    let status_by_id = self
      .tasks
      .iter()
      .map(|(task_id, task)| (task_id.clone(), task.status.clone()))
      .collect::<HashMap<_, _>>();
    let existing_task_ids = self.tasks.keys().cloned().collect::<HashSet<_>>();
    self
      .task_scope_memory
      .retain(|task_id, _| existing_task_ids.contains(task_id));
    self
      .ownership_leases
      .retain(|_, lease| existing_task_ids.contains(&lease.task_id));
    let leases = self.sorted_ownership_leases();
    let mut granted_scopes_by_task = HashMap::<String, Vec<ScopeRequest>>::new();
    for lease in &leases {
      granted_scopes_by_task
        .entry(lease.task_id.clone())
        .or_default()
        .push(ScopeRequest {
          kind: lease.scope.kind.clone(),
          path: lease.scope.path.clone(),
          access: lease.access.clone(),
          reason: Some(format!("lease {}", lease.id)),
        });
    }

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
    let effective_scope_requests_by_task = self
      .tasks
      .iter()
      .map(|(task_id, task)| {
        (
          task_id.clone(),
          Self::resolve_effective_scope_requests(
            &task.requested_scopes,
            self.task_scope_memory.get(task_id).map(Vec::as_slice),
            granted_scopes_by_task.get(task_id).map(Vec::as_slice),
          ),
        )
      })
      .collect::<HashMap<_, _>>();

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
          let active =
            dependency_status.is_some_and(|status| !matches!(status, TeamTaskStatus::Completed));
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
      let requester_thread_id = task
        .owner_thread_id
        .as_deref()
        .or(task.assignee_thread_id.as_deref());
      blockers.extend(leases.iter().filter_map(|lease| {
        let has_scope_conflict =
          effective_scope_requests_by_task
            .get(task_id)
            .is_some_and(|requests| {
              requests
                .iter()
                .any(|request| Self::scope_request_conflicts_with_lease(request, lease))
            });
        if lease.task_id == *task_id
          || requester_thread_id == Some(lease.owner_thread_id.as_str())
          || !has_scope_conflict
        {
          return None;
        }
        Some(TaskBlocker {
          id: format!("lease:{task_id}:{}", lease.id),
          kind: TaskBlockerKind::LeaseConflict,
          blocking_task_id: Some(lease.task_id.clone()),
          reason: format!(
            "scope {} locked by {} ({})",
            lease.scope.path,
            lease.owner_thread_id,
            Self::access_label(&lease.access)
          ),
          active: true,
          created_at: lease.acquired_at,
          cleared_at: None,
        })
      }));
      task.blockers = blockers;
      task.granted_scopes = granted_scopes_by_task.remove(task_id).unwrap_or_default();
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

  fn sorted_recent_messages(&self, observer_thread_id: &str, limit: usize) -> Vec<TeamMessage> {
    let now = Utc::now().timestamp();
    let mut messages = self
      .messages
      .iter()
      .filter(|message| !message.is_expired(now))
      .map(|message| message.to_team_message(observer_thread_id))
      .collect::<Vec<_>>();
    messages.sort_by(|left, right| {
      right.created_at.cmp(&left.created_at).then_with(|| {
        Self::message_priority_rank(&right.priority)
          .cmp(&Self::message_priority_rank(&left.priority))
      })
    });
    messages.truncate(limit);
    messages
  }

  fn sorted_ownership_leases(&self) -> Vec<OwnershipLease> {
    let mut ownership_leases = self.ownership_leases.values().cloned().collect::<Vec<_>>();
    ownership_leases.sort_by(|left, right| {
      left
        .scope
        .path
        .cmp(&right.scope.path)
        .then_with(|| left.acquired_at.cmp(&right.acquired_at))
    });
    ownership_leases
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

  pub(crate) fn unread_count_for(&self, thread_id: &str) -> usize {
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

  fn scope_request_conflicts_with_lease(request: &ScopeRequest, lease: &OwnershipLease) -> bool {
    if !Self::scopes_overlap(
      &request.kind,
      &request.path,
      &lease.scope.kind,
      &lease.scope.path,
    ) {
      return false;
    }
    !Self::accesses_compatible(&request.access, &lease.access)
  }

  fn accesses_compatible(
    requested_access: &OwnershipAccessMode,
    active_access: &OwnershipAccessMode,
  ) -> bool {
    matches!(requested_access, OwnershipAccessMode::SharedRead)
      || matches!(active_access, OwnershipAccessMode::SharedRead)
  }

  fn scopes_overlap(
    left_kind: &OwnershipScopeKind,
    left_path: &str,
    right_kind: &OwnershipScopeKind,
    right_path: &str,
  ) -> bool {
    match (left_kind, right_kind) {
      (OwnershipScopeKind::Module, OwnershipScopeKind::Module) => left_path == right_path,
      (OwnershipScopeKind::Module, _) | (_, OwnershipScopeKind::Module) => false,
      (OwnershipScopeKind::Glob, _) => Self::glob_matches(left_path, right_path),
      (_, OwnershipScopeKind::Glob) => Self::glob_matches(right_path, left_path),
      _ => {
        left_path == right_path
          || Self::path_prefix_matches(left_path, right_path)
          || Self::path_prefix_matches(right_path, left_path)
      }
    }
  }

  fn glob_matches(pattern: &str, candidate: &str) -> bool {
    glob::Pattern::new(pattern).is_ok_and(|compiled| compiled.matches(candidate))
  }

  fn path_prefix_matches(path: &str, prefix: &str) -> bool {
    path == prefix
      || path
        .strip_prefix(prefix)
        .is_some_and(|suffix| suffix.starts_with('/') || suffix.starts_with('\\'))
  }

  fn access_label(access: &OwnershipAccessMode) -> &'static str {
    match access {
      OwnershipAccessMode::SharedRead => "shared-read",
      OwnershipAccessMode::ExclusiveWrite => "exclusive-write",
      OwnershipAccessMode::Review => "review",
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn directory_scope(path: &str) -> ScopeRequest {
    ScopeRequest {
      kind: OwnershipScopeKind::Directory,
      path: path.to_string(),
      access: OwnershipAccessMode::ExclusiveWrite,
      reason: Some("test".to_string()),
    }
  }

  #[test]
  fn claim_ready_task_keeps_claimed_task_in_progress_without_eager_leases() {
    let mut state = TeamState::default();
    let task = state.create_task(
      "Implement feature".to_string(),
      None,
      Some("impl-thread".to_string()),
      Some("impl-thread".to_string()),
      None,
      vec![directory_scope("/repo")],
      None,
      false,
    );

    let claimed = state
      .claim_ready_task(&task.id, "impl-thread".to_string(), None)
      .expect("task should be claimed");

    assert_eq!(claimed.status, TeamTaskStatus::InProgress);
    assert!(claimed.granted_scopes.is_empty());
    assert!(state.sorted_ownership_leases().is_empty());
  }

  #[test]
  fn ensure_write_paths_owned_grants_precise_file_leases() {
    let mut state = TeamState::default();
    let task = state.create_task(
      "Implement feature".to_string(),
      None,
      Some("impl-thread".to_string()),
      Some("impl-thread".to_string()),
      None,
      vec![directory_scope("/repo")],
      None,
      false,
    );
    let _ = state
      .claim_ready_task(&task.id, "impl-thread".to_string(), None)
      .expect("task should be claimed");

    let inserted = state
      .ensure_write_paths_owned("impl-thread", &["/repo/src/a.rs".to_string()])
      .expect("write path should acquire a precise lease");
    assert_eq!(inserted, 1);

    let leases = state.sorted_ownership_leases();
    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0].scope.kind, OwnershipScopeKind::File);
    assert_eq!(leases[0].scope.path, "/repo/src/a.rs");
    assert_eq!(leases[0].owner_thread_id, "impl-thread");

    let second = state
      .ensure_write_paths_owned(
        "impl-thread",
        &["/repo/src/a.rs".to_string(), "/repo/src/b.rs".to_string()],
      )
      .expect("second distinct file should acquire an additional precise lease");
    assert_eq!(second, 1);

    let leases = state.sorted_ownership_leases();
    assert_eq!(leases.len(), 2);
    assert!(
      leases
        .iter()
        .any(|lease| lease.scope.path == "/repo/src/a.rs")
    );
    assert!(
      leases
        .iter()
        .any(|lease| lease.scope.path == "/repo/src/b.rs")
    );
    assert!(
      leases
        .iter()
        .all(|lease| lease.scope.kind == OwnershipScopeKind::File)
    );

    assert!(
      state
        .validate_write_paths("other-thread", &["/repo/src/b.rs".to_string()])
        .is_err(),
      "conflicting writers should still be blocked on the claimed file"
    );
  }
}
