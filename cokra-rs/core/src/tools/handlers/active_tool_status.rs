use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use crate::tools::spec::ToolSourceKind;

pub struct ActiveToolStatusHandler;

#[derive(Debug, Deserialize)]
struct ActiveToolStatusArgs {
  #[serde(default = "default_limit")]
  limit: usize,
}

#[derive(Debug, Serialize)]
struct ActiveToolStatusResponse {
  total_registered: usize,
  active_total: usize,
  active_external_total: usize,
  inactive_external_total: usize,
  by_source: BTreeMap<String, SourceSummary>,
  active_external_tools: Vec<String>,
  inactive_external_tools: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SourceSummary {
  total: usize,
  active: usize,
}

fn default_limit() -> usize {
  12
}

#[async_trait]
impl ToolHandler for ActiveToolStatusHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: ActiveToolStatusArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("active_tool_status missing runtime context".to_string())
    })?;
    let registry = &runtime.tool_registry;
    let specs = registry.list_specs();
    let active_specs = registry.active_specs();
    let mut by_source = BTreeMap::new();

    for source in [
      ToolSourceKind::BuiltinPrimitive,
      ToolSourceKind::BuiltinCollaboration,
      ToolSourceKind::BuiltinWorkflow,
      ToolSourceKind::Mcp,
      ToolSourceKind::Cli,
      ToolSourceKind::Api,
    ] {
      let total = specs.iter().filter(|spec| spec.source_kind == source).count();
      let active = active_specs
        .iter()
        .filter(|spec| spec.source_kind == source)
        .count();
      if total == 0 {
        continue;
      }
      by_source.insert(
        source_label(source).to_string(),
        SourceSummary { total, active },
      );
    }

    let mut active_external = registry.active_external_tool_names();
    let mut inactive_external = registry.inactive_external_tool_names();
    let response = ActiveToolStatusResponse {
      total_registered: specs.len(),
      active_total: active_specs.len(),
      active_external_total: active_external.len(),
      inactive_external_total: inactive_external.len(),
      by_source,
      active_external_tools: truncate_list(&mut active_external, args.limit),
      inactive_external_tools: truncate_list(&mut inactive_external, args.limit),
    };
    let content = serde_json::to_string(&response).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize active_tool_status: {err}"))
    })?;
    Ok(ToolOutput::success(content).with_id(invocation.id))
  }
}

fn source_label(source: ToolSourceKind) -> &'static str {
  match source {
    ToolSourceKind::BuiltinPrimitive => "builtin_primitive",
    ToolSourceKind::BuiltinCollaboration => "builtin_collaboration",
    ToolSourceKind::BuiltinWorkflow => "builtin_workflow",
    ToolSourceKind::Mcp => "mcp",
    ToolSourceKind::Cli => "cli",
    ToolSourceKind::Api => "api",
  }
}

fn truncate_list(items: &mut Vec<String>, limit: usize) -> Vec<String> {
  items.sort();
  items.truncate(limit.max(1));
  items.clone()
}
