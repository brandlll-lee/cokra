use std::sync::Arc;

use tokio::sync::{RwLock, mpsc, watch};

use crate::model::ModelClient;
use crate::session::Session;
use crate::tools::registry::ToolRegistry;
use crate::turn::{TurnConfig, TurnExecutor, TurnResult, UserInput};

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
  turn_config: TurnConfig,
  tx_event: mpsc::Sender<cokra_protocol::EventMsg>,
  status_tx: watch::Sender<AgentStatus>,
  status_rx: watch::Receiver<AgentStatus>,
}

impl AgentControl {
  pub fn new(
    id: String,
    model_client: Arc<ModelClient>,
    tool_registry: Arc<ToolRegistry>,
    session: Arc<Session>,
    turn_config: TurnConfig,
    tx_event: mpsc::Sender<cokra_protocol::EventMsg>,
  ) -> Self {
    let (status_tx, status_rx) = watch::channel(AgentStatus::PendingInit);
    Self {
      id,
      status: Arc::new(RwLock::new(AgentStatus::PendingInit)),
      model_client,
      tool_registry,
      session,
      turn_config,
      tx_event,
      status_tx,
      status_rx,
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

    let executor = TurnExecutor::new(
      self.model_client.clone(),
      self.tool_registry.clone(),
      self.session.clone(),
      self.tx_event.clone(),
      self.turn_config.clone(),
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

  pub async fn status(&self) -> AgentStatus {
    self.status.read().await.clone()
  }

  async fn transition(&self, next: AgentStatus) {
    let mut status = self.status.write().await;
    if status.can_transition_to(&next) {
      *status = next.clone();
      let _ = self.status_tx.send(next);
    }
  }
}
