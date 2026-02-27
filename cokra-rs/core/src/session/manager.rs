// Session Manager
use std::sync::Arc;

use cokra_config::Config;
use cokra_protocol::{EventMsg, ThreadId, AgentStatus};
use tokio::sync::{mpsc, watch, Mutex, broadcast};
use tracing::{debug, info, warn};

use super::{SessionConfiguration, SessionState, ActiveTurn, SessionSource};
use crate::event::Event;
use crate::turn::TurnContext;

/// Session - Core orchestrator for a single conversation thread
pub(crate) struct Session {
    /// Thread/conversation ID
    pub(crate) conversation_id: ThreadId,
    /// Event sender
    tx_event: mpsc::Sender<Event>,
    /// Agent status broadcaster
    agent_status: watch::Sender<AgentStatus>,
    /// Mutable session state
    state: Mutex<SessionState>,
    /// Session source (root or sub-agent)
    session_source: SessionSource,
    /// Active turn (running task)
    pub(crate) active_turn: Mutex<Option<ActiveTurn>>,
    /// Configuration
    config: Arc<Config>,
    /// Event broadcaster for subscribers
    event_broadcaster: broadcast::Sender<Event>,
}

impl Session {
    /// Create a new session
    pub(crate) async fn new(
        session_configuration: SessionConfiguration,
        config: Arc<Config>,
        tx_event: mpsc::Sender<Event>,
        agent_status: watch::Sender<AgentStatus>,
    ) -> anyhow::Result<Arc<Self>> {
        let (event_broadcaster, _) = broadcast::channel(256);

        let session = Self {
            conversation_id: ThreadId::new(),
            tx_event,
            agent_status,
            state: Mutex::new(SessionState::new(session_configuration)),
            session_source: SessionSource::Root,
            active_turn: Mutex::new(None),
            config,
            event_broadcaster,
        };

        Ok(Arc::new(session))
    }

    /// Get the thread ID
    pub fn thread_id(&self) -> &ThreadId {
        &self.conversation_id
    }

    /// Subscribe to events
    pub fn subscribe_events(&self) -> broadcast::Receiver<Event> {
        self.event_broadcaster.subscribe()
    }

    /// Send an event
    pub(crate) async fn send_event(&self, turn_context: &TurnContext, msg: EventMsg) {
        let event = Event {
            id: turn_context.sub_id.clone(),
            msg: msg.clone(),
        };
        self.send_event_raw(event).await;
    }

    /// Send a raw event
    pub(crate) async fn send_event_raw(&self, event: Event) {
        // Update agent status if applicable
        if let Some(status) = agent_status_from_event(&event.msg) {
            self.agent_status.send_replace(status);
        }

        // Send to main event channel
        if let Err(e) = self.tx_event.send(event.clone()).await {
            debug!("Dropping event because channel is closed: {e}");
        }

        // Broadcast to subscribers
        let _ = self.event_broadcaster.send(event);
    }

    /// Spawn a new task
    pub async fn spawn_task<T: crate::session::task::SessionTask + 'static>(
        self: &Arc<Self>,
        turn_context: Arc<TurnContext>,
        input: Vec<cokra_protocol::UserInput>,
        task: T,
    ) {
        use crate::session::task::{SessionTaskContext, RunningTask, TaskKind};
        use tokio_util::sync::CancellationToken;

        // Abort any existing tasks
        self.abort_all_tasks().await;

        let task: Arc<dyn crate::session::task::SessionTask> = Arc::new(task);
        let cancellation_token = CancellationToken::new();
        let done = Arc::new(tokio::sync::Notify::new());

        // Spawn the task
        let session = Arc::clone(self);
        let ctx = Arc::clone(&turn_context);
        let task_for_run = Arc::clone(&task);
        let task_cancellation_token = cancellation_token.child_token();

        let handle = tokio::spawn(async move {
            let session_ctx = Arc::new(SessionTaskContext::new(Arc::clone(&session)));
            let _ = task_for_run.run(
                Arc::clone(&session_ctx),
                ctx,
                input,
                task_cancellation_token,
            ).await;

            // Notify completion
            done.notify_waiters();
        });

        let running_task = RunningTask {
            done,
            handle: Arc::new(tokio_util::sync::AbortOnDropHandle::new(handle)),
            kind: task.kind(),
            task,
            cancellation_token,
            turn_context: Arc::clone(&turn_context),
        };

        self.register_new_active_task(running_task).await;
    }

    /// Register a new active task
    async fn register_new_active_task(&self, task: crate::session::task::RunningTask) {
        let mut active = self.active_turn.lock().await;
        if let Some(at) = active.as_mut() {
            at.tasks.insert(task.turn_context.sub_id.clone(), task);
        } else {
            let mut at = ActiveTurn::new();
            at.tasks.insert(task.turn_context.sub_id.clone(), task);
            *active = Some(at);
        }
    }

    /// Abort all running tasks
    pub async fn abort_all_tasks(&self) {
        let tasks = {
            let mut active = self.active_turn.lock().await;
            if let Some(at) = active.take() {
                at.tasks.into_values().collect::<Vec<_>>()
            } else {
                vec![]
            }
        };

        for task in tasks {
            task.cancellation_token.cancel();
            task.handle.abort();
        }
    }

    /// Shutdown the session
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.abort_all_tasks().await;
        let event = Event {
            id: "shutdown".to_string(),
            msg: EventMsg::ShutdownComplete,
        };
        self.send_event_raw(event).await;
        Ok(())
    }

    /// Get the configuration
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Create a new turn context
    pub(crate) async fn new_turn_context(&self, sub_id: String) -> Arc<TurnContext> {
        Arc::new(TurnContext::new(
            sub_id,
            self.config.clone(),
            self.session_source.clone(),
        ))
    }
}

/// Extract agent status from event
fn agent_status_from_event(msg: &EventMsg) -> Option<AgentStatus> {
    match msg {
        EventMsg::TurnStarted(_) => Some(AgentStatus::Running),
        EventMsg::TurnComplete(_) => Some(AgentStatus::Completed(None)),
        EventMsg::TurnAborted(e) => Some(AgentStatus::Errored(e.reason.clone())),
        EventMsg::Error(e) => Some(AgentStatus::Errored(e.error.clone())),
        EventMsg::ShutdownComplete => Some(AgentStatus::Shutdown),
        _ => None,
    }
}
