use std::sync::{Arc, Weak};

use tokio::sync::{RwLock, broadcast, mpsc, watch};

use cokra_protocol::{CollabAgentSpawnBeginEvent, CollabAgentSpawnEndEvent, EventMsg, ThreadId};

use crate::model::ModelClient;
use crate::session::Session;
use crate::thread_manager::ThreadManagerState;
use crate::tools::registry::ToolRegistry;
use crate::turn::{TurnConfig, TurnExecutor, TurnResult, UserInput};

use super::guards::{Guards, exceeds_thread_spawn_depth_limit};
use super::status::AgentStatus;

/// Turn input handled by agent control.
#[derive(Debug, Clone)]
pub struct Turn {
  pub user_message: String,
}

/// Agent control plane object.
pub struct AgentControl {
  id: String,
  status: Arc<RwLock<AgentStatus>>,
  model_client: Arc<ModelClient>,
  tool_registry: Arc<ToolRegistry>,
  session: Arc<Session>,
  turn_config: Arc<RwLock<TurnConfig>>,
  tx_event: mpsc::Sender<EventMsg>,
  status_tx: watch::Sender<AgentStatus>,
  status_rx: watch::Receiver<AgentStatus>,
  manager: Weak<ThreadManagerState>,
  guards: Arc<Guards>,
  root_thread_id: ThreadId,
}

impl AgentControl {
  #[allow(clippy::too_many_arguments)]
  pub fn new(
    id: String,
    model_client: Arc<ModelClient>,
    tool_registry: Arc<ToolRegistry>,
    session: Arc<Session>,
    turn_config: TurnConfig,
    tx_event: mpsc::Sender<EventMsg>,
    manager: Weak<ThreadManagerState>,
    guards: Arc<Guards>,
    root_thread_id: ThreadId,
  ) -> Self {
    let (status_tx, status_rx) = watch::channel(AgentStatus::PendingInit);
    Self {
      id,
      status: Arc::new(RwLock::new(AgentStatus::PendingInit)),
      model_client,
      tool_registry,
      session,
      turn_config: Arc::new(RwLock::new(turn_config)),
      tx_event,
      status_tx,
      status_rx,
      manager,
      guards,
      root_thread_id,
    }
  }

  pub fn id(&self) -> &str {
    &self.id
  }

  pub fn subscribe_status(&self) -> watch::Receiver<AgentStatus> {
    self.status_rx.clone()
  }

  pub async fn start(&self) -> anyhow::Result<()> {
    self.transition(AgentStatus::Initializing).await;
    self.transition(AgentStatus::Ready).await;
    Ok(())
  }

  pub async fn process_turn(&self, turn: Turn) -> anyhow::Result<TurnResult> {
    self.transition(AgentStatus::Busy).await;

    let turn_config = self.turn_config.read().await.clone();

    let executor = TurnExecutor::new(
      self.model_client.clone(),
      self.tool_registry.clone(),
      self.session.clone(),
      self.tx_event.clone(),
      turn_config,
    );

    let result = executor
      .run_turn(UserInput {
        content: turn.user_message,
        attachments: Vec::new(),
      })
      .await;

    match result {
      Ok(r) => {
        self.transition(AgentStatus::Ready).await;
        Ok(r)
      }
      Err(e) => {
        self.transition(AgentStatus::Error(e.to_string())).await;
        Err(anyhow::anyhow!(e))
      }
    }
  }

  pub async fn stop(&self) -> anyhow::Result<()> {
    self.transition(AgentStatus::Shutdown).await;
    Ok(())
  }

  pub async fn set_turn_config(&self, config: TurnConfig) {
    *self.turn_config.write().await = config;
  }

  pub async fn turn_config(&self) -> TurnConfig {
    self.turn_config.read().await.clone()
  }

  pub async fn status(&self) -> AgentStatus {
    self.status.read().await.clone()
  }

  pub fn root_thread_id(&self) -> ThreadId {
    self.root_thread_id.clone()
  }

  pub fn guards(&self) -> Arc<Guards> {
    Arc::clone(&self.guards)
  }

  pub fn subscribe_thread_created(&self) -> anyhow::Result<broadcast::Receiver<ThreadId>> {
    let manager = self.upgrade_manager()?;
    Ok(manager.subscribe_thread_created())
  }

  pub fn list_thread_ids(&self) -> anyhow::Result<Vec<ThreadId>> {
    let manager = self.upgrade_manager()?;
    Ok(manager.list_thread_ids())
  }

  pub async fn spawn_agent(
    &self,
    task: String,
    role: Option<String>,
    parent_thread_id: Option<ThreadId>,
    depth: usize,
    max_threads: Option<usize>,
  ) -> anyhow::Result<ThreadId> {
    if task.trim().is_empty() {
      anyhow::bail!("spawn_agent requires non-empty task");
    }
    if exceeds_thread_spawn_depth_limit(depth) {
      anyhow::bail!("spawn depth {depth} exceeds max supported depth");
    }

    let manager = self.upgrade_manager()?;
    let reservation = self
      .guards
      .reserve_spawn_slot(max_threads)
      .map_err(anyhow::Error::from)?;

    let parent_thread_id = parent_thread_id.unwrap_or_else(|| self.root_thread_id.clone());
    let role = role.unwrap_or_else(|| "default".to_string());

    let _ = self
      .tx_event
      .send(EventMsg::CollabAgentSpawnBegin(
        CollabAgentSpawnBeginEvent {
          thread_id: parent_thread_id.to_string(),
          agent_id: "pending".to_string(),
          role: role.clone(),
        },
      ))
      .await;

    let thread_id = manager.spawn_thread(parent_thread_id, depth, role, task);
    reservation.commit(thread_id.clone());
    manager.notify_thread_created(thread_id.clone());

    let _ = self
      .tx_event
      .send(EventMsg::CollabAgentSpawnEnd(CollabAgentSpawnEndEvent {
        thread_id: thread_id.to_string(),
        agent_id: thread_id.to_string(),
        status: "created".to_string(),
      }))
      .await;

    Ok(thread_id)
  }

  pub fn shutdown_spawned_agent(&self, thread_id: ThreadId) -> anyhow::Result<()> {
    let manager = self.upgrade_manager()?;
    if manager.remove_thread(&thread_id) {
      self.guards.release_spawned_thread(thread_id);
    }
    Ok(())
  }

  async fn transition(&self, next: AgentStatus) {
    let mut status = self.status.write().await;
    if status.can_transition_to(&next) {
      *status = next.clone();
      let _ = self.status_tx.send(next);
    }
  }

  fn upgrade_manager(&self) -> anyhow::Result<Arc<ThreadManagerState>> {
    self
      .manager
      .upgrade()
      .ok_or_else(|| anyhow::anyhow!("thread manager dropped"))
  }
}
