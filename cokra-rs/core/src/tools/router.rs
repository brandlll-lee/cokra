// Tool Router
// Routes tool calls to appropriate handlers

use std::sync::Arc;

use crate::tools::context::{ToolInvocation, ToolOutput, ToolPayload, FunctionCallError};
use crate::tools::registry::{ToolRegistry, ConfiguredToolSpec, ToolSpec};

/// Tool call representation
#[derive(Clone, Debug)]
pub struct ToolCall {
    /// Tool name
    pub tool_name: String,

    /// Call ID
    pub call_id: String,

    /// Payload
    pub payload: ToolPayload,
}

/// Source of tool call
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolCallSource {
    /// Direct call
    Direct,
    /// From JS REPL
    JsRepl,
}

/// Tool router
pub struct ToolRouter {
    registry: Arc<ToolRegistry>,
    specs: Vec<ConfiguredToolSpec>,
}

impl ToolRouter {
    /// Create from config
    pub fn from_registry(registry: ToolRegistry, specs: Vec<ConfiguredToolSpec>) -> Self {
        Self {
            registry: Arc::new(registry),
            specs,
        }
    }

    /// Get tool specs
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.specs.iter().map(|s| s.spec.clone()).collect()
    }

    /// Check if tool supports parallel calls
    pub fn tool_supports_parallel(&self, tool_name: &str) -> bool {
        self.specs
            .iter()
            .find(|s| s.spec.name == tool_name)
            .map(|s| s.supports_parallel_tool_calls)
            .unwrap_or(false)
    }

    /// Dispatch tool call
    pub async fn dispatch_tool_call(
        &self,
        call: ToolCall,
    ) -> Result<ToolOutput, FunctionCallError> {
        let invocation = ToolInvocation {
            session_id: "default".to_string(),
            turn_id: "default".to_string(),
            call_id: call.call_id,
            tool_name: call.tool_name.clone(),
            payload: call.payload,
        };

        self.registry.dispatch(invocation).await
    }

    /// Get registry reference
    pub fn registry(&self) -> Arc<ToolRegistry> {
        self.registry.clone()
    }
}
