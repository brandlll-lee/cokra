use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::integrations::bootstrap::IntegrationBootstrapStatus;
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

pub struct ConnectIntegrationHandler;

#[derive(Debug, Deserialize)]
struct ConnectIntegrationArgs {
  name: String,
}

#[derive(Debug, Serialize)]
struct ConnectIntegrationResponse {
  name: String,
  status: String,
  activated_tools: Vec<String>,
  missing_prerequisites: Vec<String>,
}

#[async_trait]
impl ToolHandler for ConnectIntegrationHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: ConnectIntegrationArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("connect_integration missing runtime context".to_string())
    })?;
    let catalog = discover_integrations(invocation.cwd.as_path()).await;
    let manifest = catalog
      .manifests
      .iter()
      .find(|loaded| loaded.manifest.name == args.name)
      .ok_or_else(|| FunctionCallError::RespondToModel(format!("unknown integration: {}", args.name)))?;
    let bootstrap = evaluate_bootstrap(manifest, invocation.cwd.as_path()).await;
    let mut missing_prerequisites = Vec::new();
    if matches!(bootstrap.status, IntegrationBootstrapStatus::NeedsInstall) {
      missing_prerequisites.push("install".to_string());
    }
    if matches!(bootstrap.status, IntegrationBootstrapStatus::NeedsAuth) {
      missing_prerequisites.extend(bootstrap.missing_auth_env.clone());
    }

    let activated_tools = if bootstrap.ready {
      let tool_names = connectable_tool_names(&runtime.tool_registry, manifest);
      runtime.tool_registry.activate_tools(&tool_names)
    } else {
      Vec::new()
    };

    let response = ConnectIntegrationResponse {
      name: manifest.manifest.name.clone(),
      status: match bootstrap.status {
        IntegrationBootstrapStatus::Ready => "connected",
        IntegrationBootstrapStatus::NeedsInstall => "needs_install",
        IntegrationBootstrapStatus::NeedsAuth => "needs_auth",
      }
      .to_string(),
      activated_tools,
      missing_prerequisites,
    };
    let content = serde_json::to_string(&response).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize connect_integration: {err}"))
    })?;
    Ok(ToolOutput::success(content).with_id(invocation.id))
  }
}

fn connectable_tool_names(
  registry: &crate::tools::registry::ToolRegistry,
  manifest: &LoadedIntegrationManifest,
) -> Vec<String> {
  match manifest.manifest.kind {
    IntegrationKind::Cli | IntegrationKind::Api => {
      manifest.manifest.tools.iter().map(|tool| tool.id.clone()).collect()
    }
    IntegrationKind::Mcp => registry
      .list_specs()
      .into_iter()
      .filter(|spec| spec.source_kind == ToolSourceKind::Mcp)
      .filter(|spec| spec.name.starts_with(&format!("mcp__{}__", manifest.manifest.name)))
      .map(|spec| spec.name)
      .collect(),
  }
}
