//! Minimal MCP bridge for the kernel.
//!
//! MCP stays a dynamic tool source: connect configured servers, mirror their
//! tool/resource surface, and expose that surface through the tool kernel.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use rmcp::model::ClientCapabilities;
use rmcp::model::Implementation;
use rmcp::model::InitializeRequestParams;
use rmcp::model::PaginatedRequestParams;
use rmcp::model::ProtocolVersion;
use rmcp::model::ReadResourceResult;
use rmcp::model::Resource;
use rmcp::model::ResourceTemplate;
use rmcp::model::Tool;
use serde::Serialize;
use serde_json::Value;

use cokra_config::McpConfig;
use cokra_config::McpServerConfig;
use cokra_config::McpServerTransportConfig;

use crate::tools::context::McpToolCallResult;
use crate::tools::spec::AdditionalProperties;
use crate::tools::spec::JsonSchema;
use crate::tools::spec::ToolHandlerType;
use crate::tools::spec::ToolPermissions;
use crate::tools::spec::ToolSpec;

#[derive(Clone)]
struct ManagedServer {
  client: Arc<cokra_rmcp_client::RmcpClient>,
  tool_timeout: Option<Duration>,
}

#[derive(Clone)]
struct ManagedTool {
  server: String,
  tool: String,
  spec: ToolSpec,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpToolDescriptor {
  pub exposed_name: String,
  pub server_name: String,
  pub remote_tool_name: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpResourceDescriptor {
  pub server_name: String,
  pub uri: String,
  pub name: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub title: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub description: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub mime_type: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub size: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpResourceTemplateDescriptor {
  pub server_name: String,
  pub uri_template: String,
  pub name: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub title: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub description: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub mime_type: Option<String>,
}

pub struct McpConnectionManager {
  servers: HashMap<String, ManagedServer>,
  tools: HashMap<String, ManagedTool>,
  resources: HashMap<String, Vec<Resource>>,
  resource_templates: HashMap<String, Vec<ResourceTemplate>>,
}

impl McpConnectionManager {
  pub async fn new(config: &McpConfig) -> Result<Self> {
    let mut servers = HashMap::new();
    let mut tools = HashMap::new();
    let mut resources = HashMap::new();
    let mut resource_templates = HashMap::new();

    for (server_name, server_config) in config.servers.iter().filter(|(_, cfg)| cfg.enabled) {
      let connect_result = connect_server(server_name, server_config)
        .await
        .and_then(|client| {
          let tool_timeout = server_config.tool_timeout_sec.map(Duration::from_secs);
          // Wrap in a future-friendly way — we call list_tools below once client is ready
          Ok((Arc::new(client), tool_timeout))
        });

      let (client, tool_timeout) = match connect_result {
        Ok(v) => v,
        Err(err) => {
          if server_config.required {
            return Err(err.context(format!(
              "required MCP server `{server_name}` failed to connect"
            )));
          }
          tracing::warn!(
            "MCP server `{server_name}` failed to connect (non-required, skipping): {err:#}"
          );
          continue;
        }
      };

      let listed_tools = list_tools(&client, tool_timeout).await;
      let listed_resources = list_resources(&client, tool_timeout).await;
      let listed_resource_templates = list_resource_templates(&client, tool_timeout).await;

      if listed_tools.is_err() && listed_resources.is_err() && listed_resource_templates.is_err() {
        let tool_err = listed_tools
          .as_ref()
          .err()
          .map(|err| err.to_string())
          .unwrap_or_else(|| "n/a".to_string());
        let resource_err = listed_resources
          .as_ref()
          .err()
          .map(|err| err.to_string())
          .unwrap_or_else(|| "n/a".to_string());
        let template_err = listed_resource_templates
          .as_ref()
          .err()
          .map(|err| err.to_string())
          .unwrap_or_else(|| "n/a".to_string());
        let message = format!(
          "MCP server `{server_name}` failed capability discovery (tools: {tool_err}; resources: {resource_err}; templates: {template_err})"
        );
        if server_config.required {
          return Err(anyhow!(message));
        }
        tracing::warn!("{message}");
        continue;
      }

      if let Err(err) = &listed_tools {
        tracing::warn!(
          "MCP server `{server_name}` failed to list tools (continuing without tools): {err:#}"
        );
      }
      if let Err(err) = &listed_resources {
        tracing::warn!(
          "MCP server `{server_name}` failed to list resources (continuing without resources): {err:#}"
        );
      }
      if let Err(err) = &listed_resource_templates {
        tracing::warn!(
          "MCP server `{server_name}` failed to list resource templates (continuing without templates): {err:#}"
        );
      }

      for tool in filter_tools(server_config, listed_tools.unwrap_or_default()) {
        let exposed_name = qualify_tool_name(server_name, tool.name.as_ref(), &tools);
        tools.insert(
          exposed_name.clone(),
          ManagedTool {
            server: server_name.clone(),
            tool: tool.name.to_string(),
            spec: ToolSpec::new(
              exposed_name,
              tool
                .description
                .as_deref()
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("MCP tool {}", tool.name)),
              json_schema_from_mcp_tool(&tool),
              None,
              ToolHandlerType::Mcp,
              ToolPermissions::default(),
            ),
          },
        );
      }

      if let Ok(server_resources) = listed_resources {
        resources.insert(server_name.clone(), server_resources);
      }
      if let Ok(server_templates) = listed_resource_templates {
        resource_templates.insert(server_name.clone(), server_templates);
      }

      servers.insert(
        server_name.clone(),
        ManagedServer {
          client,
          tool_timeout,
        },
      );
    }

    Ok(Self {
      servers,
      tools,
      resources,
      resource_templates,
    })
  }

  pub fn tool_specs(&self) -> Vec<ToolSpec> {
    self.tools.values().map(|tool| tool.spec.clone()).collect()
  }

  pub fn tool_descriptors(&self) -> Vec<McpToolDescriptor> {
    let mut descriptors = self
      .tools
      .iter()
      .map(|(exposed_name, tool)| McpToolDescriptor {
        exposed_name: exposed_name.clone(),
        server_name: tool.server.clone(),
        remote_tool_name: tool.tool.clone(),
        description: (!tool.spec.description.trim().is_empty()).then(|| tool.spec.description.clone()),
      })
      .collect::<Vec<_>>();
    descriptors.sort_by(|left, right| left.exposed_name.cmp(&right.exposed_name));
    descriptors
  }

  pub fn tool_names(&self) -> Vec<String> {
    self.tools.keys().cloned().collect()
  }

  pub fn server_names(&self) -> Vec<String> {
    let mut names = self.servers.keys().cloned().collect::<Vec<_>>();
    names.sort();
    names
  }

  pub fn has_tools(&self) -> bool {
    !self.tools.is_empty()
  }

  pub fn resource_descriptors(&self) -> Vec<McpResourceDescriptor> {
    let mut descriptors = self
      .resources
      .iter()
      .flat_map(|(server_name, resources)| {
        resources
          .iter()
          .map(move |resource| McpResourceDescriptor {
            server_name: server_name.clone(),
            uri: resource.uri.clone(),
            name: resource.name.clone(),
            title: resource.title.clone(),
            description: resource.description.clone(),
            mime_type: resource.mime_type.clone(),
            size: resource.size,
          })
      })
      .collect::<Vec<_>>();
    descriptors.sort_by(|left, right| {
      left
        .server_name
        .cmp(&right.server_name)
        .then_with(|| left.uri.cmp(&right.uri))
    });
    descriptors
  }

  pub fn resource_template_descriptors(&self) -> Vec<McpResourceTemplateDescriptor> {
    let mut descriptors = self
      .resource_templates
      .iter()
      .flat_map(|(server_name, templates)| {
        templates
          .iter()
          .map(move |template| McpResourceTemplateDescriptor {
            server_name: server_name.clone(),
            uri_template: template.uri_template.clone(),
            name: template.name.clone(),
            title: template.title.clone(),
            description: template.description.clone(),
            mime_type: template.mime_type.clone(),
          })
      })
      .collect::<Vec<_>>();
    descriptors.sort_by(|left, right| {
      left
        .server_name
        .cmp(&right.server_name)
        .then_with(|| left.uri_template.cmp(&right.uri_template))
    });
    descriptors
  }

  pub fn resolve_tool_name(&self, exposed_name: &str) -> Option<(&str, &str)> {
    self
      .tools
      .get(exposed_name)
      .map(|tool| (tool.server.as_str(), tool.tool.as_str()))
  }

  pub async fn call_tool(
    &self,
    exposed_name: &str,
    arguments: Option<Value>,
  ) -> Result<McpToolCallResult> {
    let managed_tool = self
      .tools
      .get(exposed_name)
      .ok_or_else(|| anyhow!("unknown MCP tool `{exposed_name}`"))?;
    let server = self
      .servers
      .get(&managed_tool.server)
      .ok_or_else(|| anyhow!("unknown MCP server `{}`", managed_tool.server))?;
    let result = server
      .client
      .call_tool(managed_tool.tool.clone(), arguments, server.tool_timeout)
      .await?;

    Ok(McpToolCallResult {
      content: result
        .content
        .into_iter()
        .map(|item| {
          serde_json::to_value(item).unwrap_or_else(|_| Value::String("<content>".to_string()))
        })
        .collect(),
      structured_content: result.structured_content,
      is_error: result.is_error.unwrap_or(false),
    })
  }

  pub async fn read_resource(&self, server_name: &str, uri: &str) -> Result<ReadResourceResult> {
    let server = self
      .servers
      .get(server_name)
      .ok_or_else(|| anyhow!("unknown MCP server `{server_name}`"))?;
    server
      .client
      .read_resource(uri.to_string(), server.tool_timeout)
      .await
  }
}

impl Default for McpConnectionManager {
  fn default() -> Self {
    Self {
      servers: HashMap::new(),
      tools: HashMap::new(),
      resources: HashMap::new(),
      resource_templates: HashMap::new(),
    }
  }
}

async fn connect_server(
  server_name: &str,
  config: &McpServerConfig,
) -> Result<cokra_rmcp_client::RmcpClient> {
  let client = match &config.transport {
    McpServerTransportConfig::Stdio {
      command,
      args,
      env,
      cwd,
    } => {
      cokra_rmcp_client::RmcpClient::new_stdio_client(
        OsString::from(command),
        args.iter().map(OsString::from).collect(),
        env.clone(),
        cwd.clone(),
      )
      .await?
    }
    McpServerTransportConfig::Http {
      url,
      bearer_token,
      headers,
    } => {
      cokra_rmcp_client::RmcpClient::new_streamable_http_client(
        url,
        bearer_token.clone(),
        headers.clone(),
      )
      .await?
    }
  };

  client
    .initialize(
      InitializeRequestParams {
        meta: None,
        capabilities: ClientCapabilities {
          experimental: None,
          extensions: None,
          roots: None,
          sampling: None,
          elicitation: None,
          tasks: None,
        },
        client_info: Implementation {
          name: "cokra-mcp-client".to_string(),
          version: env!("CARGO_PKG_VERSION").to_string(),
          title: Some("Cokra".into()),
          description: None,
          icons: None,
          website_url: None,
        },
        protocol_version: ProtocolVersion::V_2025_06_18,
      },
      config.startup_timeout_sec.map(Duration::from_secs),
    )
    .await
    .map_err(|err| anyhow!("failed to initialize MCP server `{server_name}`: {err:#}"))?;

  Ok(client)
}

async fn list_tools(
  client: &cokra_rmcp_client::RmcpClient,
  timeout: Option<Duration>,
) -> Result<Vec<Tool>> {
  let mut tools = Vec::new();
  let mut cursor: Option<String> = None;

  loop {
    let result = client
      .list_tools(
        cursor.as_ref().map(|next| PaginatedRequestParams {
          meta: None,
          cursor: Some(next.clone()),
        }),
        timeout,
      )
      .await?;
    tools.extend(result.tools);

    match result.next_cursor {
      Some(next) => cursor = Some(next),
      None => return Ok(tools),
    }
  }
}

async fn list_resources(
  client: &cokra_rmcp_client::RmcpClient,
  timeout: Option<Duration>,
) -> Result<Vec<Resource>> {
  let mut resources = Vec::new();
  let mut cursor: Option<String> = None;

  loop {
    let result = client
      .list_resources(
        cursor.as_ref().map(|next| PaginatedRequestParams {
          meta: None,
          cursor: Some(next.clone()),
        }),
        timeout,
      )
      .await?;
    resources.extend(result.resources);

    match result.next_cursor {
      Some(next) => cursor = Some(next),
      None => return Ok(resources),
    }
  }
}

async fn list_resource_templates(
  client: &cokra_rmcp_client::RmcpClient,
  timeout: Option<Duration>,
) -> Result<Vec<ResourceTemplate>> {
  let mut resource_templates = Vec::new();
  let mut cursor: Option<String> = None;

  loop {
    let result = client
      .list_resource_templates(
        cursor.as_ref().map(|next| PaginatedRequestParams {
          meta: None,
          cursor: Some(next.clone()),
        }),
        timeout,
      )
      .await?;
    resource_templates.extend(result.resource_templates);

    match result.next_cursor {
      Some(next) => cursor = Some(next),
      None => return Ok(resource_templates),
    }
  }
}

fn filter_tools(server_config: &McpServerConfig, tools: Vec<Tool>) -> Vec<Tool> {
  tools
    .into_iter()
    .filter(|tool| {
      if let Some(enabled) = &server_config.enabled_tools
        && !enabled.iter().any(|name| name == tool.name.as_ref())
      {
        return false;
      }
      if let Some(disabled) = &server_config.disabled_tools
        && disabled.iter().any(|name| name == tool.name.as_ref())
      {
        return false;
      }
      true
    })
    .collect()
}

fn qualify_tool_name(
  server_name: &str,
  tool_name: &str,
  existing: &HashMap<String, ManagedTool>,
) -> String {
  let base = sanitize_tool_name(&format!("mcp__{server_name}__{tool_name}"));
  if !existing.contains_key(&base) {
    return base;
  }

  let mut index = 2usize;
  loop {
    let candidate = format!("{base}_{index}");
    if !existing.contains_key(&candidate) {
      return candidate;
    }
    index += 1;
  }
}

fn sanitize_tool_name(name: &str) -> String {
  let sanitized = name
    .chars()
    .map(|ch| {
      if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
        ch
      } else {
        '_'
      }
    })
    .collect::<String>();
  if sanitized.is_empty() {
    "_".to_string()
  } else {
    sanitized
  }
}

fn json_schema_from_mcp_tool(tool: &Tool) -> JsonSchema {
  serde_json::to_value(tool.input_schema.as_ref())
    .ok()
    .map(|value| json_schema_from_value(&value))
    .unwrap_or_else(empty_object_schema)
}

fn json_schema_from_value(value: &Value) -> JsonSchema {
  let Some(object) = value.as_object() else {
    return empty_object_schema();
  };
  match object.get("type").and_then(Value::as_str) {
    Some("string") => JsonSchema::String {
      description: object
        .get("description")
        .and_then(Value::as_str)
        .map(ToString::to_string),
    },
    Some("number") | Some("integer") => JsonSchema::Number {
      description: object
        .get("description")
        .and_then(Value::as_str)
        .map(ToString::to_string),
    },
    Some("boolean") => JsonSchema::Boolean {
      description: object
        .get("description")
        .and_then(Value::as_str)
        .map(ToString::to_string),
    },
    Some("array") => JsonSchema::Array {
      items: Box::new(
        object
          .get("items")
          .map(json_schema_from_value)
          .unwrap_or_else(empty_object_schema),
      ),
      description: object
        .get("description")
        .and_then(Value::as_str)
        .map(ToString::to_string),
    },
    _ => {
      let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
          properties
            .iter()
            .map(|(key, value)| (key.clone(), json_schema_from_value(value)))
            .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
      let required = object
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
          items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
        });
      let additional_properties =
        object
          .get("additionalProperties")
          .and_then(|value| match value {
            Value::Bool(flag) => Some(AdditionalProperties::Boolean(*flag)),
            Value::Object(_) => Some(AdditionalProperties::Schema(Box::new(
              json_schema_from_value(value),
            ))),
            _ => None,
          });
      JsonSchema::Object {
        properties,
        required,
        additional_properties,
      }
    }
  }
}

fn empty_object_schema() -> JsonSchema {
  JsonSchema::Object {
    properties: BTreeMap::new(),
    required: Some(Vec::new()),
    additional_properties: Some(false.into()),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn empty_tools() -> HashMap<String, ManagedTool> {
    HashMap::new()
  }

  fn make_tool(exposed: &str, server: &str, tool: &str) -> (String, ManagedTool) {
    let spec = ToolSpec::new(
      exposed,
      "test",
      JsonSchema::Object {
        properties: BTreeMap::new(),
        required: Some(vec![]),
        additional_properties: None,
      },
      None,
      ToolHandlerType::Mcp,
      ToolPermissions::default(),
    );
    (
      exposed.to_string(),
      ManagedTool {
        server: server.to_string(),
        tool: tool.to_string(),
        spec,
      },
    )
  }

  #[test]
  fn qualify_tool_name_no_conflict() {
    let tools = empty_tools();
    let name = qualify_tool_name("github", "search_repos", &tools);
    assert_eq!(name, "mcp__github__search_repos");
  }

  #[test]
  fn qualify_tool_name_sanitizes_special_chars() {
    let tools = empty_tools();
    let name = qualify_tool_name("my-server.v2", "get/resource", &tools);
    // non-alphanumeric/underscore/dash chars → underscore
    assert!(!name.contains('.'));
    assert!(!name.contains('/'));
    assert!(name.starts_with("mcp__"));
  }

  #[test]
  fn qualify_tool_name_suffix_on_conflict() {
    let mut tools = empty_tools();
    let (k, v) = make_tool("mcp__srv__tool", "srv", "tool");
    tools.insert(k, v);

    let name = qualify_tool_name("srv", "tool", &tools);
    assert_eq!(name, "mcp__srv__tool_2");
  }

  #[test]
  fn qualify_tool_name_suffix_increments() {
    let mut tools = empty_tools();
    for suffix in ["mcp__s__t", "mcp__s__t_2", "mcp__s__t_3"] {
      let (k, v) = make_tool(suffix, "s", "t");
      tools.insert(k, v);
    }
    let name = qualify_tool_name("s", "t", &tools);
    assert_eq!(name, "mcp__s__t_4");
  }

  #[test]
  fn qualify_tool_name_never_conflicts_with_builtin_names() {
    // Built-in tool names never start with "mcp__" so they
    // cannot clash with any MCP-qualified name.
    let tools = empty_tools();
    for builtin in [
      "shell",
      "edit_file",
      "write_file",
      "web_search",
      "diagnostics",
      "save_memory",
    ] {
      let qualified = qualify_tool_name("server", builtin, &tools);
      assert!(
        qualified.starts_with("mcp__server__"),
        "expected mcp__ prefix, got {qualified}"
      );
      assert_ne!(qualified.as_str(), builtin);
    }
  }

  #[test]
  fn sanitize_tool_name_alphanumeric_passthrough() {
    assert_eq!(sanitize_tool_name("hello_world-123"), "hello_world-123");
  }

  #[test]
  fn sanitize_tool_name_replaces_dots_and_slashes() {
    let s = sanitize_tool_name("a.b/c");
    assert_eq!(s, "a_b_c");
  }

  #[test]
  fn sanitize_tool_name_empty_returns_underscore() {
    assert_eq!(sanitize_tool_name(""), "_");
  }

  #[test]
  fn sanitize_tool_name_spaces_replaced() {
    assert_eq!(sanitize_tool_name("my tool name"), "my_tool_name");
  }
}
