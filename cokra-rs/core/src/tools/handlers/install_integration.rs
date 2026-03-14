use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::exec::ExecExpiration;
use crate::exec::ExecParams;
use crate::exec::WindowsSandboxLevel;
use crate::exec::execute_command;
use crate::exec::format_exec_output;
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

pub struct InstallIntegrationHandler;

#[derive(Debug, Deserialize)]
struct InstallIntegrationArgs {
  name: String,
}

#[derive(Debug, Serialize)]
struct InstallIntegrationResponse {
  name: String,
  command: Vec<String>,
  exit_code: i32,
  status: String,
  activated_tools: Vec<String>,
  output: String,
}

#[async_trait]
impl ToolHandler for InstallIntegrationHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    true
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: InstallIntegrationArgs = invocation.parse_arguments()?;
    let catalog = discover_integrations(invocation.cwd.as_path()).await;
    let manifest = catalog
      .manifests
      .iter()
      .find(|loaded| loaded.manifest.name == args.name)
      .ok_or_else(|| {
        FunctionCallError::RespondToModel(format!("unknown integration: {}", args.name))
      })?;
    let command = manifest
      .manifest
      .install
      .as_ref()
      .and_then(|install| install.run.clone())
      .ok_or_else(|| {
        FunctionCallError::RespondToModel(format!(
          "integration `{}` does not declare an install command",
          manifest.manifest.name
        ))
      })?;
    if command.is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "integration install command must not be empty".to_string(),
      ));
    }

    let output = execute_command(&ExecParams {
      command: command.clone(),
      cwd: invocation.cwd.clone(),
      expiration: ExecExpiration::DefaultTimeout,
      env: Default::default(),
      network: None,
      network_attempt_id: None,
      sandbox_permissions: crate::exec::SandboxPermissions::UseDefault,
      additional_permissions: None,
      windows_sandbox_level: WindowsSandboxLevel::Disabled,
      justification: None,
      prefix_rule: None,
      arg0: None,
    })
    .await
    .map_err(|err| FunctionCallError::Execution(err.to_string()))?;

    let bootstrap = evaluate_bootstrap(manifest, invocation.cwd.as_path()).await;
    let activated_tools = if bootstrap.ready {
      invocation
        .runtime
        .as_ref()
        .map(|runtime| {
          let tool_names = connectable_tool_names(&runtime.tool_registry, manifest);
          runtime.tool_registry.activate_tools(tool_names)
        })
        .unwrap_or_default()
    } else {
      Vec::new()
    };
    let response = InstallIntegrationResponse {
      name: manifest.manifest.name.clone(),
      command,
      exit_code: output.exit_code,
      status: if bootstrap.ready {
        "ready"
      } else {
        "needs_followup"
      }
      .to_string(),
      activated_tools,
      output: format_exec_output(&output),
    };
    let content = serde_json::to_string(&response).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize install_integration: {err}"))
    })?;
    Ok(
      ToolOutput::success(content)
        .with_id(invocation.id)
        .with_success(output.exit_code == 0),
    )
  }
}

fn connectable_tool_names(
  registry: &crate::tools::registry::ToolRegistry,
  manifest: &LoadedIntegrationManifest,
) -> Vec<String> {
  match manifest.manifest.kind {
    IntegrationKind::Cli | IntegrationKind::Api => manifest
      .manifest
      .tools
      .iter()
      .map(|tool| tool.id.clone())
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
      .map(|spec| spec.name)
      .collect(),
  }
}
