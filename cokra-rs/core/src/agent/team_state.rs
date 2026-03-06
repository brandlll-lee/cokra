use std::collections::HashMap;
use std::collections::HashSet;

use chrono::Utc;
use uuid::Uuid;

use cokra_protocol::AgentStatus;
use cokra_protocol::TeamMember;
use cokra_protocol::TeamMessage;
use cokra_protocol::TeamSnapshot;
use cokra_protocol::TeamTask;
use cokra_protocol::TeamTaskStatus;

use crate::thread_manager::ThreadInfo;

#[derive(Debug, Clone)]
struct StoredMessage {
  id: String,
  sender_thread_id: String,
  recipient_thread_id: Option<String>,
  message: String,
  created_at: i64,
  seen_by: HashSet<String>,
}

#[derive(Debug, Default)]
pub(crate) struct TeamState {
  tasks: HashMap<String, TeamTask>,
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
      unread_counts,
    }
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

  pub(crate) fn post_message(
    &mut self,
    sender_thread_id: String,
    recipient_thread_id: Option<String>,
    message: String,
  ) -> TeamMessage {
    let mut seen_by = HashSet::new();
    seen_by.insert(sender_thread_id.clone());
    let stored = StoredMessage {
      id: Uuid::new_v4().to_string(),
      sender_thread_id,
      recipient_thread_id,
      message,
      created_at: Utc::now().timestamp(),
      seen_by,
    };
    let out = TeamMessage {
      id: stored.id.clone(),
      sender_thread_id: stored.sender_thread_id.clone(),
      recipient_thread_id: stored.recipient_thread_id.clone(),
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
          None => true,
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
          message: message.message.clone(),
          created_at: message.created_at,
          unread,
        }
      })
      .collect()
  }

  fn sorted_tasks(&self) -> Vec<TeamTask> {
    let mut tasks = self.tasks.values().cloned().collect::<Vec<_>>();
    tasks.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    tasks
  }

  fn unread_count_for(&self, thread_id: &str) -> usize {
    self
      .messages
      .iter()
      .filter(|message| match &message.recipient_thread_id {
        Some(recipient_thread_id) => recipient_thread_id == thread_id,
        None => true,
      })
      .filter(|message| !message.seen_by.contains(thread_id))
      .count()
  }
}
