//! Turn execution module
//!
//! Handles execution of individual turns in a Cokra session

pub mod context;
pub mod executor;
pub mod regular_task;
pub mod response_items;
pub mod sse_executor;
pub mod task;
mod text_function_calls;

pub use context::TurnContext;
pub use executor::TurnConfig;
pub use executor::TurnError;
pub use executor::TurnExecutor;
pub use executor::TurnResult;
pub use executor::UserInput;
pub use task::CancellationToken;
pub use task::SessionTask;
pub use task::TaskKind;
pub use task::TaskMetadata;

use crate::model::ModelClient;
use crate::session::Session;
use crate::tools::registry::ToolRegistry;
use crate::tools::router::ToolRouter;
use cokra_protocol::EventMsg as Event;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Create a new turn executor
pub fn new_executor(
  model_client: Arc<ModelClient>,
  tool_registry: Arc<ToolRegistry>,
  tool_router: Arc<ToolRouter>,
  session: Arc<Session>,
  tx_event: mpsc::Sender<Event>,
  config: TurnConfig,
) -> TurnExecutor {
  TurnExecutor::new(
    model_client,
    tool_registry,
    tool_router,
    session,
    tx_event,
    config,
  )
}
