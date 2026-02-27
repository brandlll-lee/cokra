// Session Task System
use std::sync::Arc;

use async_trait::async_trait;
use cokra_protocol::UserInput;
use tokio::sync::Notify;
use tokio_util::sync::{CancellationToken, AbortOnDropHandle};

use super::Session;
use crate::turn::TurnContext;

/// Session task trait - all tasks must implement this
#[async_trait]
pub trait SessionTask: Send + Sync {
    /// Get the task kind
    fn kind(&self) -> TaskKind;

    /// Run the task
    async fn run(
        self: Arc<Self>,
        session_ctx: Arc<SessionTaskContext>,
        turn_context: Arc<TurnContext>,
        input: Vec<UserInput>,
        cancellation_token: CancellationToken,
    ) -> anyhow::Result<()>;

    /// Abort the task (cleanup)
    async fn abort(
        &self,
        _session_ctx: Arc<SessionTaskContext>,
        _turn_context: Arc<TurnContext>,
    ) {
        // Default: no cleanup needed
    }
}

/// Task kind enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// Regular turn task
    Regular,
    /// Background task
    Background,
    /// Sub-agent task
    SubAgent,
}

/// Session task context
pub struct SessionTaskContext {
    session: Arc<Session>,
}

impl SessionTaskContext {
    /// Create a new session task context
    pub fn new(session: Arc<Session>) -> Self {
        Self { session }
    }

    /// Get the session
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// Clone the session
    pub fn clone_session(&self) -> Arc<Session> {
        Arc::clone(&self.session)
    }
}

/// Running task wrapper
pub(crate) struct RunningTask {
    /// Completion notifier
    pub(crate) done: Arc<Notify>,
    /// Task handle
    pub(crate) handle: Arc<AbortOnDropHandle<()>>,
    /// Task kind
    pub(crate) kind: TaskKind,
    /// The task itself
    pub(crate) task: Arc<dyn SessionTask>,
    /// Cancellation token
    pub(crate) cancellation_token: CancellationToken,
    /// Turn context
    pub(crate) turn_context: Arc<TurnContext>,
}

/// Regular turn task
pub struct RegularTask {
    /// Task ID
    pub id: String,
}

#[async_trait]
impl SessionTask for RegularTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Regular
    }

    async fn run(
        self: Arc<Self>,
        session_ctx: Arc<SessionTaskContext>,
        turn_context: Arc<TurnContext>,
        input: Vec<UserInput>,
        cancellation_token: CancellationToken,
    ) -> anyhow::Result<()> {
        use cokra_protocol::{EventMsg, TurnStartedEvent, TurnCompleteEvent};

        let session = session_ctx.session();

        // Emit turn started
        let event = EventMsg::TurnStarted(TurnStartedEvent {
            thread_id: session.thread_id().to_string(),
            turn_id: turn_context.sub_id.clone(),
            mode: cokra_protocol::ModeKind::Default,
            model: turn_context.model.clone(),
            start_time: chrono::Utc::now().timestamp(),
        });
        session.send_event(&turn_context, event).await;

        // Process input
        if !input.is_empty() {
            // TODO: Actual turn execution logic
        }

        // Check for cancellation
        if cancellation_token.is_cancelled() {
            return Ok(());
        }

        // Emit turn complete
        let event = EventMsg::TurnComplete(cokra_protocol::TurnCompleteEvent {
            thread_id: session.thread_id().to_string(),
            turn_id: turn_context.sub_id.clone(),
            status: cokra_protocol::CompletionStatus::Success,
            end_time: chrono::Utc::now().timestamp(),
        });
        session.send_event(&turn_context, event).await;

        Ok(())
    }
}
