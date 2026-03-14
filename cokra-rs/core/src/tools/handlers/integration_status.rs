use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::integrations::bootstrap::evaluate_bootstrap;
use crate::integrations::discover_integrations;
use crate::integrations::loader::LoadedIntegrationManifest;
use crate::integrations::manifest::IntegrationKind;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use crate::tools::spec::ToolSourceKind;

pub struct IntegrationStatusHandler;

#[derive(Debug, Deserialize)]
struct IntegrationStatusArgs {
  name: Option<String>,
}

#[derive(Debug, Serialize)]
struct IntegrationStatusResponse {
  integrations: Vec<IntegrationStatusEntry>,
}

#[derive(Debug, Serialize)]
struct IntegrationStatusEntry {
  name: String,
  kind: String,
  scope: String,
  enabled: bool,
  status: crate::integrations::bootstrap::IntegrationBootstrapSummary,
  declared_tool_ids: Vec<String>,
  active_tool_ids: Vec<String>,
}

#[async_trait]
impl ToolHandler for IntegrationStatusHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: IntegrationStatusArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("integration_status missing runtime context".to_string())
    })?;
    let catalog = discover_integrations(invocation.cwd.as_path()).await;
    let mut entries = Vec::new();
    for manifest in &catalog.manifests {
      if let Some(name) = &args.name
        && manifest.manifest.name != name.trim()
      {
        continue;
      }
      let active_tool_ids = active_tool_ids_for_manifest(&runtime.tool_registry, manifest);
      entries.push(IntegrationStatusEntry {
        name: manifest.manifest.name.clone(),
        kind: kind_label(manifest.manifest.kind),
        scope: match manifest.scope {
          crate::integrations::loader::IntegrationScope::Project => "project",
          crate::integrations::loader::IntegrationScope::User => "user",
        }
        .to_string(),
        enabled: manifest.manifest.enabled,
        status: evaluate_bootstrap(manifest, invocation.cwd.as_path()).await,
        declared_tool_ids: declared_tool_ids(manifest),
        active_tool_ids,
      });
    }

    let content = serde_json::to_string(&IntegrationStatusResponse {
      integrations: entries,
    })
    .map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize integration_status: {err}"))
    })?;
    Ok(ToolOutput::success(content).with_id(invocation.id))
  }
}

fn declared_tool_ids(manifest: &LoadedIntegrationManifest) -> Vec<String> {
  match manifest.manifest.kind {
    IntegrationKind::Cli | IntegrationKind::Api => manifest
      .manifest
      .tools
      .iter()
      .map(|tool| tool.id.clone())
      .collect(),
    IntegrationKind::Mcp => Vec::new(),
  }
}

fn active_tool_ids_for_manifest(
  registry: &crate::tools::registry::ToolRegistry,
  manifest: &LoadedIntegrationManifest,
) -> Vec<String> {
  match manifest.manifest.kind {
    IntegrationKind::Cli | IntegrationKind::Api => manifest
      .manifest
      .tools
      .iter()
      .map(|tool| tool.id.clone())
      .filter(|tool_id| registry.is_active(tool_id))
      .collect(),
    IntegrationKind::Mcp => registry
      .list_specs()
      .into_iter()
      .filter(|spec| spec.source_kind == ToolSourceKind::Mcp)
      .filter(|spec| {
        spec
          .name
          .starts_with(&format!("mcp__{}__", manifest.manifest.name))
      })
      .filter(|spec| registry.is_active(&spec.name))
      .map(|spec| spec.name)
      .collect(),
  }
}

fn kind_label(kind: IntegrationKind) -> String {
  match kind {
    IntegrationKind::Mcp => "mcp",
    IntegrationKind::Cli => "cli",
    IntegrationKind::Api => "api",
  }
  .to_string()
}
