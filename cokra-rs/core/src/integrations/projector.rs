use std::sync::Arc;

use anyhow::Result;

use crate::tool_runtime::ToolDefinition;
use crate::tools::ToolHandler;
use crate::tools::ToolSpec;

use super::bootstrap::IntegrationBootstrapSummary;
use super::bootstrap::summarize_declared_bootstrap;
use super::loader::IntegrationCatalog;
use super::manifest::IntegrationKind;
use super::providers::api::project_api_tools;
use super::providers::cli::project_cli_tools;
use super::providers::mcp::merge_mcp_integrations;

pub struct RegisteredIntegrationTool {
  pub spec: ToolSpec,
  pub definition: ToolDefinition,
  pub handler: Arc<dyn ToolHandler>,
}

pub struct ProjectedIntegrations {
  pub effective_mcp: cokra_config::McpConfig,
  pub cli_tools: Vec<RegisteredIntegrationTool>,
  pub api_tools: Vec<RegisteredIntegrationTool>,
  pub bootstrap: Vec<IntegrationBootstrapSummary>,
}

pub fn project_integrations(
  base_mcp: &cokra_config::McpConfig,
  catalog: &IntegrationCatalog,
) -> Result<ProjectedIntegrations> {
  let manifests = catalog
    .manifests
    .iter()
    .filter(|manifest| manifest.manifest.enabled)
    .collect::<Vec<_>>();
  Ok(ProjectedIntegrations {
    effective_mcp: merge_mcp_integrations(base_mcp, &manifests)?,
    cli_tools: project_cli_tools(&manifests)?,
    api_tools: project_api_tools(&manifests)?,
    bootstrap: manifests
      .iter()
      .map(|manifest| summarize_declared_bootstrap(manifest))
      .collect(),
  })
}

pub fn projected_tool_definitions(
  projected: &ProjectedIntegrations,
  kind: IntegrationKind,
) -> Vec<ToolDefinition> {
  match kind {
    IntegrationKind::Cli => projected
      .cli_tools
      .iter()
      .map(|tool| tool.definition.clone())
      .collect(),
    IntegrationKind::Api => projected
      .api_tools
      .iter()
      .map(|tool| tool.definition.clone())
      .collect(),
    IntegrationKind::Mcp => Vec::new(),
  }
}
