use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use cokra_protocol::EventMsg;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::time::timeout;
use uuid::Uuid;

use cokra_config::Config;
use cokra_protocol::CollabAgentLifecycle;
use cokra_protocol::CollabAgentRef;
use cokra_protocol::CollabAgentStateChangedEvent;
use cokra_protocol::CollabAgentStatusEntry;
use cokra_protocol::CollabAgentWaitState;
use cokra_protocol::CollabTurnOutcome;
use cokra_protocol::OwnershipAccessMode;
use cokra_protocol::OwnershipLease;
use cokra_protocol::OwnershipScopeKind;
use cokra_protocol::ScopeRequest;
use cokra_protocol::TeamMessage;
use cokra_protocol::TeamMessageDeliveryMode;
use cokra_protocol::TeamMessageKind;
use cokra_protocol::TeamMessagePriority;
use cokra_protocol::TeamPlan;
use cokra_protocol::TeamSnapshot;
use cokra_protocol::TeamTask;
use cokra_protocol::TeamTaskReviewState;
use cokra_protocol::TeamTaskStatus;
use cokra_protocol::ThreadId;
use cokra_protocol::WorkflowRun;
use cokra_protocol::WorkflowRuntimeSnapshot;
use cokra_state::StateDb;

use crate::agent::AgentControl;
use crate::agent::Turn;
use crate::model::ModelClient;
use crate::session::Session;
use crate::thread_manager::ThreadInfo;
use crate::thread_manager::ThreadManagerState;
use crate::tools::build_default_tooling_with_cwd;

use self::team_runs::TeamRunState;
use super::Guards;
use super::team_state::TeamState;

const CHILD_COMMAND_CHANNEL_CAPACITY: usize = 32;
const CHILD_EVENT_CHANNEL_CAPACITY: usize = 512;
const LEASE_STALE_AFTER_SECS: i64 = 300;

#[derive(Debug)]
enum ChildCommand {
  UserTurn { message: String },
  Shutdown,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ManagedAgentState {
  pub(crate) lifecycle: CollabAgentLifecycle,
  pub(crate) turn_outcome: CollabTurnOutcome,
  pub(crate) last_turn_summary: Option<String>,
  pub(crate) attention_reason: Option<String>,
  pub(crate) pending_wake_count: usize,
  pub(crate) scheduled_generation: u64,
  pub(crate) inflight_generation: u64,
  pub(crate) settled_generation: u64,
}

impl ManagedAgentState {
  pub(crate) fn wait_state(&self) -> CollabAgentWaitState {
    CollabAgentWaitState {
      lifecycle: self.lifecycle.clone(),
      turn_outcome: self.turn_outcome.clone(),
      last_turn_summary: self.last_turn_summary.clone(),
      attention_reason: self.attention_reason.clone(),
      pending_wake_count: self.pending_wake_count,
    }
  }
}

#[derive(Debug, Clone)]
enum WakeReason {
  UserInput,
  TaskAssigned { task_id: String },
  TaskHandedOff { task_id: String },
  ReviewRequested { task_id: String },
  MailboxUnread,
}

#[derive(Debug, Clone)]
struct WakeRequest {
  message: String,
  reason: WakeReason,
}

#[derive(Clone)]
pub(crate) struct ManagedAgentHandle {
  thread_id: ThreadId,
  session: Arc<Session>,
  tx_cmd: mpsc::Sender<ChildCommand>,
  state_tx: watch::Sender<ManagedAgentState>,
  state_rx: watch::Receiver<ManagedAgentState>,
  wake_queue: Arc<Mutex<VecDeque<WakeRequest>>>,
}

impl ManagedAgentHandle {
  pub(crate) async fn send_turn_now(&self, message: String) -> anyhow::Result<()> {
    self
      .tx_cmd
      .send(ChildCommand::UserTurn { message })
      .await
      .map_err(|_| anyhow::anyhow!("agent loop terminated"))
  }

  pub(crate) async fn shutdown(&self) -> anyhow::Result<()> {
    self
      .tx_cmd
      .send(ChildCommand::Shutdown)
      .await
      .map_err(|_| anyhow::anyhow!("agent loop terminated"))
  }

  pub(crate) fn subscribe_state(&self) -> watch::Receiver<ManagedAgentState> {
    self.state_rx.clone()
  }

  pub(crate) fn thread_id(&self) -> &ThreadId {
    &self.thread_id
  }

  pub(crate) fn state(&self) -> ManagedAgentState {
    self.state_rx.borrow().clone()
  }

  pub(crate) fn update_state<F>(&self, mutator: F) -> ManagedAgentState
  where
    F: FnOnce(&mut ManagedAgentState),
  {
    let mut state = self.state();
    mutator(&mut state);
    let _ = self.state_tx.send(state.clone());
    state
  }

  fn enqueue_wake(&self, request: WakeRequest) -> usize {
    let queue_len = {
      let mut wake_queue = self
        .wake_queue
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
      wake_queue.push_back(request);
      wake_queue.len()
    };
    self.update_state(|state| {
      state.pending_wake_count = queue_len;
    });
    queue_len
  }

  fn dequeue_wake(&self) -> Option<WakeRequest> {
    let (request, queue_len) = {
      let mut wake_queue = self
        .wake_queue
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
      let request = wake_queue.pop_front();
      (request, wake_queue.len())
    };
    if request.is_some() {
      self.update_state(|state| {
        state.pending_wake_count = queue_len;
      });
    }
    request
  }

  fn clear_wakes(&self) {
    let cleared = {
      let mut wake_queue = self
        .wake_queue
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
      wake_queue.clear();
      wake_queue.len()
    };
    self.update_state(|state| {
      state.pending_wake_count = cleared;
    });
  }

  pub(crate) async fn notify_exec_approval(
    &self,
    approval_id: &str,
    decision: cokra_protocol::ReviewDecision,
  ) -> bool {
    self
      .session
      .notify_exec_approval(approval_id, decision)
      .await
  }

  pub(crate) async fn notify_user_input(
    &self,
    request_id: &str,
    response: cokra_protocol::user_input::RequestUserInputResponse,
  ) -> bool {
    self.session.notify_user_input(request_id, response).await
  }
}

pub(crate) struct TeamRuntime {
  root_thread_id: ThreadId,
  legacy_store_key: String,
  team_store_key: String,
  run_store_key: String,
  config: Arc<Config>,
  model_client: Arc<ModelClient>,
  agent_control: Arc<AgentControl>,
  guards: Arc<Guards>,
  manager: Arc<ThreadManagerState>,
  root_tx_event: mpsc::Sender<EventMsg>,
  handles: Mutex<HashMap<String, Arc<ManagedAgentHandle>>>,
  team_state: Arc<Mutex<TeamState>>,
  run_state: Arc<Mutex<TeamRunState>>,
  mailbox_version_tx: watch::Sender<u64>,
  state_db: Arc<StateDb>,
}

static TEAM_RUNTIMES: OnceLock<Mutex<Vec<Arc<TeamRuntime>>>> = OnceLock::new();

fn runtime_registry() -> &'static Mutex<Vec<Arc<TeamRuntime>>> {
  TEAM_RUNTIMES.get_or_init(|| Mutex::new(Vec::new()))
}

fn lexically_normalize_path(path: std::path::PathBuf) -> std::path::PathBuf {
  use std::path::Component;

  let mut normalized = std::path::PathBuf::new();
  for component in path.components() {
    match component {
      Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
      Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR.to_string()),
      Component::CurDir => {}
      Component::ParentDir => {
        normalized.pop();
      }
      Component::Normal(segment) => normalized.push(segment),
    }
  }
  normalized
}

pub(crate) async fn register_team_runtime(
  config: Arc<Config>,
  model_client: Arc<ModelClient>,
  agent_control: Arc<AgentControl>,
  guards: Arc<Guards>,
  manager: Arc<ThreadManagerState>,
  root_tx_event: mpsc::Sender<EventMsg>,
  root_thread_id: ThreadId,
) -> anyhow::Result<()> {
  let state_db = Arc::new(StateDb::new(StateDb::default_path_for(&config.cwd)).await?);
  let legacy_store_key = config.cwd.display().to_string();
  let team_store_key = scoped_store_key("team", &legacy_store_key);
  let run_store_key = scoped_store_key("workflow", &legacy_store_key);
  let persisted =
    load_persisted_state::<TeamState>(&state_db, &team_store_key, Some(&legacy_store_key)).await?;
  let run_state = load_persisted_state::<TeamRunState>(&state_db, &run_store_key, None).await?;
  let mailbox_version = persisted
    .as_ref()
    .map(TeamState::mailbox_version)
    .unwrap_or_default();
  let (mailbox_version_tx, _mailbox_version_rx) = watch::channel(mailbox_version);
  let runtime = Arc::new(TeamRuntime {
    root_thread_id: root_thread_id.clone(),
    legacy_store_key,
    team_store_key,
    run_store_key,
    config,
    model_client,
    agent_control,
    guards,
    manager,
    root_tx_event,
    handles: Mutex::new(HashMap::new()),
    team_state: Arc::new(Mutex::new(persisted.unwrap_or_default())),
    run_state: Arc::new(Mutex::new(run_state.unwrap_or_default())),
    mailbox_version_tx,
    state_db,
  });

  let mut runtimes = runtime_registry()
    .lock()
    .unwrap_or_else(std::sync::PoisonError::into_inner);
  runtimes.retain(|item| item.root_thread_id != root_thread_id);
  runtimes.push(runtime);
  Ok(())
}

fn scoped_store_key(scope: &str, base_key: &str) -> String {
  format!("{scope}::{base_key}")
}

async fn load_persisted_state<T>(
  state_db: &StateDb,
  primary_key: &str,
  legacy_key: Option<&str>,
) -> anyhow::Result<Option<T>>
where
  T: serde::de::DeserializeOwned,
{
  if let Some(value) = state_db.load_json(primary_key).await? {
    return Ok(Some(value));
  }
  if let Some(legacy_key) = legacy_key {
    let value = state_db.load_json(legacy_key).await?;
    return Ok(value);
  }
  Ok(None)
}

pub(crate) fn clear_team_runtime(root_thread_id: &ThreadId) {
  let mut runtimes = runtime_registry()
    .lock()
    .unwrap_or_else(std::sync::PoisonError::into_inner);
  runtimes.retain(|runtime| &runtime.root_thread_id != root_thread_id);
}

pub(crate) fn runtime_for_thread(thread_id: &str) -> Option<Arc<TeamRuntime>> {
  let runtimes = runtime_registry()
    .lock()
    .unwrap_or_else(std::sync::PoisonError::into_inner);
  runtimes
    .iter()
    .find(|runtime| runtime.handles_thread(thread_id))
    .cloned()
}

impl TeamRuntime {
  pub(crate) fn is_root_thread(&self, thread_id: &str) -> bool {
    self.root_thread_id.to_string() == thread_id
  }

  pub(crate) fn list_spawned_agent_ids(&self) -> Vec<String> {
    self
      .handles
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .keys()
      .cloned()
      .collect()
  }

  pub(crate) fn snapshot(&self) -> TeamSnapshot {
    self.maintain_leases_inline();
    let threads = self
      .manager
      .list_thread_ids()
      .into_iter()
      .filter_map(|thread_id| self.manager.get_thread(&thread_id))
      .collect::<Vec<_>>();
    let statuses = threads
      .iter()
      .map(|thread| {
        let status = if thread.thread_id == self.root_thread_id {
          self.root_wait_state()
        } else {
          self
            .handle_for(&thread.thread_id.to_string())
            .map(|handle| handle.state().wait_state())
            .unwrap_or(CollabAgentWaitState {
              lifecycle: CollabAgentLifecycle::NotFound,
              ..Default::default()
            })
        };
        (thread.thread_id.to_string(), status)
      })
      .collect();
    let mut team_state = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner);
    team_state.snapshot(
      self.root_thread_id.to_string(),
      threads,
      statuses,
      Some(self.run_snapshot()),
    )
  }

  pub(crate) fn run_snapshot(&self) -> WorkflowRuntimeSnapshot {
    self
      .run_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .snapshot(self.root_thread_id.to_string())
  }

  pub(crate) async fn record_plan_artifact(&self, thread_id: String, text: String) -> WorkflowRun {
    let run = self
      .run_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .record_ad_hoc_plan(thread_id, text);
    self.persist_run_state().await;
    run
  }

  pub(crate) async fn create_task(
    &self,
    title: String,
    details: Option<String>,
    owner_thread_id: Option<String>,
    assignee_thread_id: Option<String>,
    workflow_run_id: Option<String>,
    requested_scopes: Vec<ScopeRequest>,
    blocking_reason: Option<String>,
    scope_policy_override: bool,
  ) -> anyhow::Result<TeamTask> {
    let requested_scopes = self.normalize_scope_requests(requested_scopes);
    self.validate_task_scope_policy(
      assignee_thread_id.as_deref().or(owner_thread_id.as_deref()),
      &requested_scopes,
      scope_policy_override,
    )?;
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .create_task(
        title,
        details,
        owner_thread_id,
        assignee_thread_id.clone(),
        workflow_run_id,
        requested_scopes,
        blocking_reason,
        scope_policy_override,
      );
    if let Some(assignee_thread_id) = assignee_thread_id.as_deref()
      && matches!(task.status, TeamTaskStatus::InProgress)
    {
      self.note_task_claim(task.clone(), assignee_thread_id.to_string());
    }
    self.persist_states().await;
    if let Some(assignee_thread_id) = task.assignee_thread_id.as_deref()
      && !matches!(
        task.ready_state,
        cokra_protocol::TeamTaskReadyState::Blocked
      )
    {
      let _ = self
        .schedule_turn(
          assignee_thread_id,
          wake_message(&WakeReason::TaskAssigned {
            task_id: task.id.clone(),
          }),
          WakeReason::TaskAssigned {
            task_id: task.id.clone(),
          },
        )
        .await;
    }
    Ok(task)
  }

  pub(crate) async fn submit_plan(
    &self,
    author_thread_id: String,
    summary: String,
    steps: Vec<String>,
    requires_approval: bool,
  ) -> TeamPlan {
    let workflow_run = self
      .run_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .create_run_for_team_plan(
        author_thread_id.clone(),
        summary.clone(),
        &steps,
        requires_approval,
      );
    let plan = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .submit_plan(
        author_thread_id,
        summary,
        steps,
        requires_approval,
        Some(workflow_run.id.clone()),
      );
    self.persist_states().await;
    plan
  }

  pub(crate) async fn decide_plan(
    &self,
    plan_id: &str,
    reviewer_thread_id: String,
    approved: bool,
    note: Option<String>,
  ) -> Option<TeamPlan> {
    let plan = {
      self
        .team_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .decide_plan(plan_id, reviewer_thread_id, approved, note)
    };
    if let Some(plan) = &plan {
      self
        .run_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .sync_plan_decision(plan);
    }
    self.persist_states().await;
    plan
  }

  pub(crate) fn requires_plan_approval(&self, thread_id: &str) -> bool {
    let team_requires = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .requires_plan_approval(thread_id);
    if team_requires {
      return true;
    }
    self
      .run_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .requires_approval(thread_id)
  }

  pub(crate) async fn update_task(
    &self,
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
  ) -> anyhow::Result<Option<TeamTask>> {
    let requested_scopes = requested_scopes.map(|scopes| self.normalize_scope_requests(scopes));
    let granted_scopes = granted_scopes.map(|scopes| self.normalize_scope_requests(scopes));
    if let Some(current) = self.task(task_id) {
      let effective_owner = owner_thread_id
        .as_ref()
        .and_then(|owner| owner.as_deref())
        .or(current.owner_thread_id.as_deref());
      let effective_assignee = assignee_thread_id
        .as_ref()
        .and_then(|assignee| assignee.as_deref())
        .or(current.assignee_thread_id.as_deref());
      let effective_scopes = requested_scopes
        .as_deref()
        .unwrap_or(&current.requested_scopes);
      let effective_override = scope_policy_override.unwrap_or(current.scope_policy_override);
      self.validate_task_scope_policy(
        effective_assignee.or(effective_owner),
        effective_scopes,
        effective_override,
      )?;
    }
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .update_task(
        task_id,
        status,
        assignee_thread_id,
        owner_thread_id,
        reviewer_thread_id,
        note,
        requested_scopes,
        granted_scopes,
        review_state,
        scope_policy_override,
      );
    if let Some(task) = &task
      && let Some(assignee_thread_id) = task.assignee_thread_id.as_deref()
      && matches!(task.status, TeamTaskStatus::InProgress)
    {
      self.note_task_claim(task.clone(), assignee_thread_id.to_string());
    }
    self.persist_states().await;
    if let Some(task) = &task {
      self.nudge_task_participants(task).await;
    }
    Ok(task)
  }

  pub(crate) async fn assign_task(
    &self,
    task_id: &str,
    assignee_thread_id: String,
    note: Option<String>,
    override_assignee: bool,
  ) -> Option<TeamTask> {
    if !override_assignee
      && let Some(existing) = self.task(task_id)
      && let Some(current_assignee) = existing.assignee_thread_id
      && current_assignee != assignee_thread_id
    {
      return None;
    }
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .assign_task(task_id, assignee_thread_id, note);
    if let Some(task) = &task
      && let Some(assignee_thread_id) = task.assignee_thread_id.as_deref()
      && matches!(task.status, TeamTaskStatus::InProgress)
    {
      self.note_task_claim(task.clone(), assignee_thread_id.to_string());
    }
    self.persist_states().await;
    if let Some(task) = &task {
      self.nudge_task_participants(task).await;
    }
    task
  }

  pub(crate) async fn handoff_task(
    &self,
    task_id: &str,
    to_thread_id: String,
    note: Option<String>,
    review_mode: bool,
  ) -> Option<TeamTask> {
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .handoff_task(task_id, to_thread_id, note, review_mode);
    if let Some(task) = &task
      && let Some(assignee_thread_id) = task.assignee_thread_id.as_deref()
      && matches!(
        task.status,
        TeamTaskStatus::InProgress | TeamTaskStatus::Review
      )
    {
      self.note_task_claim(task.clone(), assignee_thread_id.to_string());
    }
    self.persist_states().await;
    if let Some(task) = &task
      && let Some(assignee_thread_id) = task.assignee_thread_id.as_deref()
    {
      let reason = if review_mode {
        WakeReason::ReviewRequested {
          task_id: task.id.clone(),
        }
      } else {
        WakeReason::TaskHandedOff {
          task_id: task.id.clone(),
        }
      };
      let _ = self
        .schedule_turn(assignee_thread_id, wake_message(&reason), reason)
        .await;
    }
    task
  }

  pub(crate) async fn claim_task(
    &self,
    task_id: &str,
    claimer_thread_id: String,
    note: Option<String>,
  ) -> anyhow::Result<Option<TeamTask>> {
    if let Some(task) = self.task(task_id) {
      self.validate_claim_scope_policy(&claimer_thread_id, &task)?;
    }
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .claim_task(task_id, claimer_thread_id.clone(), note);
    if let Some(task) = &task {
      self.note_task_claim(task.clone(), claimer_thread_id);
    }
    self.persist_states().await;
    Ok(task)
  }

  pub(crate) async fn list_ready_tasks(&self, claimer_thread_id: &str) -> Vec<TeamTask> {
    self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .list_ready_tasks(claimer_thread_id)
  }

  pub(crate) async fn claim_ready_task(
    &self,
    task_id: &str,
    claimer_thread_id: String,
    note: Option<String>,
  ) -> anyhow::Result<Option<TeamTask>> {
    if let Some(task) = self.task(task_id) {
      self.validate_claim_scope_policy(&claimer_thread_id, &task)?;
    }
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .claim_ready_task(task_id, claimer_thread_id.clone(), note);
    if let Some(task) = &task {
      self.note_task_claim(task.clone(), claimer_thread_id);
    }
    self.persist_states().await;
    Ok(task)
  }

  pub(crate) async fn claim_next_task(
    &self,
    claimer_thread_id: &str,
  ) -> anyhow::Result<Option<TeamTask>> {
    let ready_tasks = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .list_ready_tasks(claimer_thread_id);
    let mut task = None;
    for candidate in ready_tasks {
      if self
        .validate_claim_scope_policy(claimer_thread_id, &candidate)
        .is_err()
      {
        continue;
      }
      task = self
        .team_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .claim_ready_task(&candidate.id, claimer_thread_id.to_string(), None);
      if task.is_some() {
        break;
      }
    }
    if let Some(task) = &task {
      self.note_task_claim(task.clone(), claimer_thread_id.to_string());
    }
    self.persist_states().await;
    Ok(task)
  }

  pub(crate) async fn add_task_dependency(
    &self,
    task_id: &str,
    dependency_task_id: &str,
    reason: Option<String>,
  ) -> Option<TeamTask> {
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .add_task_dependency(task_id, dependency_task_id, reason);
    self.persist_states().await;
    task
  }

  pub(crate) async fn remove_task_dependency(
    &self,
    task_id: &str,
    dependency_task_id: &str,
  ) -> Option<TeamTask> {
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .remove_task_dependency(task_id, dependency_task_id);
    self.persist_states().await;
    task
  }

  pub(crate) async fn block_task(&self, task_id: &str, reason: String) -> Option<TeamTask> {
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .block_task(task_id, reason);
    self.persist_states().await;
    task
  }

  pub(crate) async fn unblock_task(
    &self,
    task_id: &str,
    blocker_id: Option<&str>,
  ) -> Option<TeamTask> {
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .unblock_task(task_id, blocker_id);
    self.persist_states().await;
    task
  }

  pub(crate) async fn release_task_leases(&self, task_id: &str) -> Option<TeamTask> {
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .release_task_leases(task_id);
    self.persist_states().await;
    task
  }

  pub(crate) async fn force_release_lease(
    &self,
    actor_thread_id: &str,
    lease_id: &str,
  ) -> anyhow::Result<OwnershipLease> {
    if !self.is_root_thread(actor_thread_id) {
      anyhow::bail!("only @main can force release ownership leases");
    }
    let lease = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .force_release_lease(lease_id)
      .ok_or_else(|| anyhow::anyhow!("unknown lease id: {lease_id}"))?;
    self.persist_states().await;
    Ok(lease)
  }

  pub(crate) async fn ensure_mutation_paths_owned(
    &self,
    thread_id: &str,
    paths: &[String],
  ) -> anyhow::Result<()> {
    self.maintain_leases().await;
    let paths = paths
      .iter()
      .map(|path| self.normalize_scope_path(&OwnershipScopeKind::File, path))
      .collect::<Vec<_>>();
    let granted = {
      let mut team_state = self
        .team_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
      team_state.ensure_write_paths_owned(thread_id, &paths)
    };
    match granted {
      Ok(inserted) => {
        if inserted > 0 {
          self.persist_team_state().await;
        }
      }
      Err(failures) => {
        let details = failures
          .into_iter()
          .map(|failure| match failure {
            super::team_state::WriteOwnershipFailure::Blocked { path, lease } => format!(
              "{path} is locked by {} via task {} ({})",
              lease.owner_thread_id,
              lease.task_id,
              Self::access_label(&lease.access)
            ),
            super::team_state::WriteOwnershipFailure::MissingClaim { path } => {
              format!("{path} has no claimed task with an exclusive-write scope covering it")
            }
            super::team_state::WriteOwnershipFailure::ClaimedByOther {
              path,
              task_id,
              owner_thread_id,
            } => {
              let owner_label = self
                .collab_agent_ref(&owner_thread_id)
                .and_then(|agent| agent.nickname)
                .map(|nickname| format!("@{nickname}"))
                .unwrap_or(owner_thread_id);
              format!("{path} is claimed by {owner_label} via task {task_id} (exclusive-write)")
            }
            super::team_state::WriteOwnershipFailure::AmbiguousClaim { path, task_ids } => format!(
              "{path} matches multiple claimed tasks: {}",
              task_ids.join(", ")
            ),
          })
          .collect::<Vec<_>>()
          .join("; ");
        anyhow::bail!("ownership lock required before mutating paths: {details}");
      }
    }
    let touched = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .touch_thread_leases(thread_id);
    if touched > 0 {
      self.persist_team_state().await;
    }
    Ok(())
  }

  #[allow(clippy::too_many_arguments)]
  pub(crate) async fn post_message(
    &self,
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
    let message = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .post_message(
        sender_thread_id,
        recipient_thread_id,
        kind,
        route_key,
        delivery_mode,
        priority,
        correlation_id,
        task_id,
        message,
        expires_at,
      );
    self.persist_team_state_and_publish_mailbox().await;
    self.nudge_message_recipients(&message).await;
    message
  }

  pub(crate) async fn peek_messages(
    &self,
    reader_thread_id: &str,
    unread_only: bool,
  ) -> Vec<TeamMessage> {
    let messages = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .peek_messages(reader_thread_id, unread_only);
    self.persist_team_state_and_publish_mailbox().await;
    messages
  }

  pub(crate) async fn read_messages(
    &self,
    reader_thread_id: &str,
    unread_only: bool,
  ) -> Vec<TeamMessage> {
    let messages = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .read_messages(reader_thread_id, unread_only);
    self.persist_team_state_and_publish_mailbox().await;
    messages
  }

  pub(crate) async fn claim_queue_messages(
    &self,
    claimer_thread_id: &str,
    queue_name: &str,
    limit: usize,
  ) -> Vec<TeamMessage> {
    let messages = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .claim_queue_messages(claimer_thread_id, queue_name, limit);
    self.persist_team_state_and_publish_mailbox().await;
    messages
  }

  pub(crate) async fn ack_message(
    &self,
    acker_thread_id: &str,
    message_id: &str,
  ) -> Option<TeamMessage> {
    let message = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .ack_message(acker_thread_id, message_id);
    self.persist_team_state_and_publish_mailbox().await;
    message
  }

  pub(crate) async fn watch_inbox(
    &self,
    reader_thread_id: &str,
    after_version: Option<u64>,
    timeout_ms: Option<u64>,
    unread_only: bool,
  ) -> anyhow::Result<(u64, Vec<TeamMessage>, bool)> {
    let after_version = after_version.unwrap_or_default();
    let current_version = self.current_mailbox_version();
    if current_version > after_version {
      let messages = self.peek_messages(reader_thread_id, unread_only).await;
      return Ok((self.current_mailbox_version(), messages, false));
    }

    let mut version_rx = self.mailbox_version_tx.subscribe();
    if *version_rx.borrow() > after_version {
      let messages = self.peek_messages(reader_thread_id, unread_only).await;
      return Ok((self.current_mailbox_version(), messages, false));
    }

    let timeout_ms = timeout_ms.unwrap_or(30_000).clamp(1_000, 3_600_000);
    let wait_result = timeout(Duration::from_millis(timeout_ms), async {
      loop {
        if version_rx.changed().await.is_err() {
          break;
        }
        if *version_rx.borrow() > after_version {
          break;
        }
      }
    })
    .await;

    if wait_result.is_err() {
      return Ok((self.current_mailbox_version(), Vec::new(), true));
    }

    let messages = self.peek_messages(reader_thread_id, unread_only).await;
    Ok((self.current_mailbox_version(), messages, false))
  }

  pub(crate) async fn clear_state(&self) {
    self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .clear();
    self
      .run_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .clear();
    self.publish_mailbox_version();
    let _ = self.state_db.delete(&self.team_store_key).await;
    let _ = self.state_db.delete(&self.run_store_key).await;
    let _ = self.state_db.delete(&self.legacy_store_key).await;
  }

  pub(crate) fn thread_depth(&self, thread_id: &str) -> Option<usize> {
    self.find_thread_info(thread_id).map(|info| info.depth)
  }

  pub(crate) fn resolve_thread_id(&self, thread_id: &str) -> Option<ThreadId> {
    if self.root_thread_id.to_string() == thread_id {
      return Some(self.root_thread_id.clone());
    }

    self
      .manager
      .list_thread_ids()
      .into_iter()
      .find(|candidate| candidate.to_string() == thread_id)
  }

  pub(crate) fn resolve_agent_selector(&self, selector: &str) -> Option<String> {
    self.resolve_agent_selector_strict(selector).ok()
  }

  pub(crate) fn resolve_agent_selector_strict(&self, selector: &str) -> Result<String, String> {
    let selector = selector.trim();
    if selector.is_empty() {
      return Err("selector cannot be empty".to_string());
    }
    let selector = selector.strip_prefix('@').unwrap_or(selector);

    if self.resolve_thread_id(selector).is_some() {
      return Ok(selector.to_string());
    }
    if selector == "main" {
      return Ok(self.root_thread_id.to_string());
    }

    let matches = self
      .manager
      .list_thread_ids()
      .into_iter()
      .filter_map(|thread_id| self.manager.get_thread(&thread_id))
      .filter(|info| info.nickname.as_deref() == Some(selector))
      .map(|info| info.thread_id.to_string())
      .collect::<Vec<_>>();
    match matches.as_slice() {
      [thread_id] => Ok(thread_id.clone()),
      [] => Err(format!("agent not found: {selector}")),
      _ => Err(format!(
        "selector {selector} is ambiguous; matching thread ids: {}",
        matches.join(", ")
      )),
    }
  }

  pub(crate) async fn spawn_agent(
    &self,
    parent_thread_id: &str,
    message: String,
    nickname: Option<String>,
    role: String,
  ) -> anyhow::Result<ThreadId> {
    let parent = self
      .resolve_thread_id(parent_thread_id)
      .ok_or_else(|| anyhow::anyhow!("unknown parent thread: {parent_thread_id}"))?;
    let depth = self.thread_depth(parent_thread_id).unwrap_or(0) + 1;
    let thread_id = self
      .agent_control
      .spawn_agent(
        message.clone(),
        nickname,
        Some(role),
        Some(parent),
        depth,
        Some(self.config.agents.max_threads),
      )
      .await?;

    if let Err(err) = self.launch_spawned_agent(thread_id.clone(), message).await {
      let _ = self.agent_control.shutdown_spawned_agent(thread_id.clone());
      return Err(err);
    }

    Ok(thread_id)
  }

  pub(crate) async fn send_input(&self, agent_id: &str, message: String) -> anyhow::Result<()> {
    self
      .schedule_turn(agent_id, message, WakeReason::UserInput)
      .await
  }

  pub(crate) async fn close_agent(&self, agent_id: &str) -> anyhow::Result<CollabAgentLifecycle> {
    let handle = {
      let mut handles = self
        .handles
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
      handles.remove(agent_id)
    };

    let Some(handle) = handle else {
      return Ok(CollabAgentLifecycle::NotFound);
    };

    handle.update_state(|state| {
      state.lifecycle = CollabAgentLifecycle::Shutdown;
      state.pending_wake_count = 0;
      state.attention_reason = None;
    });
    handle.clear_wakes();
    let _ = handle.shutdown().await;
    self
      .agent_control
      .shutdown_spawned_agent(handle.thread_id().clone())?;
    if self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .release_thread_leases(agent_id)
      > 0
    {
      self.persist_states().await;
    }
    self.emit_agent_state_changed(agent_id).await;
    Ok(CollabAgentLifecycle::Shutdown)
  }

  pub(crate) fn subscribe_state(
    &self,
    agent_id: &str,
  ) -> Option<watch::Receiver<ManagedAgentState>> {
    self
      .handle_for(agent_id)
      .map(|handle| handle.subscribe_state())
  }

  pub(crate) fn wait_target_generation(&self, agent_id: &str) -> Option<u64> {
    self
      .handle_for(agent_id)
      .map(|handle| handle.state().scheduled_generation)
  }

  pub(crate) fn wait_state(&self, agent_id: &str) -> Option<CollabAgentWaitState> {
    self
      .handle_for(agent_id)
      .map(|handle| handle.state().wait_state())
  }

  pub(crate) fn collab_agent_ref(&self, agent_id: &str) -> Option<CollabAgentRef> {
    let thread = self.find_thread_info(agent_id)?;
    Some(CollabAgentRef {
      thread_id: thread.thread_id.to_string(),
      nickname: thread.nickname,
      role: Some(thread.role),
    })
  }

  pub(crate) fn collab_agent_refs(&self, agent_ids: &[String]) -> Vec<CollabAgentRef> {
    agent_ids
      .iter()
      .filter_map(|agent_id| self.collab_agent_ref(agent_id))
      .collect()
  }

  pub(crate) fn collab_agent_status_entries(
    &self,
    statuses: &HashMap<String, CollabAgentWaitState>,
  ) -> Vec<CollabAgentStatusEntry> {
    let mut entries = statuses
      .iter()
      .map(|(thread_id, status)| {
        let thread = self.find_thread_info(thread_id);
        CollabAgentStatusEntry {
          thread_id: thread_id.clone(),
          nickname: thread.as_ref().and_then(|info| info.nickname.clone()),
          role: thread.map(|info| info.role),
          state: status.clone(),
        }
      })
      .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.thread_id.cmp(&right.thread_id));
    entries
  }

  fn handles_thread(&self, thread_id: &str) -> bool {
    self.find_thread_info(thread_id).is_some()
  }

  fn find_thread_info(&self, thread_id: &str) -> Option<ThreadInfo> {
    if self.root_thread_id.to_string() == thread_id {
      return self.manager.get_thread(&self.root_thread_id);
    }

    self
      .manager
      .list_thread_ids()
      .into_iter()
      .find(|candidate| candidate.to_string() == thread_id)
      .and_then(|candidate| self.manager.get_thread(&candidate))
  }

  fn handle_for(&self, agent_id: &str) -> Option<Arc<ManagedAgentHandle>> {
    self
      .handles
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .get(agent_id)
      .cloned()
  }

  fn root_wait_state(&self) -> CollabAgentWaitState {
    let lifecycle = match self.agent_control.subscribe_status().borrow().clone() {
      crate::agent::AgentStatus::PendingInit | crate::agent::AgentStatus::Initializing => {
        CollabAgentLifecycle::PendingInit
      }
      crate::agent::AgentStatus::Ready => CollabAgentLifecycle::Ready,
      crate::agent::AgentStatus::Busy => CollabAgentLifecycle::Busy,
      crate::agent::AgentStatus::Error(_) => CollabAgentLifecycle::Error,
      crate::agent::AgentStatus::Shutdown => CollabAgentLifecycle::Shutdown,
    };
    CollabAgentWaitState {
      lifecycle,
      turn_outcome: CollabTurnOutcome::NoneYet,
      last_turn_summary: None,
      attention_reason: None,
      pending_wake_count: 0,
    }
  }

  fn note_task_claim(&self, task: TeamTask, claimer_thread_id: String) {
    self
      .run_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .note_task_claim(&task, &claimer_thread_id);
  }

  pub(crate) fn note_attention(&self, thread_id: &str, reason: impl Into<String>) {
    if let Some(handle) = self.handle_for(thread_id) {
      let reason = reason.into();
      handle.update_state(|state| {
        state.attention_reason = Some(reason.clone());
      });
      if let Some(thread_info) = self.find_thread_info(thread_id) {
        let _ = self.root_tx_event.try_send(agent_state_changed_event(
          &self.team_state,
          Some(&thread_info),
          thread_id,
          &handle.state(),
        ));
      }
    }
  }

  pub(crate) fn clear_attention(&self, thread_id: &str) {
    if let Some(handle) = self.handle_for(thread_id) {
      handle.update_state(|state| {
        state.attention_reason = None;
      });
      if let Some(thread_info) = self.find_thread_info(thread_id) {
        let _ = self.root_tx_event.try_send(agent_state_changed_event(
          &self.team_state,
          Some(&thread_info),
          thread_id,
          &handle.state(),
        ));
      }
    }
  }

  fn task(&self, task_id: &str) -> Option<TeamTask> {
    self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .task(task_id)
  }

  async fn schedule_turn(
    &self,
    agent_id: &str,
    message: String,
    reason: WakeReason,
  ) -> anyhow::Result<()> {
    let handle = self
      .handle_for(agent_id)
      .ok_or_else(|| anyhow::anyhow!("agent not found: {agent_id}"))?;
    self.clear_attention(agent_id);
    match handle.state().lifecycle {
      CollabAgentLifecycle::Ready => {
        handle.update_state(|state| {
          state.scheduled_generation = state.scheduled_generation.saturating_add(1);
          state.inflight_generation = state.scheduled_generation;
          state.lifecycle = CollabAgentLifecycle::Busy;
          state.turn_outcome = CollabTurnOutcome::NoneYet;
        });
        if let Err(err) = handle.send_turn_now(message).await {
          let error = err.to_string();
          handle.clear_wakes();
          handle.update_state(|state| {
            state.lifecycle = CollabAgentLifecycle::Error;
            state.turn_outcome = CollabTurnOutcome::Errored;
            state.last_turn_summary = Some(error.clone());
            state.attention_reason = Some(error.clone());
            state.settled_generation = state.scheduled_generation;
          });
          self.emit_agent_state_changed(agent_id).await;
          return Err(err);
        }
      }
      CollabAgentLifecycle::PendingInit | CollabAgentLifecycle::Busy => {
        let queue_len = handle.enqueue_wake(WakeRequest { message, reason });
        handle.update_state(|state| {
          state.scheduled_generation = state.scheduled_generation.saturating_add(1);
          state.pending_wake_count = queue_len;
        });
      }
      CollabAgentLifecycle::Error => {
        anyhow::bail!("agent {agent_id} is in an error state and cannot accept new work")
      }
      CollabAgentLifecycle::Shutdown | CollabAgentLifecycle::NotFound => {
        anyhow::bail!("agent {agent_id} is not available")
      }
    }
    self.emit_agent_state_changed(agent_id).await;
    Ok(())
  }

  async fn nudge_task_participants(&self, task: &TeamTask) {
    if matches!(
      task.ready_state,
      cokra_protocol::TeamTaskReadyState::Blocked
    ) {
      return;
    }
    if matches!(task.status, TeamTaskStatus::Review) {
      if let Some(reviewer_thread_id) = task.reviewer_thread_id.as_deref() {
        let reason = WakeReason::ReviewRequested {
          task_id: task.id.clone(),
        };
        let _ = self
          .schedule_turn(reviewer_thread_id, wake_message(&reason), reason)
          .await;
      }
      return;
    }

    if let Some(assignee_thread_id) = task.assignee_thread_id.as_deref()
      && !matches!(
        task.status,
        TeamTaskStatus::Completed | TeamTaskStatus::Failed | TeamTaskStatus::Canceled
      )
    {
      let reason = WakeReason::TaskAssigned {
        task_id: task.id.clone(),
      };
      let _ = self
        .schedule_turn(assignee_thread_id, wake_message(&reason), reason)
        .await;
    }
  }

  async fn nudge_message_recipients(&self, message: &TeamMessage) {
    if let Some(recipient_thread_id) = message.recipient_thread_id.as_deref() {
      let reason = WakeReason::MailboxUnread;
      let _ = self
        .schedule_turn(recipient_thread_id, wake_message(&reason), reason)
        .await;
      return;
    }

    if matches!(
      message.kind,
      TeamMessageKind::Broadcast | TeamMessageKind::Channel
    ) {
      for agent_id in self.list_spawned_agent_ids() {
        if agent_id == message.sender_thread_id.as_str() {
          continue;
        }
        let reason = WakeReason::MailboxUnread;
        let _ = self
          .schedule_turn(&agent_id, wake_message(&reason), reason)
          .await;
      }
    }
  }

  async fn emit_agent_state_changed(&self, thread_id: &str) {
    let Some(handle) = self.handle_for(thread_id) else {
      return;
    };
    let Some(thread) = self.find_thread_info(thread_id) else {
      return;
    };
    let state = handle.state();
    let (open_task_count, unread_count) = {
      let team_state = self
        .team_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
      (
        team_state.open_task_count_for_thread(thread_id),
        team_state.unread_count_for(thread_id),
      )
    };
    let _ = self
      .root_tx_event
      .send(EventMsg::CollabAgentStateChanged(
        CollabAgentStateChangedEvent {
          thread_id: thread_id.to_string(),
          nickname: thread.nickname,
          role: Some(thread.role),
          lifecycle: state.lifecycle,
          turn_outcome: state.turn_outcome,
          last_turn_summary: state.last_turn_summary,
          attention_reason: state.attention_reason,
          pending_wake_count: state.pending_wake_count,
          open_task_count,
          unread_count,
        },
      ))
      .await;
  }

  fn normalize_scope_requests(&self, scopes: Vec<ScopeRequest>) -> Vec<ScopeRequest> {
    scopes
      .into_iter()
      .map(|scope| ScopeRequest {
        kind: scope.kind.clone(),
        path: self.normalize_scope_path(&scope.kind, &scope.path),
        access: scope.access,
        reason: scope.reason,
      })
      .collect()
  }

  fn validate_task_scope_policy(
    &self,
    primary_owner_thread_id: Option<&str>,
    requested_scopes: &[ScopeRequest],
    scope_policy_override: bool,
  ) -> anyhow::Result<()> {
    if scope_policy_override || !self.has_spawned_teammates() {
      return Ok(());
    }

    let is_root_claim =
      primary_owner_thread_id.is_none_or(|thread_id| self.is_root_thread(thread_id));
    if !is_root_claim {
      return Ok(());
    }

    if requested_scopes
      .iter()
      .any(|scope| self.is_repo_root_directory_write_scope(scope))
    {
      anyhow::bail!(
        "@main cannot claim or stage a repo-root exclusive-write directory scope while teammates exist; assign the task to a teammate or set scope_policy_override=true"
      );
    }

    Ok(())
  }

  fn validate_claim_scope_policy(
    &self,
    claimer_thread_id: &str,
    task: &TeamTask,
  ) -> anyhow::Result<()> {
    self.validate_task_scope_policy(
      Some(claimer_thread_id),
      &task.requested_scopes,
      task.scope_policy_override,
    )
  }

  fn normalize_scope_path(&self, kind: &OwnershipScopeKind, path: &str) -> String {
    if matches!(kind, OwnershipScopeKind::Module) {
      return path.trim().to_string();
    }
    let path_buf = std::path::PathBuf::from(path);
    let joined = if path_buf.is_absolute() {
      path_buf
    } else {
      self.config.cwd.join(path_buf)
    };
    lexically_normalize_path(joined).display().to_string()
  }

  fn has_spawned_teammates(&self) -> bool {
    self
      .manager
      .list_thread_ids()
      .into_iter()
      .any(|thread_id| thread_id != self.root_thread_id)
  }

  fn is_repo_root_directory_write_scope(&self, scope: &ScopeRequest) -> bool {
    matches!(scope.kind, OwnershipScopeKind::Directory)
      && matches!(scope.access, OwnershipAccessMode::ExclusiveWrite)
      && scope.path == self.config.cwd.display().to_string()
  }

  fn active_lease_owner_ids(&self) -> HashSet<String> {
    let mut active_thread_ids = HashSet::from([self.root_thread_id.to_string()]);
    let handles = self
      .handles
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner);
    for (thread_id, handle) in handles.iter() {
      if !matches!(
        handle.state().lifecycle,
        CollabAgentLifecycle::Shutdown | CollabAgentLifecycle::NotFound
      ) {
        active_thread_ids.insert(thread_id.clone());
      }
    }
    active_thread_ids
  }

  fn maintain_leases_inline(&self) -> usize {
    let active_thread_ids = self.active_lease_owner_ids();
    let stale_before = Utc::now().timestamp() - LEASE_STALE_AFTER_SECS;
    self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .cleanup_stale_leases(&active_thread_ids, stale_before)
      .len()
  }

  async fn maintain_leases(&self) {
    if self.maintain_leases_inline() > 0 {
      self.persist_team_state().await;
    }
  }

  fn access_label(access: &OwnershipAccessMode) -> &'static str {
    match access {
      OwnershipAccessMode::SharedRead => "shared-read",
      OwnershipAccessMode::ExclusiveWrite => "exclusive-write",
      OwnershipAccessMode::Review => "review",
    }
  }

  fn current_mailbox_version(&self) -> u64 {
    self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .mailbox_version()
  }

  fn publish_mailbox_version(&self) {
    let mailbox_version = self.current_mailbox_version();
    if *self.mailbox_version_tx.borrow() != mailbox_version {
      let _ = self.mailbox_version_tx.send(mailbox_version);
    }
  }

  async fn persist_team_state(&self) {
    let snapshot = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .clone();
    let _ = self
      .state_db
      .save_json(&self.team_store_key, &snapshot)
      .await;
  }

  async fn persist_team_state_and_publish_mailbox(&self) {
    self.persist_team_state().await;
    self.publish_mailbox_version();
  }

  async fn persist_run_state(&self) {
    let snapshot = self
      .run_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .clone();
    let _ = self
      .state_db
      .save_json(&self.run_store_key, &snapshot)
      .await;
  }

  async fn persist_states(&self) {
    self.persist_team_state().await;
    self.persist_run_state().await;
  }

  pub(crate) async fn notify_exec_approval(
    &self,
    approval_id: &str,
    decision: cokra_protocol::ReviewDecision,
  ) -> bool {
    let handles = self
      .handles
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .values()
      .cloned()
      .collect::<Vec<_>>();
    for handle in handles {
      if handle
        .notify_exec_approval(approval_id, decision.clone())
        .await
      {
        return true;
      }
    }
    false
  }

  pub(crate) async fn notify_user_input(
    &self,
    request_id: &str,
    response: cokra_protocol::user_input::RequestUserInputResponse,
  ) -> bool {
    let handles = self
      .handles
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .values()
      .cloned()
      .collect::<Vec<_>>();
    for handle in handles {
      if handle.notify_user_input(request_id, response.clone()).await {
        return true;
      }
    }
    false
  }

  async fn launch_spawned_agent(
    &self,
    thread_id: ThreadId,
    initial_message: String,
  ) -> anyhow::Result<()> {
    let session = Arc::new(Session::new_with_thread_id(thread_id.clone()));
    let thread_info = self.find_thread_info(&thread_id.to_string());
    let mut turn_config = self.agent_control.turn_config().await;
    if let Some(base) = turn_config.system_prompt.as_deref() {
      // Tradeoff: we append a small sub-agent contract to the base prompt instead of
      // replacing it wholesale. This keeps tool-use and safety guidance consistent
      // with the main agent while making "agent teams" useful for research/discussion.
      turn_config.system_prompt = Some(build_spawned_agent_system_prompt(
        base,
        thread_info
          .as_ref()
          .and_then(|info| info.nickname.as_deref()),
        thread_info
          .as_ref()
          .map(|info| info.role.as_str())
          .unwrap_or("default"),
      ));
    }
    let tooling = build_default_tooling_with_cwd(self.config.as_ref(), &self.config.cwd).await?;
    let tool_registry = tooling.registry;
    let tool_router = tooling.router;
    let tool_runtime = tooling.runtime;
    let (tx_raw_event, mut rx_raw_event) = mpsc::channel(CHILD_EVENT_CHANNEL_CAPACITY);
    let root_tx_event = self.root_tx_event.clone();
    tokio::spawn(async move {
      while let Some(event) = rx_raw_event.recv().await {
        let _ = root_tx_event.send(event).await;
      }
    });

    let agent_control = Arc::new(AgentControl::new(
      Uuid::new_v4().to_string(),
      self.model_client.clone(),
      tool_registry,
      tool_router,
      tool_runtime,
      session.clone(),
      turn_config,
      tx_raw_event,
      Arc::downgrade(&self.manager),
      self.guards.clone(),
      thread_id.clone(),
    ));
    agent_control
      .start()
      .await
      .context("failed to start spawned agent")?;

    let (tx_cmd, mut rx_cmd) = mpsc::channel(CHILD_COMMAND_CHANNEL_CAPACITY);
    let initial_state = ManagedAgentState {
      lifecycle: CollabAgentLifecycle::Ready,
      ..Default::default()
    };
    let (state_tx, state_rx) = watch::channel(initial_state);
    let handle = Arc::new(ManagedAgentHandle {
      thread_id: thread_id.clone(),
      session: session.clone(),
      tx_cmd: tx_cmd.clone(),
      state_tx,
      state_rx,
      wake_queue: Arc::new(Mutex::new(VecDeque::new())),
    });

    self
      .handles
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .insert(thread_id.to_string(), handle.clone());

    let root_tx_event_for_state = self.root_tx_event.clone();
    let team_state_for_events = self.team_state.clone();
    let handle_for_loop = handle.clone();
    let thread_info_for_loop = thread_info.clone();
    let thread_id_for_loop = thread_id.to_string();
    let tx_cmd_loop = tx_cmd.clone();

    tokio::spawn(async move {
      while let Some(command) = rx_cmd.recv().await {
        match command {
          ChildCommand::UserTurn { message } => {
            handle_for_loop.update_state(|state| {
              state.lifecycle = CollabAgentLifecycle::Busy;
              state.attention_reason = None;
              state.turn_outcome = CollabTurnOutcome::NoneYet;
            });
            let _ = root_tx_event_for_state
              .send(agent_state_changed_event(
                &team_state_for_events,
                thread_info_for_loop.as_ref(),
                &thread_id_for_loop,
                &handle_for_loop.state(),
              ))
              .await;
            let turn = Turn {
              turn_id: Uuid::new_v4().to_string(),
              user_message: message,
            };
            match agent_control.process_turn(turn).await {
              Ok(result) => {
                let has_output = !result.content.trim().is_empty();
                let final_message = has_output.then_some(result.content);
                handle_for_loop.update_state(|state| {
                  state.lifecycle = CollabAgentLifecycle::Ready;
                  state.turn_outcome = CollabTurnOutcome::Succeeded;
                  state.last_turn_summary = final_message.clone();
                  state.attention_reason = None;
                  state.settled_generation = state.inflight_generation;
                });
                let _ = root_tx_event_for_state
                  .send(agent_state_changed_event(
                    &team_state_for_events,
                    thread_info_for_loop.as_ref(),
                    &thread_id_for_loop,
                    &handle_for_loop.state(),
                  ))
                  .await;
                if let Some(next_wake) = handle_for_loop.dequeue_wake() {
                  handle_for_loop.update_state(|state| {
                    state.lifecycle = CollabAgentLifecycle::Busy;
                    state.inflight_generation = state.settled_generation.saturating_add(1);
                    state.turn_outcome = CollabTurnOutcome::NoneYet;
                  });
                  let _ = root_tx_event_for_state
                    .send(agent_state_changed_event(
                      &team_state_for_events,
                      thread_info_for_loop.as_ref(),
                      &thread_id_for_loop,
                      &handle_for_loop.state(),
                    ))
                    .await;
                  if tx_cmd_loop
                    .send(ChildCommand::UserTurn {
                      message: next_wake.message,
                    })
                    .await
                    .is_err()
                  {
                    handle_for_loop.clear_wakes();
                    handle_for_loop.update_state(|state| {
                      state.lifecycle = CollabAgentLifecycle::Error;
                      state.turn_outcome = CollabTurnOutcome::Errored;
                      state.last_turn_summary =
                        Some("agent loop terminated while dispatching queued work".to_string());
                      state.attention_reason =
                        Some("agent loop terminated while dispatching queued work".to_string());
                      state.settled_generation = state.scheduled_generation;
                    });
                    let _ = root_tx_event_for_state
                      .send(agent_state_changed_event(
                        &team_state_for_events,
                        thread_info_for_loop.as_ref(),
                        &thread_id_for_loop,
                        &handle_for_loop.state(),
                      ))
                      .await;
                  }
                }
              }
              Err(err) => {
                let err = err.to_string();
                handle_for_loop.clear_wakes();
                handle_for_loop.update_state(|state| {
                  state.lifecycle = CollabAgentLifecycle::Error;
                  state.turn_outcome = CollabTurnOutcome::Errored;
                  state.last_turn_summary = Some(err.clone());
                  state.attention_reason = Some(err.clone());
                  state.settled_generation = state.scheduled_generation;
                });
                let _ = root_tx_event_for_state
                  .send(agent_state_changed_event(
                    &team_state_for_events,
                    thread_info_for_loop.as_ref(),
                    &thread_id_for_loop,
                    &handle_for_loop.state(),
                  ))
                  .await;
              }
            }
          }
          ChildCommand::Shutdown => {
            let _ = agent_control.stop().await;
            handle_for_loop.update_state(|state| {
              state.lifecycle = CollabAgentLifecycle::Shutdown;
              state.pending_wake_count = 0;
              state.attention_reason = None;
            });
            let _ = root_tx_event_for_state
              .send(agent_state_changed_event(
                &team_state_for_events,
                thread_info_for_loop.as_ref(),
                &thread_id_for_loop,
                &handle_for_loop.state(),
              ))
              .await;
            break;
          }
        }
      }

      let _ = agent_control.stop().await;
      let _ = session.shutdown().await;
    });

    handle.update_state(|state| {
      state.scheduled_generation = 1;
      state.inflight_generation = 1;
      state.lifecycle = CollabAgentLifecycle::Busy;
      state.turn_outcome = CollabTurnOutcome::NoneYet;
    });
    let thread_id_string = thread_id.to_string();
    self.emit_agent_state_changed(&thread_id_string).await;
    tx_cmd
      .send(ChildCommand::UserTurn {
        message: initial_message,
      })
      .await
      .map_err(|_| anyhow::anyhow!("spawned agent loop terminated before initial task"))?;

    Ok(())
  }
}

fn wake_message(reason: &WakeReason) -> String {
  match reason {
    WakeReason::UserInput => {
      "You have new coordinator input. Process it now and continue the team workflow."
        .to_string()
    }
    WakeReason::TaskAssigned { task_id } => {
      format!(
        "You have a newly assigned team task ({task_id}). Check team_status, claim it if ready, and continue."
      )
    }
    WakeReason::TaskHandedOff { task_id } => {
      format!(
        "A team task ({task_id}) was handed off to you. Check team_status, claim or continue it, and report progress through team tools."
      )
    }
    WakeReason::ReviewRequested { task_id } => {
      format!(
        "A review task ({task_id}) is waiting for you. Check team_status, review the handoff, and record the result."
      )
    }
    WakeReason::MailboxUnread => {
      "You have unread team mailbox messages. Read your team messages and act on any newly assigned work."
        .to_string()
    }
  }
}

fn agent_state_changed_event(
  team_state: &Arc<Mutex<TeamState>>,
  thread_info: Option<&ThreadInfo>,
  thread_id: &str,
  state: &ManagedAgentState,
) -> EventMsg {
  let (open_task_count, unread_count) = {
    let team_state = team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner);
    (
      team_state.open_task_count_for_thread(thread_id),
      team_state.unread_count_for(thread_id),
    )
  };
  EventMsg::CollabAgentStateChanged(CollabAgentStateChangedEvent {
    thread_id: thread_id.to_string(),
    nickname: thread_info.and_then(|info| info.nickname.clone()),
    role: thread_info.map(|info| info.role.clone()),
    lifecycle: state.lifecycle.clone(),
    turn_outcome: state.turn_outcome.clone(),
    last_turn_summary: state.last_turn_summary.clone(),
    attention_reason: state.attention_reason.clone(),
    pending_wake_count: state.pending_wake_count,
    open_task_count,
    unread_count,
  })
}

fn build_spawned_agent_system_prompt(base: &str, nickname: Option<&str>, role: &str) -> String {
  let mut out =
    String::with_capacity(base.len() + crate::prompts::AGENT_SPAWNED_SUFFIX.len() + 128);
  out.push_str(base);
  if let Some(nickname) = nickname.map(str::trim).filter(|value| !value.is_empty()) {
    out.push_str(&format!("\nYour teammate nickname is @{nickname}."));
  }
  if !role.trim().is_empty() && !role.eq_ignore_ascii_case("default") {
    out.push_str(&format!("\nYour teammate role is {role}."));
  }
  out.push_str(crate::prompts::AGENT_SPAWNED_SUFFIX);
  out
}

/// 为主代理（leader/orchestrator）构建 agent-teams 系统提示后缀。
/// 当 agent-teams 功能启用时，追加到主代理的系统提示末尾。
/// Prompt text lives in `src/prompts/agent_leader.md`.
pub(crate) fn build_leader_agent_teams_prompt_suffix() -> &'static str {
  crate::prompts::AGENT_LEADER_SUFFIX
}

#[allow(dead_code)]
mod team_runs {
  use std::collections::HashMap;

  use chrono::Utc;
  use serde::Deserialize;
  use serde::Serialize;
  use uuid::Uuid;

  use cokra_protocol::TeamPlan;
  use cokra_protocol::TeamPlanStatus;
  use cokra_protocol::TeamTask;
  use cokra_protocol::WorkflowApprovalState;
  use cokra_protocol::WorkflowApprovalStatus;
  use cokra_protocol::WorkflowArtifact;
  use cokra_protocol::WorkflowRun;
  use cokra_protocol::WorkflowRunStatus;
  use cokra_protocol::WorkflowRuntimeSnapshot;
  use cokra_protocol::WorkflowStepState;
  use cokra_protocol::WorkflowStepStatus;

  const AD_HOC_PLAN_WORKFLOW: &str = "ad_hoc_plan";
  const TEAM_PLAN_WORKFLOW: &str = "team_plan";

  #[derive(Debug, Default, Clone, Serialize, Deserialize)]
  pub(crate) struct TeamRunState {
    #[serde(default)]
    runs: HashMap<String, WorkflowRun>,
  }

  impl TeamRunState {
    pub(crate) fn snapshot(&self, root_thread_id: String) -> WorkflowRuntimeSnapshot {
      let mut runs = self.runs.values().cloned().collect::<Vec<_>>();
      runs.sort_by(|left, right| {
        left
          .created_at
          .cmp(&right.created_at)
          .then_with(|| left.id.cmp(&right.id))
      });
      WorkflowRuntimeSnapshot {
        root_thread_id,
        runs,
      }
    }

    pub(crate) fn requires_approval(&self, thread_id: &str) -> bool {
      self
        .runs
        .values()
        .filter(|run| run.owner_thread_id == thread_id)
        .any(|run| {
          matches!(
            run.approval.status,
            WorkflowApprovalStatus::Pending | WorkflowApprovalStatus::Rejected
          )
        })
    }

    pub(crate) fn record_ad_hoc_plan(
      &mut self,
      owner_thread_id: String,
      text: String,
    ) -> WorkflowRun {
      let run_id = self
        .find_latest_open_run(&owner_thread_id, AD_HOC_PLAN_WORKFLOW)
        .unwrap_or_else(|| {
          let now = Utc::now().timestamp();
          let run = WorkflowRun {
            id: Uuid::new_v4().to_string(),
            workflow_name: AD_HOC_PLAN_WORKFLOW.to_string(),
            title: "Ad hoc plan".to_string(),
            owner_thread_id: owner_thread_id.clone(),
            status: WorkflowRunStatus::Active,
            resume_token: None,
            current_step_id: None,
            steps: Vec::new(),
            artifacts: Vec::new(),
            approval: WorkflowApprovalState::default(),
            created_at: now,
            updated_at: now,
          };
          let run_id = run.id.clone();
          self.runs.insert(run_id.clone(), run);
          run_id
        });

      let now = Utc::now().timestamp();
      let run = self.runs.get_mut(&run_id).expect("workflow run exists");
      let step_id = "plan".to_string();
      upsert_step(
        &mut run.steps,
        WorkflowStepState {
          id: step_id.clone(),
          title: "Capture plan".to_string(),
          details: Some("Record an ad hoc plan item emitted by the agent.".to_string()),
          status: WorkflowStepStatus::Completed,
          assigned_thread_id: Some(owner_thread_id.clone()),
          updated_at: now,
        },
      );
      run.status = WorkflowRunStatus::Active;
      run.current_step_id = Some(step_id);
      run.resume_token = Some(format!("workflow://{}/{}", owner_thread_id, run.id));
      run.updated_at = now;
      run.artifacts.push(WorkflowArtifact {
        id: Uuid::new_v4().to_string(),
        kind: "plan_text".to_string(),
        label: humanize_artifact_label(&text, "Plan item"),
        content: text,
        created_by_thread_id: Some(owner_thread_id),
        created_at: now,
      });
      run.clone()
    }

    pub(crate) fn create_run_for_team_plan(
      &mut self,
      author_thread_id: String,
      summary: String,
      steps: &[String],
      requires_approval: bool,
    ) -> WorkflowRun {
      let now = Utc::now().timestamp();
      let run_id = Uuid::new_v4().to_string();
      let workflow_steps = steps
        .iter()
        .enumerate()
        .map(|(index, step)| WorkflowStepState {
          id: format!("step-{}", index + 1),
          title: step.clone(),
          details: None,
          status: if index == 0 && !requires_approval {
            WorkflowStepStatus::InProgress
          } else if requires_approval {
            WorkflowStepStatus::Blocked
          } else {
            WorkflowStepStatus::Pending
          },
          assigned_thread_id: Some(author_thread_id.clone()),
          updated_at: now,
        })
        .collect::<Vec<_>>();
      let approval = WorkflowApprovalState {
        status: if requires_approval {
          WorkflowApprovalStatus::Pending
        } else {
          WorkflowApprovalStatus::Approved
        },
        requested_by_thread_id: requires_approval.then_some(author_thread_id.clone()),
        reviewer_thread_id: None,
        note: None,
        updated_at: now,
      };
      let run = WorkflowRun {
        id: run_id.clone(),
        workflow_name: TEAM_PLAN_WORKFLOW.to_string(),
        title: summary.clone(),
        owner_thread_id: author_thread_id.clone(),
        status: if requires_approval {
          WorkflowRunStatus::WaitingApproval
        } else {
          WorkflowRunStatus::Active
        },
        resume_token: Some(format!("workflow://{author_thread_id}/{run_id}")),
        current_step_id: workflow_steps.first().map(|step| step.id.clone()),
        steps: workflow_steps,
        artifacts: vec![WorkflowArtifact {
          id: Uuid::new_v4().to_string(),
          kind: "team_plan_summary".to_string(),
          label: humanize_artifact_label(&summary, "Team plan"),
          content: summary,
          created_by_thread_id: Some(author_thread_id),
          created_at: now,
        }],
        approval,
        created_at: now,
        updated_at: now,
      };
      self.runs.insert(run_id, run.clone());
      run
    }

    pub(crate) fn sync_plan_decision(&mut self, plan: &TeamPlan) -> Option<WorkflowRun> {
      let workflow_run_id = plan.workflow_run_id.as_deref()?;
      let run = self.runs.get_mut(workflow_run_id)?;
      let now = Utc::now().timestamp();
      run.approval.status = match plan.status {
        TeamPlanStatus::Approved => WorkflowApprovalStatus::Approved,
        TeamPlanStatus::Rejected => WorkflowApprovalStatus::Rejected,
        TeamPlanStatus::PendingApproval => WorkflowApprovalStatus::Pending,
        TeamPlanStatus::Draft => WorkflowApprovalStatus::NotRequested,
      };
      run.approval.reviewer_thread_id = plan.reviewer_thread_id.clone();
      run.approval.note = plan.review_note.clone();
      run.approval.updated_at = now;
      run.status = match plan.status {
        TeamPlanStatus::Approved => WorkflowRunStatus::Active,
        TeamPlanStatus::Rejected => WorkflowRunStatus::Failed,
        TeamPlanStatus::PendingApproval => WorkflowRunStatus::WaitingApproval,
        TeamPlanStatus::Draft => WorkflowRunStatus::Pending,
      };
      if matches!(plan.status, TeamPlanStatus::Approved) {
        if let Some(first) = run.steps.first_mut() {
          first.status = WorkflowStepStatus::InProgress;
          first.updated_at = now;
          run.current_step_id = Some(first.id.clone());
        }
      } else if matches!(plan.status, TeamPlanStatus::Rejected) {
        for step in &mut run.steps {
          if !matches!(
            step.status,
            WorkflowStepStatus::Completed | WorkflowStepStatus::Skipped
          ) {
            step.status = WorkflowStepStatus::Blocked;
            step.updated_at = now;
          }
        }
      }
      run.updated_at = now;
      Some(run.clone())
    }

    pub(crate) fn update_step(
      &mut self,
      run_id: &str,
      step_id: &str,
      status: WorkflowStepStatus,
      assigned_thread_id: Option<String>,
      details: Option<String>,
    ) -> Option<WorkflowRun> {
      let run = self.runs.get_mut(run_id)?;
      let now = Utc::now().timestamp();
      let step = run.steps.iter_mut().find(|step| step.id == step_id)?;
      step.status = status;
      if let Some(assigned_thread_id) = assigned_thread_id {
        step.assigned_thread_id = Some(assigned_thread_id);
      }
      if let Some(details) = details {
        step.details = Some(details);
      }
      step.updated_at = now;
      run.current_step_id = Some(step.id.clone());
      run.updated_at = now;
      Some(run.clone())
    }

    pub(crate) fn append_artifact(
      &mut self,
      run_id: &str,
      kind: impl Into<String>,
      label: impl Into<String>,
      content: impl Into<String>,
      created_by_thread_id: Option<String>,
    ) -> Option<WorkflowRun> {
      let run = self.runs.get_mut(run_id)?;
      let now = Utc::now().timestamp();
      run.artifacts.push(WorkflowArtifact {
        id: Uuid::new_v4().to_string(),
        kind: kind.into(),
        label: label.into(),
        content: content.into(),
        created_by_thread_id,
        created_at: now,
      });
      run.updated_at = now;
      Some(run.clone())
    }

    pub(crate) fn set_run_status(
      &mut self,
      run_id: &str,
      status: WorkflowRunStatus,
      current_step_id: Option<String>,
      resume_token: Option<String>,
    ) -> Option<WorkflowRun> {
      let run = self.runs.get_mut(run_id)?;
      let now = Utc::now().timestamp();
      run.status = status;
      if let Some(current_step_id) = current_step_id {
        run.current_step_id = Some(current_step_id);
      }
      if let Some(resume_token) = resume_token {
        run.resume_token = Some(resume_token);
      }
      run.updated_at = now;
      Some(run.clone())
    }

    pub(crate) fn note_task_claim(
      &mut self,
      task: &TeamTask,
      claimer_thread_id: &str,
    ) -> Option<WorkflowRun> {
      let workflow_run_id = task.workflow_run_id.as_deref()?;
      let run = self.runs.get_mut(workflow_run_id)?;
      let now = Utc::now().timestamp();
      run.owner_thread_id = claimer_thread_id.to_string();
      run.resume_token = Some(format!("workflow://task/{}/{}", task.id, claimer_thread_id));
      run.status = WorkflowRunStatus::Active;
      run.updated_at = now;
      if let Some(current_step_id) = run.current_step_id.clone()
        && let Some(step) = run.steps.iter_mut().find(|step| step.id == current_step_id)
      {
        step.assigned_thread_id = Some(claimer_thread_id.to_string());
        if matches!(
          step.status,
          WorkflowStepStatus::Pending | WorkflowStepStatus::Blocked
        ) {
          step.status = WorkflowStepStatus::InProgress;
        }
        step.updated_at = now;
      }
      Some(run.clone())
    }

    pub(crate) fn clear(&mut self) {
      self.runs.clear();
    }

    fn find_latest_open_run(&self, owner_thread_id: &str, workflow_name: &str) -> Option<String> {
      self
        .runs
        .values()
        .filter(|run| {
          run.owner_thread_id == owner_thread_id
            && run.workflow_name == workflow_name
            && !matches!(
              run.status,
              WorkflowRunStatus::Completed
                | WorkflowRunStatus::Failed
                | WorkflowRunStatus::Canceled
            )
        })
        .max_by_key(|run| run.updated_at)
        .map(|run| run.id.clone())
    }
  }

  fn upsert_step(steps: &mut Vec<WorkflowStepState>, next: WorkflowStepState) {
    if let Some(existing) = steps.iter_mut().find(|step| step.id == next.id) {
      *existing = next;
    } else {
      steps.push(next);
    }
  }

  fn humanize_artifact_label(text: &str, fallback: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
      return fallback.to_string();
    }
    let mut label = trimmed.lines().next().unwrap_or(trimmed).trim().to_string();
    if label.len() > 48 {
      label.truncate(45);
      label.push_str("...");
    }
    label
  }

  #[cfg(test)]
  mod tests {
    use super::*;

    #[test]
    fn team_plan_workflow_requires_approval_until_reviewed() {
      let mut state = TeamRunState::default();
      let run = state.create_run_for_team_plan(
        "root".to_string(),
        "Review deploy plan".to_string(),
        &["Inspect release".to_string(), "Deploy".to_string()],
        true,
      );

      assert!(state.requires_approval("root"));
      assert_eq!(run.approval.status, WorkflowApprovalStatus::Pending);
      assert_eq!(run.status, WorkflowRunStatus::WaitingApproval);
    }

    #[test]
    fn sync_plan_decision_promotes_first_step_after_approval() {
      let mut state = TeamRunState::default();
      let run = state.create_run_for_team_plan(
        "root".to_string(),
        "Review deploy plan".to_string(),
        &["Inspect release".to_string(), "Deploy".to_string()],
        true,
      );
      let plan = TeamPlan {
        id: "plan-1".to_string(),
        author_thread_id: "root".to_string(),
        summary: "Review deploy plan".to_string(),
        steps: vec!["Inspect release".to_string(), "Deploy".to_string()],
        status: TeamPlanStatus::Approved,
        requires_approval: true,
        reviewer_thread_id: Some("reviewer".to_string()),
        review_note: Some("Looks good".to_string()),
        workflow_run_id: Some(run.id.clone()),
        created_at: 0,
        updated_at: 0,
      };

      let updated = state.sync_plan_decision(&plan).expect("run");
      assert_eq!(updated.approval.status, WorkflowApprovalStatus::Approved);
      assert_eq!(updated.status, WorkflowRunStatus::Active);
      assert_eq!(updated.steps[0].status, WorkflowStepStatus::InProgress);
      assert!(!state.requires_approval("root"));
    }
  }
}
