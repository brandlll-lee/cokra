// Parallel Execution
// Manages concurrent tool execution

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::tools::context::{ToolOutput, FunctionCallError};
use crate::tools::router::{ToolRouter, ToolCall};

/// Tool call runtime for parallel execution
pub(crate) struct ToolCallRuntime {
    router: Arc<ToolRouter>,
    parallel_execution: Arc<RwLock<()>>,
}

impl ToolCallRuntime {
    pub(crate) fn new(router: Arc<ToolRouter>) -> Self {
        Self {
            router,
            parallel_execution: Arc::new(RwLock::new(())),
        }
    }

    /// Handle tool call
    pub(crate) async fn handle_tool_call(
        self,
        call: ToolCall,
    ) -> Result<ToolOutput, FunctionCallError> {
        // Acquire parallel execution lock if needed
        let _guard = self.parallel_execution.read().await;

        self.router.dispatch_tool_call(call).await
    }
}
