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

pub use approval::ApprovalMode;
pub use approval::ToolApproval;
pub use approval::ToolRiskLevel;
pub use events::ToolExecutionEvent;
pub use events::ToolExecutionStage;
pub use events::ToolExecutionStatus;
pub use executor::ToolCatalogMatch;
pub use executor::ToolRuntimeCatalog;
pub use executor::UnifiedToolRuntime;
pub use provider::ApiToolProvider;
pub use provider::BuiltinToolProvider;
pub use provider::CliToolProvider;
pub use provider::McpToolProvider;
pub use provider::ToolProvider;
pub use types::ToolCall;
pub use types::ToolCapabilityFacets;
pub use types::ToolDefinition;
pub use types::ToolResult;
pub use types::ToolResultMetadata;
pub use types::ToolSource;
