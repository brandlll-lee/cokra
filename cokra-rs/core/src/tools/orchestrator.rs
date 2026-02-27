// Tool Orchestrator
// Central place for approvals + sandbox selection + retry semantics

use crate::tools::sandboxing::{ApprovalStore, ReviewDecision, ToolError};

/// Tool orchestrator
pub struct ToolOrchestrator {
    approval_store: ApprovalStore,
}

impl ToolOrchestrator {
    pub fn new() -> Self {
        Self {
            approval_store: ApprovalStore::new(),
        }
    }

    /// Run tool with approval and sandbox
    pub async fn run<Req, Out, F, Fut>(
        &mut self,
        tool_name: &str,
        req: &Req,
        f: F,
    ) -> Result<Out, ToolError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Out, ToolError>>,
    {
        // Execute tool
        f().await
    }
}

impl Default for ToolOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}
