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
  model_provider_id: Option<String>,
  model_runtime_kind: Option<String>,
  provider_native_web_search: bool,
  by_source: BTreeMap<String, SourceSummary>,
  network_backends: Vec<String>,
  semantic_lsp_tools: usize,
  interactive_exec_tools: usize,
  lsp_clients_connected: usize,
  lsp_clients_broken: usize,
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
    let active_definitions = active_specs
      .iter()
      .map(|spec| crate::tool_runtime::ToolDefinition {
        id: spec.name.clone(),
        name: spec.name.clone(),
        description: spec.description.clone(),
        input_schema: spec.input_schema.to_value(),
        output_schema: spec.output_schema.as_ref().map(|schema| schema.to_value()),
        source: match spec.source_kind {
          ToolSourceKind::BuiltinPrimitive
          | ToolSourceKind::BuiltinCollaboration
          | ToolSourceKind::BuiltinWorkflow => crate::tool_runtime::ToolSource::Builtin,
          ToolSourceKind::Mcp => crate::tool_runtime::ToolSource::Mcp,
          ToolSourceKind::Cli => crate::tool_runtime::ToolSource::Cli,
          ToolSourceKind::Api => crate::tool_runtime::ToolSource::Api,
        },
        aliases: registry.aliases_for(&spec.name),
        tags: Vec::new(),
        approval: crate::tool_runtime::ToolApproval::from_permissions(
          &spec.permissions,
          spec.permission_key.clone(),
          spec.mutates_state,
        ),
        enabled: registry.is_active(&spec.name),
        supports_parallel: spec.supports_parallel,
        mutates_state: spec.mutates_state,
        input_keys: match &spec.input_schema {
          crate::tools::spec::JsonSchema::Object { properties, .. } => {
            properties.keys().cloned().collect()
          }
          _ => Vec::new(),
        },
        capabilities: crate::tool_runtime::ToolCapabilityFacets::for_tool_name(
          &spec.name,
          spec.permissions.allow_network,
        ),
        provider_id: None,
        source_kind: Some(source_label(spec.source_kind).to_string()),
        server_name: None,
        remote_name: None,
      })
      .collect::<Vec<_>>();
    let mut network_backends = active_definitions
      .iter()
      .flat_map(|tool| tool.capabilities.network_backends.iter().cloned())
      .collect::<Vec<_>>();
    if runtime.supports_native_web_search {
      network_backends.push("provider_native_openai_codex".to_string());
    }
    network_backends.sort();
    network_backends.dedup();
    let semantic_lsp_tools = active_definitions
      .iter()
      .filter(|tool| tool.capabilities.semantic_lsp)
      .count();
    let interactive_exec_tools = active_definitions
      .iter()
      .filter(|tool| tool.capabilities.interactive_exec)
      .count();
    let lsp_status = crate::lsp::manager().status().await;

    for source in [
      ToolSourceKind::BuiltinPrimitive,
      ToolSourceKind::BuiltinCollaboration,
      ToolSourceKind::BuiltinWorkflow,
      ToolSourceKind::Mcp,
      ToolSourceKind::Cli,
      ToolSourceKind::Api,
    ] {
      let total = specs
        .iter()
        .filter(|spec| spec.source_kind == source)
        .count();
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
      model_provider_id: runtime.model_provider_id.clone(),
      model_runtime_kind: runtime.model_runtime_kind.clone(),
      provider_native_web_search: runtime.supports_native_web_search,
      by_source,
      network_backends,
      semantic_lsp_tools,
      interactive_exec_tools,
      lsp_clients_connected: lsp_status
        .clients
        .iter()
        .filter(|client| client.status == "connected")
        .count(),
      lsp_clients_broken: lsp_status
        .clients
        .iter()
        .filter(|client| client.status == "broken")
        .count(),
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

#[cfg(test)]
mod tests {
  use std::collections::BTreeMap;
  use std::sync::Arc;

  use super::*;
  use crate::session::Session;
  use crate::tools::context::ToolPayload;
  use crate::tools::context::ToolRuntimeContext;
  use crate::tools::registry::ToolRegistry;
  use crate::tools::spec::JsonSchema;
  use crate::tools::spec::ToolHandlerType;
  use crate::tools::spec::ToolPermissions;
  use crate::tools::spec::ToolSpec;
  use cokra_protocol::AskForApproval;

  fn tool_spec(name: &str, permissions: ToolPermissions) -> ToolSpec {
    ToolSpec::new(
      name,
      "test tool",
      JsonSchema::Object {
        properties: BTreeMap::new(),
        required: Some(Vec::new()),
        additional_properties: Some(false.into()),
      },
      None,
      ToolHandlerType::Function,
      permissions,
    )
  }

  fn runtime_context(
    session: Arc<Session>,
    tool_registry: Arc<ToolRegistry>,
  ) -> Arc<ToolRuntimeContext> {
    Arc::new(ToolRuntimeContext {
      session,
      tool_registry,
      tx_event: None,
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      approval_policy: AskForApproval::OnRequest,
      model_provider_id: Some("openai".to_string()),
      model_runtime_kind: Some("openai_codex".to_string()),
      supports_native_web_search: true,
      has_managed_network_requirements: false,
      allowed_domains: Vec::new(),
      denied_domains: Vec::new(),
      network_attempt_id: None,
    })
  }

  #[tokio::test]
  async fn reports_capability_facets_and_runtime_context() {
    let mut registry = ToolRegistry::new();
    registry.register_spec(tool_spec(
      "web_search",
      ToolPermissions {
        requires_approval: true,
        allow_network: true,
        allow_fs_write: false,
      },
    ));
    registry.register_spec(tool_spec("lsp", ToolPermissions::default()));
    let tool_registry = Arc::new(registry);
    let runtime = runtime_context(Arc::new(Session::new()), Arc::clone(&tool_registry));

    let output = ActiveToolStatusHandler
      .handle_async(ToolInvocation {
        id: "status-1".to_string(),
        name: "active_tool_status".to_string(),
        payload: ToolPayload::Function {
          arguments: "{}".to_string(),
        },
        cwd: std::env::temp_dir(),
        runtime: Some(runtime),
      })
      .await
      .expect("status succeeds");

    let parsed: serde_json::Value =
      serde_json::from_str(&output.text_content()).expect("valid json");
    let backends = parsed["network_backends"]
      .as_array()
      .expect("backends array")
      .iter()
      .filter_map(serde_json::Value::as_str)
      .collect::<Vec<_>>();
    assert_eq!(parsed["model_provider_id"], "openai");
    assert_eq!(parsed["model_runtime_kind"], "openai_codex");
    assert_eq!(parsed["provider_native_web_search"], true);
    assert_eq!(parsed["semantic_lsp_tools"], 1);
    assert!(backends.contains(&"provider_native_openai_codex"));
    assert!(backends.contains(&"local_exa"));
  }
}
