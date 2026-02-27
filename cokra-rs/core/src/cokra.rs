// Cokra Core Orchestrator
// Main entry point for the Cokra AI Agent Team system

use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, watch};
use tracing::{debug, info};

use cokra_config::Config;
use cokra_protocol::{EventMsg, Op, ThreadId, AgentStatus};

use crate::session::{Session, SessionConfiguration};
use crate::event::{Event, EventBroadcaster};

/// The high-level interface to the Cokra system.
///
/// It operates as a queue pair where you send submissions and receive events.
pub struct Cokra {
    /// Sender for submissions
    tx_sub: mpsc::Sender<Submission>,
    /// Receiver for events
    rx_event: mpsc::Receiver<Event>,
    /// Agent status watcher
    agent_status: watch::Receiver<AgentStatus>,
    /// Session reference
    session: Arc<Session>,
}

/// Result of spawning a new Cokra instance
pub struct CokraSpawnOk {
    pub cokra: Cokra,
    pub thread_id: ThreadId,
}

/// Submission types
pub enum Submission {
    /// User operation
    Op(OpSubmission),
    /// Shutdown request
    Shutdown,
}

/// Operation submission
pub struct OpSubmission {
    pub sub_id: String,
    pub op: Op,
}

impl Cokra {
    /// Create a new Cokra instance
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let config = Arc::new(config);

        // Create channels
        let (tx_sub, rx_sub) = mpsc::channel(128);
        let (tx_event, rx_event) = mpsc::channel(128);
        let (agent_status_tx, agent_status_rx) = watch::channel(AgentStatus::PendingInit);

        // Create session
        let session = Session::new(
            SessionConfiguration::default(),
            config.clone(),
            tx_event,
            agent_status_tx,
        ).await?;

        Ok(Self {
            tx_sub,
            rx_event,
            agent_status: agent_status_rx,
            session,
        })
    }

    /// Submit an operation
    pub async fn submit(&self, op: Op) -> anyhow::Result<()> {
        let sub_id = uuid::Uuid::new_v4().to_string();

        let submission = Submission::Op(OpSubmission {
            sub_id,
            op,
        });

        self.tx_sub.send(submission).await?;
        Ok(())
    }

    /// Subscribe to events
    pub fn subscribe_events(&self) -> broadcast::Receiver<Event> {
        self.session.subscribe_events()
    }

    /// Get agent status
    pub fn agent_status(&self) -> AgentStatus {
        self.agent_status.borrow().clone()
    }

    /// Get thread ID
    pub fn thread_id(&self) -> &ThreadId {
        self.session.thread_id()
    }

    /// Shutdown Cokra
    pub async fn shutdown(self) -> anyhow::Result<()> {
        self.tx_sub.send(Submission::Shutdown).await?;
        self.session.shutdown().await?;
        Ok(())
    }
}

impl CokraSpawnOk {
    /// Get the thread ID
    pub fn thread_id(&self) -> &ThreadId {
        self.cokra.thread_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cokra_creation() {
        let config = Config::default();
        let result = Cokra::new(config).await;
        assert!(result.is_ok());
    }
}
