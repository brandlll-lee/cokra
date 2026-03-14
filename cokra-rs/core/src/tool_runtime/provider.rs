use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::mcp::McpConnectionManager;
use crate::tools::registry::ToolRegistry;
use crate::tools::spec::JsonSchema;
use crate::tools::spec::ToolSourceKind;
use crate::tools::spec::ToolSpec;

use super::ApprovalMode;
use super::ToolApproval;
use super::ToolCapabilityFacets;
use super::ToolDefinition;
use super::ToolRiskLevel;
use super::ToolSource;

#[async_trait]
pub trait ToolProvider: Send + Sync {
  fn provider_id(&self) -> &str;
  fn source(&self) -> ToolSource;
  async fn list_tools(&self) -> Result<Vec<ToolDefinition>>;
}

#[derive(Clone)]
pub struct BuiltinToolProvider {
  provider_id: String,
  tools: Vec<ToolDefinition>,
}

impl BuiltinToolProvider {
  pub fn from_registry(registry: &ToolRegistry) -> Self {
    let tools = registry
      .list_specs()
      .into_iter()
      .filter(|spec| {
        matches!(
          spec.source_kind,
          ToolSourceKind::BuiltinPrimitive
            | ToolSourceKind::BuiltinCollaboration
            | ToolSourceKind::BuiltinWorkflow
        )
      })
      .map(|spec| {
        let aliases = registry.aliases_for(&spec.name);
        let enabled = !registry.is_excluded(&spec.name);
        tool_definition_from_spec(
          spec,
          ToolSource::Builtin,
          aliases,
          enabled,
          Some("builtin".to_string()),
          None,
          None,
        )
      })
      .collect();

    Self {
      provider_id: "builtin".to_string(),
      tools,
    }
  }
}

#[async_trait]
impl ToolProvider for BuiltinToolProvider {
  fn provider_id(&self) -> &str {
    &self.provider_id
  }

  fn source(&self) -> ToolSource {
    ToolSource::Builtin
  }

  async fn list_tools(&self) -> Result<Vec<ToolDefinition>> {
    Ok(self.tools.clone())
  }
}

#[derive(Clone)]
pub struct McpToolProvider {
  provider_id: String,
  tools: Vec<ToolDefinition>,
}

impl McpToolProvider {
  pub fn from_manager(manager: Arc<McpConnectionManager>) -> Self {
    let descriptors = manager.tool_descriptors();
    let specs_by_name = manager
      .tool_specs()
      .into_iter()
      .map(|spec| (spec.name.clone(), spec))
      .collect::<std::collections::HashMap<_, _>>();

    let tools = descriptors
      .into_iter()
      .filter_map(|descriptor| {
        let spec = specs_by_name.get(&descriptor.exposed_name)?.clone();
        Some(tool_definition_from_spec(
          spec,
          ToolSource::Mcp,
          Vec::new(),
          true,
          Some(format!("mcp:{}", descriptor.server_name)),
          Some(descriptor.server_name),
          Some(descriptor.remote_tool_name),
        ))
      })
      .collect();

    Self {
      provider_id: "mcp".to_string(),
      tools,
    }
  }
}

#[async_trait]
impl ToolProvider for McpToolProvider {
  fn provider_id(&self) -> &str {
    &self.provider_id
  }

  fn source(&self) -> ToolSource {
    ToolSource::Mcp
  }

  async fn list_tools(&self) -> Result<Vec<ToolDefinition>> {
    Ok(self.tools.clone())
  }
}

#[derive(Clone, Default)]
pub struct CliToolProvider {
  provider_id: String,
  tools: Vec<ToolDefinition>,
}

impl CliToolProvider {
  pub fn new(provider_id: impl Into<String>, tools: Vec<ToolDefinition>) -> Self {
    Self {
      provider_id: provider_id.into(),
      tools,
    }
  }
}

#[async_trait]
impl ToolProvider for CliToolProvider {
  fn provider_id(&self) -> &str {
    &self.provider_id
  }

  fn source(&self) -> ToolSource {
    ToolSource::Cli
  }

  async fn list_tools(&self) -> Result<Vec<ToolDefinition>> {
    Ok(self.tools.clone())
  }
}

#[derive(Clone, Default)]
pub struct ApiToolProvider {
  provider_id: String,
  tools: Vec<ToolDefinition>,
}

impl ApiToolProvider {
  pub fn new(provider_id: impl Into<String>, tools: Vec<ToolDefinition>) -> Self {
    Self {
      provider_id: provider_id.into(),
      tools,
    }
  }
}

#[async_trait]
impl ToolProvider for ApiToolProvider {
  fn provider_id(&self) -> &str {
    &self.provider_id
  }

  fn source(&self) -> ToolSource {
    ToolSource::Api
  }

  async fn list_tools(&self) -> Result<Vec<ToolDefinition>> {
    Ok(self.tools.clone())
  }
}

fn collect_input_keys(schema: &JsonSchema) -> Vec<String> {
  match schema {
    JsonSchema::Object { properties, .. } => properties.keys().cloned().collect(),
    _ => Vec::new(),
  }
}

fn source_kind_tag(source_kind: &ToolSourceKind) -> String {
  match source_kind {
    ToolSourceKind::BuiltinPrimitive => "builtin_primitive",
    ToolSourceKind::BuiltinCollaboration => "builtin_collaboration",
    ToolSourceKind::BuiltinWorkflow => "builtin_workflow",
    ToolSourceKind::Cli => "cli",
    ToolSourceKind::Api => "api",
    ToolSourceKind::Mcp => "mcp",
  }
  .to_string()
}

fn derive_tags(
  spec: &ToolSpec,
  source: ToolSource,
  server_name: Option<&str>,
  remote_name: Option<&str>,
) -> Vec<String> {
  let mut tags = HashSet::from([
    match source {
      ToolSource::Builtin => "builtin",
      ToolSource::Mcp => "mcp",
      ToolSource::Cli => "cli",
      ToolSource::Api => "api",
    }
    .to_string(),
    source_kind_tag(&spec.source_kind),
  ]);

  if spec.mutates_state {
    tags.insert("mutating".to_string());
  } else {
    tags.insert("read_only".to_string());
  }

  if spec.permissions.allow_network {
    tags.insert("network".to_string());
  }

  if let Some(server_name) = server_name {
    tags.insert(server_name.to_string());
  }
  if let Some(remote_name) = remote_name {
    tags.insert(remote_name.to_string());
  }

  let mut tags = tags.into_iter().collect::<Vec<_>>();
  tags.sort();
  tags
}

fn normalize_mcp_approval(permission_key: Option<String>) -> ToolApproval {
  ToolApproval {
    risk_level: ToolRiskLevel::Low,
    approval_mode: ApprovalMode::Auto,
    permission_key,
    allow_network: false,
    allow_fs_write: false,
  }
}

fn tool_definition_from_spec(
  spec: ToolSpec,
  source: ToolSource,
  aliases: Vec<String>,
  enabled: bool,
  provider_id: Option<String>,
  server_name: Option<String>,
  remote_name: Option<String>,
) -> ToolDefinition {
  let permission_key = spec.permission_key.clone();
  let approval = if source == ToolSource::Mcp {
    normalize_mcp_approval(permission_key)
  } else {
    ToolApproval::from_permissions(&spec.permissions, permission_key, spec.mutates_state)
  };

  ToolDefinition {
    id: spec.name.clone(),
    name: spec.name.clone(),
    description: spec.description.clone(),
    input_schema: spec.input_schema.to_value(),
    output_schema: spec.output_schema.as_ref().map(JsonSchema::to_value),
    source,
    aliases,
    tags: derive_tags(
      &spec,
      source,
      server_name.as_deref(),
      remote_name.as_deref(),
    ),
    approval,
    enabled,
    supports_parallel: spec.supports_parallel,
    mutates_state: spec.mutates_state,
    input_keys: collect_input_keys(&spec.input_schema),
    capabilities: ToolCapabilityFacets::for_tool_name(&spec.name, spec.permissions.allow_network),
    provider_id,
    source_kind: Some(source_kind_tag(&spec.source_kind)),
    server_name,
    remote_name,
  }
}
