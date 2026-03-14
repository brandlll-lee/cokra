use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::exec::ExecExpiration;
use crate::exec::ExecParams;
use crate::exec::WindowsSandboxLevel;
use crate::exec::execute_command;
use crate::exec::format_exec_output_for_model_structured;
use crate::integrations::loader::LoadedIntegrationManifest;
use crate::integrations::manifest::IntegrationKind;
use crate::integrations::manifest::IntegrationToolExecution;
use crate::integrations::manifest::IntegrationToolManifest;
use crate::integrations::projector::RegisteredIntegrationTool;
use crate::tool_runtime::ApprovalMode;
use crate::tool_runtime::ToolApproval;
use crate::tool_runtime::ToolDefinition;
use crate::tool_runtime::ToolRiskLevel;
use crate::tool_runtime::ToolSource;
use crate::tools::ToolHandler;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolKind;
use crate::tools::spec::AdditionalProperties;
use crate::tools::spec::JsonSchema;
use crate::tools::spec::ToolHandlerType;
use crate::tools::spec::ToolPermissions;
use crate::tools::spec::ToolSourceKind;
use crate::tools::spec::ToolSpec;
use crate::truncate::DEFAULT_TOOL_OUTPUT_TOKENS;
use crate::truncate::TruncationPolicy;

pub fn project_cli_tools(
  manifests: &[&LoadedIntegrationManifest],
) -> anyhow::Result<Vec<RegisteredIntegrationTool>> {
  let mut projected = Vec::new();
  for loaded in manifests {
    if loaded.manifest.kind != IntegrationKind::Cli {
      continue;
    }
    for tool in &loaded.manifest.tools {
      let IntegrationToolExecution::Command {
        command,
        workdir,
        timeout_ms,
        env,
      } = &tool.execution
      else {
        continue;
      };
      projected.push(RegisteredIntegrationTool {
        spec: spec_from_manifest(tool, ToolSourceKind::Cli),
        definition: definition_from_manifest(
          &loaded.manifest.name,
          tool,
          ToolSource::Cli,
          ToolSourceKind::Cli,
        ),
        handler: Arc::new(ManifestCliHandler {
          command: command.clone(),
          workdir: workdir.clone(),
          timeout_ms: *timeout_ms,
          env: env.clone(),
        }),
      });
    }
  }
  Ok(projected)
}

struct ManifestCliHandler {
  command: Vec<String>,
  workdir: Option<String>,
  timeout_ms: Option<u64>,
  env: HashMap<String, String>,
}

#[async_trait]
impl ToolHandler for ManifestCliHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    true
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> std::result::Result<ToolOutput, FunctionCallError> {
    let input = invocation.parse_arguments_value()?;
    let command = self
      .command
      .iter()
      .map(|part| render_string_template(part, &input, invocation.cwd.as_path()))
      .collect::<Result<Vec<_>, _>>()?;
    if command.is_empty() {
      return Err(FunctionCallError::Execution(
        "CLI integration command must not be empty".to_string(),
      ));
    }
    let cwd = resolve_workdir(invocation.cwd.as_path(), self.workdir.as_deref(), &input)?;
    let env = self
      .env
      .iter()
      .map(|(key, value)| {
        render_string_template(value, &input, invocation.cwd.as_path()).map(|rendered| {
          (key.clone(), rendered)
        })
      })
      .collect::<Result<HashMap<_, _>, _>>()?;
    let output = execute_command(&ExecParams {
      command,
      cwd,
      expiration: self
        .timeout_ms
        .map(|timeout| ExecExpiration::Timeout(std::time::Duration::from_millis(timeout)))
        .unwrap_or(ExecExpiration::DefaultTimeout),
      env,
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

    let content = format_exec_output_for_model_structured(
      &output,
      TruncationPolicy::Tokens(DEFAULT_TOOL_OUTPUT_TOKENS),
    );
    Ok(
      ToolOutput::success(content)
        .with_id(invocation.id)
        .with_success(output.exit_code == 0),
    )
  }
}

pub(crate) fn spec_from_manifest(
  tool: &IntegrationToolManifest,
  source_kind: ToolSourceKind,
) -> ToolSpec {
  ToolSpec::new(
    tool.id.clone(),
    tool.description.clone(),
    json_schema_from_value(&tool.input_schema),
    tool.output_schema.as_ref().map(json_schema_from_value),
    ToolHandlerType::Function,
    ToolPermissions {
      requires_approval: matches!(
        tool.approval_mode.unwrap_or_default(),
        ApprovalMode::Manual
      ),
      allow_network: tool.allow_network,
      allow_fs_write: tool.allow_fs_write || tool.mutates_state.unwrap_or(false),
    },
  )
  .with_source_kind(source_kind)
  .with_permission_key(
    tool
      .permission_key
      .clone()
      .unwrap_or_else(|| tool.id.clone()),
  )
  .with_supports_parallel(tool.supports_parallel.unwrap_or(true))
  .with_mutates_state(tool.mutates_state.unwrap_or(tool.allow_fs_write))
}

pub(crate) fn definition_from_manifest(
  provider_id: &str,
  tool: &IntegrationToolManifest,
  source: ToolSource,
  source_kind: ToolSourceKind,
) -> ToolDefinition {
  ToolDefinition {
    id: tool.id.clone(),
    name: tool.name.clone().unwrap_or_else(|| tool.id.clone()),
    description: tool.description.clone(),
    input_schema: tool.input_schema.clone(),
    output_schema: tool.output_schema.clone(),
    source,
    aliases: tool.aliases.clone(),
    tags: {
      let mut tags = vec![provider_id.to_string()];
      tags.extend(tool.tags.clone());
      tags.push(match source {
        ToolSource::Builtin => "builtin",
        ToolSource::Mcp => "mcp",
        ToolSource::Cli => "cli",
        ToolSource::Api => "api",
      }
      .to_string());
      tags.sort();
      tags.dedup();
      tags
    },
    approval: ToolApproval {
      risk_level: tool.risk_level.unwrap_or_else(|| {
        if tool.allow_fs_write || tool.mutates_state.unwrap_or(false) {
          ToolRiskLevel::High
        } else if tool.allow_network {
          ToolRiskLevel::Medium
        } else {
          ToolRiskLevel::Low
        }
      }),
      approval_mode: tool.approval_mode.unwrap_or_else(|| {
        if tool.allow_fs_write || tool.mutates_state.unwrap_or(false) {
          ApprovalMode::Manual
        } else {
          ApprovalMode::Auto
        }
      }),
      permission_key: tool.permission_key.clone().or_else(|| Some(tool.id.clone())),
      allow_network: tool.allow_network,
      allow_fs_write: tool.allow_fs_write || tool.mutates_state.unwrap_or(false),
    },
    enabled: tool.enabled,
    supports_parallel: tool.supports_parallel.unwrap_or(true),
    mutates_state: tool.mutates_state.unwrap_or(tool.allow_fs_write),
    input_keys: input_keys_from_value(&tool.input_schema),
    provider_id: Some(provider_id.to_string()),
    source_kind: Some(match source_kind {
      ToolSourceKind::BuiltinPrimitive => "builtin_primitive",
      ToolSourceKind::BuiltinCollaboration => "builtin_collaboration",
      ToolSourceKind::BuiltinWorkflow => "builtin_workflow",
      ToolSourceKind::Cli => "cli",
      ToolSourceKind::Api => "api",
      ToolSourceKind::Mcp => "mcp",
    }
    .to_string()),
    server_name: None,
    remote_name: None,
  }
}

pub(crate) fn render_string_template(
  template: &str,
  input: &serde_json::Value,
  workspace_root: &Path,
) -> std::result::Result<String, FunctionCallError> {
  let mut rendered = template.to_string();
  for (key, value) in template_values(input, workspace_root) {
    rendered = rendered.replace(&format!("{{{{{key}}}}}"), &value);
  }
  while let Some(start) = rendered.find("${") {
    let Some(end_offset) = rendered[start + 2..].find('}') else {
      break;
    };
    let end = start + 2 + end_offset;
    let env_key = &rendered[start + 2..end];
    let env_value = std::env::var(env_key).unwrap_or_default();
    rendered.replace_range(start..=end, &env_value);
  }
  Ok(rendered)
}

pub(crate) fn render_json_template(
  value: &serde_json::Value,
  input: &serde_json::Value,
  workspace_root: &Path,
) -> std::result::Result<serde_json::Value, FunctionCallError> {
  match value {
    serde_json::Value::String(text) => Ok(serde_json::Value::String(render_string_template(
      text,
      input,
      workspace_root,
    )?)),
    serde_json::Value::Array(items) => Ok(serde_json::Value::Array(
      items
        .iter()
        .map(|item| render_json_template(item, input, workspace_root))
        .collect::<Result<Vec<_>, _>>()?,
    )),
    serde_json::Value::Object(map) => {
      let mut rendered = serde_json::Map::new();
      for (key, value) in map {
        rendered.insert(
          key.clone(),
          render_json_template(value, input, workspace_root)?,
        );
      }
      Ok(serde_json::Value::Object(rendered))
    }
    other => Ok(other.clone()),
  }
}

pub(crate) fn resolve_workdir(
  invocation_cwd: &Path,
  configured_workdir: Option<&str>,
  input: &serde_json::Value,
) -> std::result::Result<PathBuf, FunctionCallError> {
  let Some(configured_workdir) = configured_workdir else {
    return Ok(invocation_cwd.to_path_buf());
  };
  let rendered = render_string_template(configured_workdir, input, invocation_cwd)?;
  let path = PathBuf::from(rendered);
  if path.is_absolute() {
    Ok(path)
  } else {
    Ok(invocation_cwd.join(path))
  }
}

fn template_values(input: &serde_json::Value, workspace_root: &Path) -> HashMap<String, String> {
  let mut values = HashMap::from([(
    "workspace".to_string(),
    workspace_root.display().to_string(),
  )]);
  if let Some(object) = input.as_object() {
    for (key, value) in object {
      values.insert(
        key.clone(),
        match value {
          serde_json::Value::String(text) => text.clone(),
          _ => value.to_string(),
        },
      );
    }
  }
  values
}

fn input_keys_from_value(value: &serde_json::Value) -> Vec<String> {
  value
    .get("properties")
    .and_then(serde_json::Value::as_object)
    .map(|properties| properties.keys().cloned().collect())
    .unwrap_or_default()
}

fn json_schema_from_value(value: &serde_json::Value) -> JsonSchema {
  let Some(object) = value.as_object() else {
    return JsonSchema::Object {
      properties: BTreeMap::new(),
      required: Some(Vec::new()),
      additional_properties: Some(false.into()),
    };
  };

  match object.get("type").and_then(serde_json::Value::as_str) {
    Some("string") => JsonSchema::String {
      description: object
        .get("description")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string),
    },
    Some("number") | Some("integer") => JsonSchema::Number {
      description: object
        .get("description")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string),
    },
    Some("boolean") => JsonSchema::Boolean {
      description: object
        .get("description")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string),
    },
    Some("array") => JsonSchema::Array {
      items: Box::new(
        object
          .get("items")
          .map(json_schema_from_value)
          .unwrap_or(JsonSchema::Object {
            properties: BTreeMap::new(),
            required: Some(Vec::new()),
            additional_properties: Some(false.into()),
          }),
      ),
      description: object
        .get("description")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string),
    },
    _ => JsonSchema::Object {
      properties: object
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|properties| {
          properties
            .iter()
            .map(|(key, value)| (key.clone(), json_schema_from_value(value)))
            .collect()
        })
        .unwrap_or_default(),
      required: object
        .get("required")
        .and_then(serde_json::Value::as_array)
        .map(|required| {
          required
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(ToString::to_string)
            .collect()
        }),
      additional_properties: Some(
        object
          .get("additionalProperties")
          .and_then(|value| match value {
            serde_json::Value::Bool(flag) => Some(AdditionalProperties::Boolean(*flag)),
            _ => None,
          })
          .unwrap_or_else(|| AdditionalProperties::Boolean(false)),
      ),
    },
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn render_string_template_replaces_workspace_and_input_keys() {
    let rendered = render_string_template(
      "run {{query}} in {{workspace}}",
      &serde_json::json!({"query": "tests"}),
      Path::new("/repo"),
    )
    .expect("rendered");
    assert_eq!(rendered, "run tests in /repo");
  }
}
