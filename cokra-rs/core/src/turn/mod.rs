//! Turn execution module
//!
//! Handles execution of individual turns in a Cokra session

pub mod context;
pub mod executor;
pub mod regular_task;
pub mod task;

pub use context::TurnContext;
pub use executor::{
  Attachment, AttachmentKind, TurnConfig, TurnError, TurnExecutor, TurnResult, UserInput,
};
pub use regular_task::RegularTask;
pub use task::{CancellationToken, SessionTask, TaskKind, TaskMetadata};

use crate::model::ModelClient;
use crate::session::Session;
use crate::tools::registry::ToolRegistry;
use cokra_protocol::EventMsg as Event;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Create a new turn executor
pub fn new_executor(
  model_client: Arc<ModelClient>,
  tool_registry: Arc<ToolRegistry>,
  session: Arc<Session>,
  tx_event: mpsc::Sender<Event>,
  config: TurnConfig,
) -> TurnExecutor {
  TurnExecutor::new(model_client, tool_registry, session, tx_event, config)
}
