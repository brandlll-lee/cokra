use std::collections::HashMap;
use std::collections::HashSet;

use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

use cokra_protocol::AgentStatus;
use cokra_protocol::TeamMember;
use cokra_protocol::TeamMessage;
use cokra_protocol::TeamMessageKind;
use cokra_protocol::TeamPlan;
use cokra_protocol::TeamPlanStatus;
use cokra_protocol::TeamSnapshot;
use cokra_protocol::TeamTask;
use cokra_protocol::TeamTaskStatus;

use crate::thread_manager::ThreadInfo;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMessage {
  id: String,
  sender_thread_id: String,
  recipient_thread_id: Option<String>,
  #[serde(default)]
  kind: TeamMessageKind,
  #[serde(default)]
  route_key: Option<String>,
  #[serde(default)]
  claimed_by_thread_id: Option<String>,
  message: String,
  created_at: i64,
  seen_by: HashSet<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct TeamState {
  #[serde(default)]
  tasks: HashMap<String, TeamTask>,
  #[serde(default)]
  plans: HashMap<String, TeamPlan>,
  #[serde(default)]
  messages: Vec<StoredMessage>,
}

impl TeamState {
  pub(crate) fn snapshot(
    &self,
    root_thread_id: String,
    threads: Vec<ThreadInfo>,
    statuses: HashMap<String, AgentStatus>,
  ) -> TeamSnapshot {
    let members = threads
      .into_iter()
      .map(|thread| TeamMember {
        thread_id: thread.thread_id.to_string(),
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
      plans,
      unread_counts,
    }
  }

  pub(crate) fn submit_plan(
    &mut self,
    author_thread_id: String,
    summary: String,
    steps: Vec<String>,
    requires_approval: bool,
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
    assignee_thread_id: Option<String>,
  ) -> TeamTask {
    let now = Utc::now().timestamp();
    let task = TeamTask {
      id: Uuid::new_v4().to_string(),
      title,
      details,
      status: TeamTaskStatus::Pending,
      assignee_thread_id,
      created_at: now,
      updated_at: now,
      notes: Vec::new(),
    };
    self.tasks.insert(task.id.clone(), task.clone());
    task
  }

  pub(crate) fn update_task(
    &mut self,
    task_id: &str,
    status: Option<TeamTaskStatus>,
    assignee_thread_id: Option<Option<String>>,
    note: Option<String>,
  ) -> Option<TeamTask> {
    let task = self.tasks.get_mut(task_id)?;
    if let Some(status) = status {
      task.status = status;
    }
    if let Some(assignee_thread_id) = assignee_thread_id {
      task.assignee_thread_id = assignee_thread_id;
    }
    if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
      task.notes.push(note);
    }
    task.updated_at = Utc::now().timestamp();
    Some(task.clone())
  }

  pub(crate) fn assign_task(
    &mut self,
    task_id: &str,
    assignee_thread_id: String,
    note: Option<String>,
  ) -> Option<TeamTask> {
    let task = self.tasks.get_mut(task_id)?;
    task.assignee_thread_id = Some(assignee_thread_id);
    if matches!(task.status, TeamTaskStatus::Pending) {
      task.status = TeamTaskStatus::InProgress;
    }
    if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
      task.notes.push(note);
    }
    task.updated_at = Utc::now().timestamp();
    Some(task.clone())
  }

  pub(crate) fn handoff_task(
    &mut self,
    task_id: &str,
    to_thread_id: String,
    note: Option<String>,
    review_mode: bool,
  ) -> Option<TeamTask> {
    let task = self.tasks.get_mut(task_id)?;
    task.assignee_thread_id = Some(to_thread_id);
    task.status = if review_mode {
      TeamTaskStatus::Review
    } else {
      TeamTaskStatus::Pending
    };
    if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
      task.notes.push(note);
    }
    task.updated_at = Utc::now().timestamp();
    Some(task.clone())
  }

  pub(crate) fn claim_next_task(&mut self, claimer_thread_id: &str) -> Option<TeamTask> {
    let next_id = self
      .tasks
      .values()
      .filter(|task| {
        matches!(
          task.status,
          TeamTaskStatus::Pending | TeamTaskStatus::Review
        ) && task
          .assignee_thread_id
          .as_deref()
          .is_none_or(|assignee| assignee == claimer_thread_id)
      })
      .min_by_key(|task| task.created_at)
      .map(|task| task.id.clone())?;

    let task = self.tasks.get_mut(&next_id)?;
    task.assignee_thread_id = Some(claimer_thread_id.to_string());
    if matches!(
      task.status,
      TeamTaskStatus::Pending | TeamTaskStatus::Review
    ) {
      task.status = TeamTaskStatus::InProgress;
    }
    task.updated_at = Utc::now().timestamp();
    Some(task.clone())
  }

  pub(crate) fn post_message(
    &mut self,
    sender_thread_id: String,
    recipient_thread_id: Option<String>,
    kind: TeamMessageKind,
    route_key: Option<String>,
    message: String,
  ) -> TeamMessage {
    let mut seen_by = HashSet::new();
    seen_by.insert(sender_thread_id.clone());
    let stored = StoredMessage {
      id: Uuid::new_v4().to_string(),
      sender_thread_id,
      recipient_thread_id,
      kind: kind.clone(),
      route_key: route_key.clone(),
      claimed_by_thread_id: None,
      message,
      created_at: Utc::now().timestamp(),
      seen_by,
    };
    let out = TeamMessage {
      id: stored.id.clone(),
      sender_thread_id: stored.sender_thread_id.clone(),
      recipient_thread_id: stored.recipient_thread_id.clone(),
      kind,
      route_key,
      claimed_by_thread_id: None,
      message: stored.message.clone(),
      created_at: stored.created_at,
      unread: false,
    };
    self.messages.push(stored);
    out
  }

  pub(crate) fn read_messages(
    &mut self,
    reader_thread_id: &str,
    unread_only: bool,
  ) -> Vec<TeamMessage> {
    self
      .messages
      .iter_mut()
      .filter(|message| {
        let visible = match &message.recipient_thread_id {
          Some(recipient_thread_id) => {
            recipient_thread_id == reader_thread_id || message.sender_thread_id == reader_thread_id
          }
          None => match message.kind {
            TeamMessageKind::Queue => {
              message.claimed_by_thread_id.as_deref() == Some(reader_thread_id)
                || message.sender_thread_id == reader_thread_id
            }
            _ => true,
          },
        };
        if !visible {
          return false;
        }
        if unread_only {
          return !message.seen_by.contains(reader_thread_id);
        }
        true
      })
      .map(|message| {
        let unread = !message.seen_by.contains(reader_thread_id);
        message.seen_by.insert(reader_thread_id.to_string());
        TeamMessage {
          id: message.id.clone(),
          sender_thread_id: message.sender_thread_id.clone(),
          recipient_thread_id: message.recipient_thread_id.clone(),
          kind: message.kind.clone(),
          route_key: message.route_key.clone(),
          claimed_by_thread_id: message.claimed_by_thread_id.clone(),
          message: message.message.clone(),
          created_at: message.created_at,
          unread,
        }
      })
      .collect()
  }

  pub(crate) fn claim_queue_messages(
    &mut self,
    claimer_thread_id: &str,
    queue_name: &str,
    limit: usize,
  ) -> Vec<TeamMessage> {
    self
      .messages
      .iter_mut()
      .filter(|message| {
        message.kind == TeamMessageKind::Queue
          && message.route_key.as_deref() == Some(queue_name)
          && message.claimed_by_thread_id.is_none()
      })
      .take(limit)
      .map(|message| {
        message.claimed_by_thread_id = Some(claimer_thread_id.to_string());
        let unread = !message.seen_by.contains(claimer_thread_id);
        message.seen_by.insert(claimer_thread_id.to_string());
        TeamMessage {
          id: message.id.clone(),
          sender_thread_id: message.sender_thread_id.clone(),
          recipient_thread_id: message.recipient_thread_id.clone(),
          kind: message.kind.clone(),
          route_key: message.route_key.clone(),
          claimed_by_thread_id: message.claimed_by_thread_id.clone(),
          message: message.message.clone(),
          created_at: message.created_at,
          unread,
        }
      })
      .collect()
  }

  pub(crate) fn clear(&mut self) {
    self.tasks.clear();
    self.plans.clear();
    self.messages.clear();
  }

  fn sorted_tasks(&self) -> Vec<TeamTask> {
    let mut tasks = self.tasks.values().cloned().collect::<Vec<_>>();
    tasks.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    tasks
  }

  fn sorted_plans(&self) -> Vec<TeamPlan> {
    let mut plans = self.plans.values().cloned().collect::<Vec<_>>();
    plans.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    plans
  }

  fn unread_count_for(&self, thread_id: &str) -> usize {
    self
      .messages
      .iter()
      .filter(|message| match &message.recipient_thread_id {
        Some(recipient_thread_id) => recipient_thread_id == thread_id,
        None => match message.kind {
          TeamMessageKind::Queue => false,
          _ => true,
        },
      })
      .filter(|message| !message.seen_by.contains(thread_id))
      .count()
  }
}
