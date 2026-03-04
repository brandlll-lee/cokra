use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::spec::ToolSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
  Function,
  Mcp,
}

/// 1:1 codex: ToolHandler trait with async support.
///
/// Existing synchronous handlers implement `handle()` as before.
/// Async handlers (e.g. shell) override `handle_async()` instead.
/// The default `handle_async` delegates to synchronous `handle`.
#[async_trait]
pub trait ToolHandler: Send + Sync {
  fn kind(&self) -> ToolKind;

  fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    false
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let _ = invocation;
    Err(FunctionCallError::Execution(
      "synchronous handle not implemented; use handle_async".to_string(),
    ))
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    self.handle(invocation)
  }
}

#[derive(Default)]
pub struct ToolRegistry {
  handlers: HashMap<String, Arc<dyn ToolHandler>>,
  specs: HashMap<String, ToolSpec>,
}

impl ToolRegistry {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn register_handler(&mut self, name: impl Into<String>, handler: Arc<dyn ToolHandler>) {
    self.handlers.insert(name.into(), handler);
  }

  pub fn register_spec(&mut self, spec: ToolSpec) {
    self.specs.insert(spec.name.clone(), spec);
  }

  pub fn register_tool(&mut self, spec: ToolSpec, handler: Arc<dyn ToolHandler>) {
    let name = spec.name.clone();
    self.register_spec(spec);
    self.register_handler(name, handler);
  }

  pub fn get_handler(&self, name: &str) -> Option<&Arc<dyn ToolHandler>> {
    self.handlers.get(name)
  }

  pub fn get_spec(&self, name: &str) -> Option<&ToolSpec> {
    self.specs.get(name)
  }

  pub fn list_specs(&self) -> Vec<ToolSpec> {
    self.specs.values().cloned().collect()
  }

  pub fn dispatch(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let handler = self
      .get_handler(&invocation.name)
      .ok_or_else(|| FunctionCallError::ToolNotFound(invocation.name.clone()))?;
    handler.handle(invocation)
  }

  /// 1:1 codex: async dispatch for handlers that need async execution (e.g. shell).
  pub async fn dispatch_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let handler = self
      .get_handler(&invocation.name)
      .ok_or_else(|| FunctionCallError::ToolNotFound(invocation.name.clone()))?;
    handler.handle_async(invocation).await
  }

  pub fn is_mutating(&self, invocation: &ToolInvocation) -> Result<bool, FunctionCallError> {
    let handler = self
      .get_handler(&invocation.name)
      .ok_or_else(|| FunctionCallError::ToolNotFound(invocation.name.clone()))?;
    Ok(handler.is_mutating(invocation))
  }

  pub fn model_tools(&self) -> Vec<crate::model::Tool> {
    self
      .list_specs()
      .into_iter()
      .map(|spec| spec.to_model_tool())
      .collect()
  }
}
