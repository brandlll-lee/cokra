use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::RwLock;

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
  /// Tool name aliases: legacy_name -> current_name.
  /// When dispatching or looking up a tool, aliases are resolved transparently.
  aliases: HashMap<String, String>,
  /// Excluded tool names. Tools in this set are hidden from `model_tools()`
  /// and `active_specs()` but remain registered for potential re-inclusion.
  excluded: RwLock<HashSet<String>>,
  /// External tools can be temporarily deactivated without removing the
  /// underlying integration. Builtins stay always-on unless explicitly
  /// excluded through the existing config-driven exclusion path.
  inactive_external: RwLock<HashSet<String>>,
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

  // ── Alias management ──────────────────────────────────────────────

  /// Register a tool name alias. When the model calls `alias`, it is
  /// transparently resolved to `target` for handler lookup and dispatch.
  pub fn register_alias(&mut self, alias: impl Into<String>, target: impl Into<String>) {
    self.aliases.insert(alias.into(), target.into());
  }

  /// Resolve a tool name through the alias chain (single hop).
  pub fn resolve_name<'a>(&'a self, name: &'a str) -> &'a str {
    match self.aliases.get(name) {
      Some(target) => target.as_str(),
      None => name,
    }
  }

  pub fn aliases_for(&self, target: &str) -> Vec<String> {
    let mut aliases = self
      .aliases
      .iter()
      .filter_map(|(alias, mapped)| (mapped == target).then_some(alias.clone()))
      .collect::<Vec<_>>();
    aliases.sort();
    aliases
  }

  pub fn alias_pairs(&self) -> Vec<(String, String)> {
    let mut pairs = self
      .aliases
      .iter()
      .map(|(alias, target)| (alias.clone(), target.clone()))
      .collect::<Vec<_>>();
    pairs.sort_by(|left, right| left.0.cmp(&right.0));
    pairs
  }

  // ── Exclude management ────────────────────────────────────────────

  /// Exclude a tool by name. Excluded tools are not sent to the model
  /// but remain registered for re-inclusion.
  pub fn exclude_tool(&self, name: impl Into<String>) {
    self
      .excluded
      .write()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .insert(name.into());
  }

  /// Re-include a previously excluded tool.
  pub fn include_tool(&self, name: &str) {
    self
      .excluded
      .write()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .remove(name);
  }

  /// Bulk-set excluded tools from a set of names.
  pub fn set_excluded(&self, names: HashSet<String>) {
    *self
      .excluded
      .write()
      .unwrap_or_else(std::sync::PoisonError::into_inner) = names;
  }

  /// Returns true if the tool is currently excluded.
  pub fn is_excluded(&self, name: &str) -> bool {
    let resolved = self.resolve_name(name);
    let excluded = self
      .excluded
      .read()
      .unwrap_or_else(std::sync::PoisonError::into_inner);
    excluded.contains(resolved) || excluded.contains(name)
  }

  fn is_gateable(spec: &ToolSpec) -> bool {
    matches!(
      spec.source_kind,
      crate::tools::spec::ToolSourceKind::Mcp
        | crate::tools::spec::ToolSourceKind::Cli
        | crate::tools::spec::ToolSourceKind::Api
    )
  }

  pub fn is_active(&self, name: &str) -> bool {
    if self.is_excluded(name) {
      return false;
    }
    let Some(spec) = self.get_spec(name) else {
      return false;
    };
    if !Self::is_gateable(spec) {
      return true;
    }
    let resolved = self.resolve_name(name).to_string();
    !self
      .inactive_external
      .read()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .contains(&resolved)
  }

  pub fn activate_tool(&self, name: &str) -> bool {
    let Some(spec) = self.get_spec(name) else {
      return false;
    };
    if !Self::is_gateable(spec) {
      return false;
    }
    let resolved = self.resolve_name(name).to_string();
    self
      .inactive_external
      .write()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .remove(&resolved)
  }

  pub fn deactivate_tool(&self, name: &str) -> bool {
    let Some(spec) = self.get_spec(name) else {
      return false;
    };
    if !Self::is_gateable(spec) {
      return false;
    }
    let resolved = self.resolve_name(name).to_string();
    self
      .inactive_external
      .write()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .insert(resolved)
  }

  pub fn activate_tools<I>(&self, names: I) -> Vec<String>
  where
    I: IntoIterator,
    I::Item: AsRef<str>,
  {
    names
      .into_iter()
      .filter_map(|name| {
        let name = name.as_ref();
        if self.activate_tool(name) {
          Some(self.resolve_name(name).to_string())
        } else {
          None
        }
      })
      .collect()
  }

  pub fn deactivate_tools<I>(&self, names: I) -> Vec<String>
  where
    I: IntoIterator,
    I::Item: AsRef<str>,
  {
    names
      .into_iter()
      .filter_map(|name| {
        let name = name.as_ref();
        if self.deactivate_tool(name) {
          Some(self.resolve_name(name).to_string())
        } else {
          None
        }
      })
      .collect()
  }

  pub fn reset_active_external_tools(&self) {
    self
      .inactive_external
      .write()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .clear();
  }

  pub fn active_external_tool_names(&self) -> Vec<String> {
    let mut names = self
      .specs
      .values()
      .filter(|spec| Self::is_gateable(spec))
      .filter(|spec| self.is_active(&spec.name))
      .map(|spec| spec.name.clone())
      .collect::<Vec<_>>();
    names.sort();
    names
  }

  pub fn inactive_external_tool_names(&self) -> Vec<String> {
    let mut names = self
      .specs
      .values()
      .filter(|spec| Self::is_gateable(spec))
      .filter(|spec| !self.is_active(&spec.name))
      .map(|spec| spec.name.clone())
      .collect::<Vec<_>>();
    names.sort();
    names
  }

  // ── Lookup (alias-aware, exclude-aware) ───────────────────────────

  pub fn get_handler(&self, name: &str) -> Option<&Arc<dyn ToolHandler>> {
    let resolved = self.resolve_name(name);
    self
      .handlers
      .get(resolved)
      .or_else(|| self.handlers.get(name))
  }

  pub fn get_spec(&self, name: &str) -> Option<&ToolSpec> {
    let resolved = self.resolve_name(name);
    self.specs.get(resolved).or_else(|| self.specs.get(name))
  }

  /// All registered specs (unfiltered).
  pub fn list_specs(&self) -> Vec<ToolSpec> {
    self.specs.values().cloned().collect()
  }

  /// Only non-excluded specs, for sending to the model.
  pub fn active_specs(&self) -> Vec<ToolSpec> {
    self
      .specs
      .values()
      .filter(|s| self.is_active(&s.name))
      .cloned()
      .collect()
  }

  /// All active (non-excluded) tool names.
  pub fn active_tool_names(&self) -> Vec<String> {
    self.active_specs().into_iter().map(|s| s.name).collect()
  }

  // ── Dispatch (alias-aware) ────────────────────────────────────────

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

  /// Returns tool definitions for the model, excluding tools in the excluded set.
  pub fn model_tools(&self) -> Vec<crate::model::Tool> {
    self
      .active_specs()
      .into_iter()
      .map(|spec| spec.to_model_tool())
      .collect()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::tools::spec::JsonSchema;
  use crate::tools::spec::ToolHandlerType;
  use crate::tools::spec::ToolPermissions;
  use std::collections::BTreeMap;

  struct DummyHandler;
  #[async_trait]
  impl ToolHandler for DummyHandler {
    fn kind(&self) -> ToolKind {
      ToolKind::Function
    }
    fn handle(&self, inv: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
      Ok(ToolOutput::success("ok").with_id(inv.id))
    }
  }

  fn dummy_spec(name: &str) -> ToolSpec {
    ToolSpec::new(
      name,
      "test tool",
      JsonSchema::Object {
        properties: BTreeMap::new(),
        required: Some(vec![]),
        additional_properties: None,
      },
      None,
      ToolHandlerType::Function,
      ToolPermissions::default(),
    )
  }

  #[test]
  fn alias_resolves_to_target() {
    let mut reg = ToolRegistry::new();
    reg.register_tool(dummy_spec("shell"), Arc::new(DummyHandler));
    reg.register_alias("container.exec", "shell");

    assert!(reg.get_handler("container.exec").is_some());
    assert!(reg.get_spec("container.exec").is_some());
    assert_eq!(reg.resolve_name("container.exec"), "shell");
    assert_eq!(reg.resolve_name("shell"), "shell");
  }

  #[test]
  fn excluded_tool_hidden_from_model_tools() {
    let mut reg = ToolRegistry::new();
    reg.register_tool(dummy_spec("edit_file"), Arc::new(DummyHandler));
    reg.register_tool(dummy_spec("apply_patch"), Arc::new(DummyHandler));

    assert_eq!(reg.model_tools().len(), 2);

    reg.exclude_tool("apply_patch");
    let tools = reg.model_tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].function.as_ref().unwrap().name, "edit_file");

    // Handler still accessible for dispatch (not removed, just hidden)
    assert!(reg.get_handler("apply_patch").is_some());
  }

  #[test]
  fn include_tool_re_enables() {
    let mut reg = ToolRegistry::new();
    reg.register_tool(dummy_spec("glob"), Arc::new(DummyHandler));

    reg.exclude_tool("glob");
    assert_eq!(reg.model_tools().len(), 0);

    reg.include_tool("glob");
    assert_eq!(reg.model_tools().len(), 1);
  }

  #[test]
  fn alias_excluded_also_hides() {
    let mut reg = ToolRegistry::new();
    reg.register_tool(dummy_spec("shell"), Arc::new(DummyHandler));
    reg.register_alias("exec", "shell");

    reg.exclude_tool("shell");
    assert!(reg.is_excluded("shell"));
    assert!(reg.is_excluded("exec"));
  }

  #[test]
  fn active_specs_filters_excluded() {
    let mut reg = ToolRegistry::new();
    reg.register_tool(dummy_spec("a"), Arc::new(DummyHandler));
    reg.register_tool(dummy_spec("b"), Arc::new(DummyHandler));
    reg.register_tool(dummy_spec("c"), Arc::new(DummyHandler));

    reg.exclude_tool("b");
    let names: HashSet<String> = reg.active_specs().into_iter().map(|s| s.name).collect();
    assert!(names.contains("a"));
    assert!(!names.contains("b"));
    assert!(names.contains("c"));
  }

  #[test]
  fn set_excluded_bulk() {
    let mut reg = ToolRegistry::new();
    reg.register_tool(dummy_spec("x"), Arc::new(DummyHandler));
    reg.register_tool(dummy_spec("y"), Arc::new(DummyHandler));

    reg.set_excluded(HashSet::from(["x".to_string(), "y".to_string()]));
    assert_eq!(reg.model_tools().len(), 0);
    assert_eq!(reg.active_specs().len(), 0);
  }

  #[test]
  fn deactivate_external_hides_only_gateable_tools() {
    let mut reg = ToolRegistry::new();
    let builtin = dummy_spec("read_file");
    let external = dummy_spec("echo_demo").with_source_kind(crate::tools::spec::ToolSourceKind::Cli);
    reg.register_tool(builtin, Arc::new(DummyHandler));
    reg.register_tool(external, Arc::new(DummyHandler));

    assert!(reg.deactivate_tool("echo_demo"));
    assert!(!reg.is_active("echo_demo"));
    assert!(reg.is_active("read_file"));
    assert!(reg.model_tools().iter().all(|tool| {
      tool
        .function
        .as_ref()
        .is_some_and(|function| function.name != "echo_demo")
    }));
  }
}
