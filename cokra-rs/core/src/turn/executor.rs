// Turn Executor
use std::sync::Arc;

use async_trait::async_trait;
use cokra_protocol::{EventMsg, UserInput};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::session::{task::{SessionTask, SessionTaskContext, TaskKind}, TurnContext};

/// Turn executor - handles the main turn execution logic
pub struct TurnExecutor {
    /// Task ID
    id: String,
}

impl TurnExecutor {
    /// Create a new turn executor
    pub fn new(id: String) -> Self {
        Self { id }
    }
}

#[async_trait]
impl SessionTask for TurnExecutor {
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
        let session = session_ctx.session();

        info!("Turn executor started: {}", turn_context.sub_id);

        // Process each input item
        for item in input {
            if cancellation_token.is_cancelled() {
                debug!("Turn cancelled");
                return Ok(());
            }

            // Handle different input types
            match &item {
                UserInput::Text { text, .. } => {
                    debug!("Processing text input: {}", text);
                    // TODO: Send to model and get response
                }
                UserInput::Image { image_url } => {
                    debug!("Processing image: {}", image_url);
                }
                UserInput::LocalImage { path } => {
                    debug!("Processing local image: {:?}", path);
                }
                UserInput::Skill { name, path } => {
                    debug!("Processing skill: {} at {:?}", name, path);
                }
                UserInput::Mention { name, path } => {
                    debug!("Processing mention: {} at {}", name, path);
                }
            }
        }

        info!("Turn executor completed: {}", turn_context.sub_id);
        Ok(())
    }

    async fn abort(
        &self,
        _session_ctx: Arc<SessionTaskContext>,
        _turn_context: Arc<TurnContext>,
    ) {
        debug!("Turn executor aborted: {}", self.id);
    }
}
