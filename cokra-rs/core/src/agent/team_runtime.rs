use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use anyhow::Context;
use cokra_protocol::BackgroundEventEvent;
use cokra_protocol::EventMsg;
use tokio::sync::mpsc;
use tokio::sync::watch;
use uuid::Uuid;

use cokra_config::Config;
use cokra_protocol::AgentStatus as CollabAgentStatus;
use cokra_protocol::CollabAgentRef;
use cokra_protocol::CollabAgentStatusEntry;
use cokra_protocol::TeamMessage;
use cokra_protocol::TeamMessageKind;
use cokra_protocol::TeamPlan;
use cokra_protocol::TeamSnapshot;
use cokra_protocol::TeamTask;
use cokra_protocol::TeamTaskStatus;
use cokra_protocol::ThreadId;
use cokra_state::StateDb;

use crate::agent::AgentControl;
use crate::agent::Turn;
use crate::model::ModelClient;
use crate::session::Session;
use crate::thread_manager::ThreadInfo;
use crate::thread_manager::ThreadManagerState;
use crate::tools::build_default_tools;

use super::Guards;
use super::team_state::TeamState;

const CHILD_COMMAND_CHANNEL_CAPACITY: usize = 32;
const CHILD_EVENT_CHANNEL_CAPACITY: usize = 512;

#[derive(Debug)]
enum ChildCommand {
  UserTurn { message: String },
  Shutdown,
}

#[derive(Clone)]
pub(crate) struct ManagedAgentHandle {
  thread_id: ThreadId,
  session: Arc<Session>,
  tx_cmd: mpsc::Sender<ChildCommand>,
  status_rx: watch::Receiver<CollabAgentStatus>,
}

impl ManagedAgentHandle {
  pub(crate) async fn send_input(&self, message: String) -> anyhow::Result<()> {
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

  pub(crate) fn subscribe_status(&self) -> watch::Receiver<CollabAgentStatus> {
    self.status_rx.clone()
  }

  pub(crate) fn thread_id(&self) -> &ThreadId {
    &self.thread_id
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
  store_key: String,
  config: Arc<Config>,
  model_client: Arc<ModelClient>,
  agent_control: Arc<AgentControl>,
  guards: Arc<Guards>,
  manager: Arc<ThreadManagerState>,
  root_tx_event: mpsc::Sender<EventMsg>,
  handles: Mutex<HashMap<String, Arc<ManagedAgentHandle>>>,
  team_state: Mutex<TeamState>,
  state_db: Arc<StateDb>,
}

static TEAM_RUNTIMES: OnceLock<Mutex<Vec<Arc<TeamRuntime>>>> = OnceLock::new();

fn runtime_registry() -> &'static Mutex<Vec<Arc<TeamRuntime>>> {
  TEAM_RUNTIMES.get_or_init(|| Mutex::new(Vec::new()))
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
  let store_key = config.cwd.display().to_string();
  let persisted = state_db.load_json::<TeamState>(&store_key).await?;
  let runtime = Arc::new(TeamRuntime {
    root_thread_id: root_thread_id.clone(),
    store_key,
    config,
    model_client,
    agent_control,
    guards,
    manager,
    root_tx_event,
    handles: Mutex::new(HashMap::new()),
    team_state: Mutex::new(persisted.unwrap_or_default()),
    state_db,
  });

  let mut runtimes = runtime_registry()
    .lock()
    .unwrap_or_else(std::sync::PoisonError::into_inner);
  runtimes.retain(|item| item.root_thread_id != root_thread_id);
  runtimes.push(runtime);
  Ok(())
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
          CollabAgentStatus::Running
        } else {
          self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&thread.thread_id.to_string())
            .map(|handle| handle.status_rx.borrow().clone())
            .unwrap_or(CollabAgentStatus::NotFound)
        };
        (thread.thread_id.to_string(), status)
      })
      .collect();
    self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .snapshot(self.root_thread_id.to_string(), threads, statuses)
  }

  pub(crate) async fn create_task(
    &self,
    title: String,
    details: Option<String>,
    assignee_thread_id: Option<String>,
  ) -> TeamTask {
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .create_task(title, details, assignee_thread_id);
    self.persist_state().await;
    task
  }

  pub(crate) async fn submit_plan(
    &self,
    author_thread_id: String,
    summary: String,
    steps: Vec<String>,
    requires_approval: bool,
  ) -> TeamPlan {
    let plan = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .submit_plan(author_thread_id, summary, steps, requires_approval);
    self.persist_state().await;
    plan
  }

  pub(crate) async fn decide_plan(
    &self,
    plan_id: &str,
    reviewer_thread_id: String,
    approved: bool,
    note: Option<String>,
  ) -> Option<TeamPlan> {
    let plan = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .decide_plan(plan_id, reviewer_thread_id, approved, note);
    self.persist_state().await;
    plan
  }

  pub(crate) fn requires_plan_approval(&self, thread_id: &str) -> bool {
    self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .requires_plan_approval(thread_id)
  }

  pub(crate) async fn update_task(
    &self,
    task_id: &str,
    status: Option<TeamTaskStatus>,
    assignee_thread_id: Option<Option<String>>,
    note: Option<String>,
  ) -> Option<TeamTask> {
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .update_task(task_id, status, assignee_thread_id, note);
    self.persist_state().await;
    task
  }

  pub(crate) async fn assign_task(
    &self,
    task_id: &str,
    assignee_thread_id: String,
    note: Option<String>,
  ) -> Option<TeamTask> {
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .assign_task(task_id, assignee_thread_id, note);
    self.persist_state().await;
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
    self.persist_state().await;
    task
  }

  pub(crate) async fn claim_next_task(&self, claimer_thread_id: &str) -> Option<TeamTask> {
    let task = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .claim_next_task(claimer_thread_id);
    self.persist_state().await;
    task
  }

  pub(crate) async fn post_message(
    &self,
    sender_thread_id: String,
    recipient_thread_id: Option<String>,
    kind: TeamMessageKind,
    route_key: Option<String>,
    message: String,
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
        message,
      );
    self.persist_state().await;
    message
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
    self.persist_state().await;
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
    self.persist_state().await;
    messages
  }

  pub(crate) async fn clear_state(&self) {
    self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .clear();
    let _ = self.state_db.delete(&self.store_key).await;
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
    let selector = selector.trim();
    if selector.is_empty() {
      return None;
    }

    if self.resolve_thread_id(selector).is_some() {
      return Some(selector.to_string());
    }

    // Fallback: resolve by nickname (Codex-style teams often refer to agents by name).
    self
      .manager
      .list_thread_ids()
      .into_iter()
      .filter_map(|thread_id| self.manager.get_thread(&thread_id))
      .find(|info| info.nickname.as_deref() == Some(selector))
      .map(|info| info.thread_id.to_string())
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
    let handle = self
      .handle_for(agent_id)
      .ok_or_else(|| anyhow::anyhow!("agent not found: {agent_id}"))?;
    handle.send_input(message).await
  }

  pub(crate) async fn close_agent(&self, agent_id: &str) -> anyhow::Result<CollabAgentStatus> {
    let handle = {
      let mut handles = self
        .handles
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
      handles.remove(agent_id)
    };

    let Some(handle) = handle else {
      return Ok(CollabAgentStatus::NotFound);
    };

    let _ = handle.shutdown().await;
    self
      .agent_control
      .shutdown_spawned_agent(handle.thread_id().clone())?;
    Ok(CollabAgentStatus::Shutdown)
  }

  pub(crate) fn subscribe_status(
    &self,
    agent_id: &str,
  ) -> Option<watch::Receiver<CollabAgentStatus>> {
    self
      .handle_for(agent_id)
      .map(|handle| handle.subscribe_status())
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
    statuses: &HashMap<String, CollabAgentStatus>,
  ) -> Vec<CollabAgentStatusEntry> {
    let mut entries = statuses
      .iter()
      .map(|(thread_id, status)| {
        let thread = self.find_thread_info(thread_id);
        CollabAgentStatusEntry {
          thread_id: thread_id.clone(),
          nickname: thread.as_ref().and_then(|info| info.nickname.clone()),
          role: thread.map(|info| info.role),
          status: status.clone(),
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

  async fn persist_state(&self) {
    let snapshot = self
      .team_state
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .clone();
    let _ = self.state_db.save_json(&self.store_key, &snapshot).await;
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
    let (tool_registry, tool_router) = build_default_tools(self.config.as_ref()).await?;
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
    let (status_tx, status_rx) = watch::channel(CollabAgentStatus::PendingInit);
    let handle = Arc::new(ManagedAgentHandle {
      thread_id: thread_id.clone(),
      session: session.clone(),
      tx_cmd: tx_cmd.clone(),
      status_rx,
    });

    self
      .handles
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .insert(thread_id.to_string(), handle);

    let root_tx_event_for_bg = self.root_tx_event.clone();
    let nickname_display = thread_info
      .as_ref()
      .and_then(|info| info.nickname.clone())
      .map(|value| value.trim().to_string())
      .filter(|value| !value.is_empty())
      .unwrap_or_else(|| "agent".to_string());

    tokio::spawn(async move {
      while let Some(command) = rx_cmd.recv().await {
        match command {
          ChildCommand::UserTurn { message } => {
            let _ = status_tx.send(CollabAgentStatus::Running);
            let turn = Turn {
              turn_id: Uuid::new_v4().to_string(),
              user_message: message,
            };
            match agent_control.process_turn(turn).await {
              Ok(result) => {
                let has_output = !result.content.trim().is_empty();
                let final_message = has_output.then_some(result.content);
                let _ = status_tx.send(CollabAgentStatus::Completed(final_message));
                let bg_msg = if has_output {
                  format!("@{nickname_display} completed task")
                } else {
                  format!("@{nickname_display} completed (no output)")
                };
                let _ = root_tx_event_for_bg
                  .send(EventMsg::BackgroundEvent(BackgroundEventEvent {
                    message: bg_msg,
                  }))
                  .await;
              }
              Err(err) => {
                let err = err.to_string();
                let _ = status_tx.send(CollabAgentStatus::Errored(err.clone()));
                let bg_msg = format!("@{nickname_display} errored: {err}");
                let _ = root_tx_event_for_bg
                  .send(EventMsg::BackgroundEvent(BackgroundEventEvent {
                    message: bg_msg,
                  }))
                  .await;
              }
            }
          }
          ChildCommand::Shutdown => {
            let _ = agent_control.stop().await;
            let _ = status_tx.send(CollabAgentStatus::Shutdown);
            break;
          }
        }
      }

      let _ = agent_control.stop().await;
      let _ = session.shutdown().await;
    });

    tx_cmd
      .send(ChildCommand::UserTurn {
        message: initial_message,
      })
      .await
      .map_err(|_| anyhow::anyhow!("spawned agent loop terminated before initial task"))?;

    Ok(())
  }
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
