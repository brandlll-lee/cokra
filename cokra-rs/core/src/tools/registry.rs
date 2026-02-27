// Tool Registry
// Central dispatcher for tool handlers

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::tools::context::{ToolInvocation, ToolOutput, FunctionCallError};

/// Tool kind classification
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolKind {
    Function,
    Mcp,
}

/// Core trait all tool handlers must implement
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// Get tool kind
    fn kind(&self) -> ToolKind;

    /// Check if payload matches this handler's kind
    fn matches_kind(&self, payload: &crate::tools::context::ToolPayload) -> bool {
        matches!(
            (self.kind(), payload),
            (ToolKind::Function, crate::tools::context::ToolPayload::Function { .. })
                | (ToolKind::Mcp, crate::tools::context::ToolPayload::Mcp { .. })
        )
    }

    /// Whether this tool is mutating (writes to filesystem)
    async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    /// Handle the tool invocation
    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError>;
}

/// Tool registry stores handlers by name
pub struct ToolRegistry {
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
}

impl ToolRegistry {
    /// Create new registry
    pub fn new(handlers: HashMap<String, Arc<dyn ToolHandler>>) -> Self {
        Self { handlers }
    }

    /// Get handler by name
    pub fn handler(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        self.handlers.get(name).cloned()
    }

    /// Check if tool exists
    pub fn has_tool(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    /// List all tool names
    pub fn tool_names(&self) -> Vec<&str> {
        self.handlers.keys().map(|s| s.as_str()).collect()
    }

    /// Dispatch tool call to handler
    pub async fn dispatch(
        &self,
        invocation: ToolInvocation,
    ) -> Result<ToolOutput, FunctionCallError> {
        let handler = self.handler(&invocation.tool_name)
            .ok_or_else(|| FunctionCallError::ToolNotFound(invocation.tool_name.clone()))?;

        handler.handle(invocation).await
    }
}

/// Builder for constructing registries
pub struct ToolRegistryBuilder {
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
    specs: Vec<ConfiguredToolSpec>,
}

impl ToolRegistryBuilder {
    /// Create new builder
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            specs: Vec::new(),
        }
    }

    /// Register a tool handler
    pub fn register_handler(&mut self, name: impl Into<String>, handler: Arc<dyn ToolHandler>) {
        self.handlers.insert(name.into(), handler);
    }

    /// Add tool spec
    pub fn push_spec(&mut self, spec: ToolSpec) {
        self.specs.push(ConfiguredToolSpec {
            spec,
            supports_parallel_tool_calls: true,
        });
    }

    /// Add tool spec with parallel support flag
    pub fn push_spec_with_parallel_support(&mut self, spec: ToolSpec, supports_parallel: bool) {
        self.specs.push(ConfiguredToolSpec {
            spec,
            supports_parallel_tool_calls: supports_parallel,
        })
    }

    /// Build registry and specs
    pub fn build(self) -> (Vec<ConfiguredToolSpec>, ToolRegistry) {
        (
            self.specs,
            ToolRegistry::new(self.handlers),
        )
    }
}

impl Default for ToolRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Configured tool spec with metadata
#[derive(Debug, Clone)]
pub struct ConfiguredToolSpec {
    /// Tool specification
    pub spec: ToolSpec,
    /// Whether tool supports parallel calls
    pub supports_parallel_tool_calls: bool,
}

/// Tool specification
#[derive(Debug, Clone)]
pub struct ToolSpec {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// Parameters schema
    pub parameters: serde_json::Value,
}

impl ToolSpec {
    /// Create new tool spec
    pub fn new(name: &str, description: &str, parameters: serde_json::Value) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
        }
    }
}
