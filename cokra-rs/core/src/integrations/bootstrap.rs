use std::path::Path;

use serde::Serialize;

use crate::exec::ExecExpiration;
use crate::exec::ExecParams;
use crate::exec::WindowsSandboxLevel;
use crate::exec::execute_command;

use super::loader::IntegrationScope;
use super::loader::LoadedIntegrationManifest;
use super::manifest::IntegrationHealthcheck;
use super::manifest::IntegrationKind;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationBootstrapStatus {
  Ready,
  NeedsInstall,
  NeedsAuth,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationHealthStatus {
  NotConfigured,
  Healthy,
  Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct IntegrationHealthcheckSummary {
  pub status: IntegrationHealthStatus,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IntegrationBootstrapSummary {
  pub name: String,
  pub kind: String,
  pub scope: String,
  pub status: IntegrationBootstrapStatus,
  pub ready: bool,
  pub install_command: Option<Vec<String>>,
  pub install_check: Option<Vec<String>>,
  pub auth_env: Vec<String>,
  pub missing_auth_env: Vec<String>,
  pub location: String,
  pub tool_count: usize,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub healthcheck: Option<IntegrationHealthcheckSummary>,
}

pub async fn summarize_bootstrap(
  manifest: &LoadedIntegrationManifest,
  cwd: &Path,
) -> IntegrationBootstrapSummary {
  evaluate_bootstrap(manifest, cwd).await
}

pub fn summarize_declared_bootstrap(
  manifest: &LoadedIntegrationManifest,
) -> IntegrationBootstrapSummary {
  let auth_env = manifest
    .manifest
    .auth
    .as_ref()
    .map(|auth| auth.env.clone())
    .unwrap_or_default();
  let install_command = manifest
    .manifest
    .install
    .as_ref()
    .and_then(|install| install.run.clone());
  let install_check = manifest
    .manifest
    .install
    .as_ref()
    .and_then(|install| install.check.clone());
  let status = if !auth_env.is_empty() {
    IntegrationBootstrapStatus::NeedsAuth
  } else if install_command.is_some() || install_check.is_some() {
    IntegrationBootstrapStatus::NeedsInstall
  } else {
    IntegrationBootstrapStatus::Ready
  };

  IntegrationBootstrapSummary {
    name: manifest.manifest.name.clone(),
    kind: kind_label(manifest.manifest.kind),
    scope: scope_label(manifest.scope),
    ready: matches!(status, IntegrationBootstrapStatus::Ready),
    status,
    install_command,
    install_check,
    auth_env,
    missing_auth_env: Vec::new(),
    location: manifest.location.display().to_string(),
    tool_count: manifest.manifest.tools.len(),
    healthcheck: manifest
      .manifest
      .healthcheck
      .as_ref()
      .map(|_| IntegrationHealthcheckSummary {
        status: IntegrationHealthStatus::NotConfigured,
        detail: Some("probe not run during startup projection".to_string()),
      }),
  }
}

pub async fn evaluate_bootstrap(
  manifest: &LoadedIntegrationManifest,
  cwd: &Path,
) -> IntegrationBootstrapSummary {
  let auth_env = manifest
    .manifest
    .auth
    .as_ref()
    .map(|auth| auth.env.clone())
    .unwrap_or_default();
  let missing_auth_env = auth_env
    .iter()
    .filter(|key| std::env::var(key.as_str()).unwrap_or_default().trim().is_empty())
    .cloned()
    .collect::<Vec<_>>();
  let install_command = manifest
    .manifest
    .install
    .as_ref()
    .and_then(|install| install.run.clone());
  let install_check = manifest
    .manifest
    .install
    .as_ref()
    .and_then(|install| install.check.clone());
  let install_ready = if let Some(check) = &install_check {
    run_command_probe(check, cwd).await
  } else {
    install_command.is_none()
  };
  let status = if !missing_auth_env.is_empty() {
    IntegrationBootstrapStatus::NeedsAuth
  } else if !install_ready {
    IntegrationBootstrapStatus::NeedsInstall
  } else {
    IntegrationBootstrapStatus::Ready
  };

  IntegrationBootstrapSummary {
    name: manifest.manifest.name.clone(),
    kind: kind_label(manifest.manifest.kind),
    scope: scope_label(manifest.scope),
    ready: matches!(status, IntegrationBootstrapStatus::Ready),
    status,
    install_command,
    install_check,
    auth_env,
    missing_auth_env,
    location: manifest.location.display().to_string(),
    tool_count: manifest.manifest.tools.len(),
    healthcheck: evaluate_healthcheck(manifest, cwd).await,
  }
}

pub async fn evaluate_healthcheck(
  manifest: &LoadedIntegrationManifest,
  cwd: &Path,
) -> Option<IntegrationHealthcheckSummary> {
  let healthcheck = manifest.manifest.healthcheck.as_ref()?;
  let summary = match healthcheck {
    IntegrationHealthcheck::Command { run } => {
      if run_command_probe(run, cwd).await {
        IntegrationHealthcheckSummary {
          status: IntegrationHealthStatus::Healthy,
          detail: Some("command probe succeeded".to_string()),
        }
      } else {
        IntegrationHealthcheckSummary {
          status: IntegrationHealthStatus::Failed,
          detail: Some("command probe failed".to_string()),
        }
      }
    }
    IntegrationHealthcheck::Http {
      method,
      url,
      headers,
    } => {
      let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
      {
        Ok(client) => client,
        Err(err) => {
          return Some(IntegrationHealthcheckSummary {
            status: IntegrationHealthStatus::Failed,
            detail: Some(format!("failed to build HTTP client: {err}")),
          });
        }
      };

      let method = method
        .as_deref()
        .unwrap_or("GET")
        .parse::<reqwest::Method>()
        .unwrap_or(reqwest::Method::GET);
      let mut request = client.request(method, url);
      for (key, value) in headers {
        request = request.header(key, value);
      }

      match request.send().await {
        Ok(response) if response.status().is_success() => IntegrationHealthcheckSummary {
          status: IntegrationHealthStatus::Healthy,
          detail: Some(format!("http {}", response.status().as_u16())),
        },
        Ok(response) => IntegrationHealthcheckSummary {
          status: IntegrationHealthStatus::Failed,
          detail: Some(format!("http {}", response.status().as_u16())),
        },
        Err(err) => IntegrationHealthcheckSummary {
          status: IntegrationHealthStatus::Failed,
          detail: Some(err.to_string()),
        },
      }
    }
  };

  Some(summary)
}

async fn run_command_probe(command: &[String], cwd: &Path) -> bool {
  if command.is_empty() {
    return false;
  }

  execute_command(&ExecParams {
    command: command.to_vec(),
    cwd: cwd.to_path_buf(),
    expiration: ExecExpiration::Timeout(std::time::Duration::from_secs(5)),
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
  .map(|output| output.exit_code == 0)
  .unwrap_or(false)
}

fn kind_label(kind: IntegrationKind) -> String {
  match kind {
    IntegrationKind::Mcp => "mcp",
    IntegrationKind::Cli => "cli",
    IntegrationKind::Api => "api",
  }
  .to_string()
}

fn scope_label(scope: IntegrationScope) -> String {
  match scope {
    IntegrationScope::Project => "project",
    IntegrationScope::User => "user",
  }
  .to_string()
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::integrations::loader::IntegrationScope;
  use crate::integrations::manifest::IntegrationAuth;
  use crate::integrations::manifest::IntegrationManifest;

  #[tokio::test]
  async fn evaluate_bootstrap_flags_missing_auth_env() {
    let manifest = LoadedIntegrationManifest {
      manifest: IntegrationManifest {
        name: "demo".to_string(),
        kind: IntegrationKind::Api,
        enabled: true,
        install: None,
        auth: Some(IntegrationAuth {
          env: vec!["DEMO_INTEGRATION_TOKEN".to_string()],
          optional: false,
        }),
        healthcheck: None,
        discovery: None,
        tools: Vec::new(),
        command: None,
        args: Vec::new(),
        env: None,
        cwd: None,
        url: None,
        bearer_token: None,
        headers: None,
        required: false,
        startup_timeout_sec: None,
        tool_timeout_sec: None,
        enabled_tools: None,
        disabled_tools: None,
      },
      location: std::env::temp_dir().join("demo.toml"),
      scope: IntegrationScope::Project,
    };

    let summary = evaluate_bootstrap(&manifest, std::path::Path::new(".")).await;
    assert_eq!(summary.status, IntegrationBootstrapStatus::NeedsAuth);
    assert_eq!(summary.missing_auth_env, vec!["DEMO_INTEGRATION_TOKEN"]);
  }
}
