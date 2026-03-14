use anyhow::Result;

use cokra_config::McpConfig;
use cokra_config::McpServerConfig;
use cokra_config::McpServerTransportConfig;

use crate::integrations::loader::LoadedIntegrationManifest;
use crate::integrations::manifest::IntegrationKind;

pub fn merge_mcp_integrations(
  base: &McpConfig,
  manifests: &[&LoadedIntegrationManifest],
) -> Result<McpConfig> {
  let mut merged = base.clone();

  for loaded in manifests {
    let manifest = &loaded.manifest;
    if manifest.kind != IntegrationKind::Mcp || !manifest.enabled {
      continue;
    }
    merged
      .servers
      .insert(manifest.name.clone(), manifest_to_server_config(manifest)?);
  }

  Ok(merged)
}

fn manifest_to_server_config(
  manifest: &crate::integrations::manifest::IntegrationManifest,
) -> Result<McpServerConfig> {
  let transport = if let Some(command) = &manifest.command {
    McpServerTransportConfig::Stdio {
      command: command.clone(),
      args: manifest.args.clone(),
      env: manifest.env.clone(),
      cwd: manifest.cwd.clone(),
    }
  } else if let Some(url) = &manifest.url {
    McpServerTransportConfig::Http {
      url: url.clone(),
      bearer_token: manifest.bearer_token.clone(),
      headers: manifest.headers.clone(),
    }
  } else {
    anyhow::bail!(
      "MCP integration `{}` must declare either `command` or `url`",
      manifest.name
    );
  };

  Ok(McpServerConfig {
    transport,
    enabled: manifest.enabled,
    required: manifest.required,
    startup_timeout_sec: manifest.startup_timeout_sec,
    tool_timeout_sec: manifest.tool_timeout_sec,
    enabled_tools: manifest.enabled_tools.clone(),
    disabled_tools: manifest.disabled_tools.clone(),
  })
}
