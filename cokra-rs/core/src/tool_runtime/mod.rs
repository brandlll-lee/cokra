//! Unified tool runtime surface.
//!
//! This module keeps the kernel small while giving every tool source the same
//! runtime shape. Builtins, MCP, CLI integrations, and API integrations are all
//! projected into the same `ToolDefinition` model, searched through the same
//! catalog, and normalized into the same execution/result types.

mod approval;
mod events;
mod executor;
mod provider;
mod types;

pub use approval::{ApprovalMode, ToolApproval, ToolRiskLevel};
pub use events::{ToolExecutionEvent, ToolExecutionStage, ToolExecutionStatus};
pub use executor::{ToolCatalogMatch, ToolRuntimeCatalog, UnifiedToolRuntime};
pub use provider::{
  ApiToolProvider, BuiltinToolProvider, CliToolProvider, McpToolProvider, ToolProvider,
};
pub use types::{ToolCall, ToolDefinition, ToolResult, ToolResultMetadata, ToolSource};
