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
use rmcp::model::Tool;
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

pub struct McpConnectionManager {
  servers: HashMap<String, ManagedServer>,
  tools: HashMap<String, ManagedTool>,
}

impl McpConnectionManager {
  pub async fn new(config: &McpConfig) -> Result<Self> {
    let mut servers = HashMap::new();
    let mut tools = HashMap::new();

    for (server_name, server_config) in config.servers.iter().filter(|(_, cfg)| cfg.enabled) {
      let client = Arc::new(connect_server(server_name, server_config).await?);
      let tool_timeout = server_config.tool_timeout_sec.map(Duration::from_secs);
      let listed_tools = list_tools(&client, tool_timeout).await?;

      for tool in filter_tools(server_config, listed_tools) {
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

      servers.insert(
        server_name.clone(),
        ManagedServer {
          client,
          tool_timeout,
        },
      );
    }

    Ok(Self { servers, tools })
  }

  pub fn tool_specs(&self) -> Vec<ToolSpec> {
    self.tools.values().map(|tool| tool.spec.clone()).collect()
  }

  pub fn tool_names(&self) -> Vec<String> {
    self.tools.keys().cloned().collect()
  }

  pub fn has_tools(&self) -> bool {
    !self.tools.is_empty()
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
      .call_tool(
        managed_tool.tool.clone(),
        arguments,
        server.tool_timeout,
      )
      .await?;

    Ok(McpToolCallResult {
      content: result
        .content
        .into_iter()
        .map(|item| serde_json::to_value(item).unwrap_or_else(|_| Value::String("<content>".to_string())))
        .collect(),
      structured_content: result.structured_content,
      is_error: result.is_error.unwrap_or(false),
    })
  }
}

impl Default for McpConnectionManager {
  fn default() -> Self {
    Self {
      servers: HashMap::new(),
      tools: HashMap::new(),
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
      let additional_properties = object.get("additionalProperties").and_then(|value| match value {
        Value::Bool(flag) => Some(AdditionalProperties::Boolean(*flag)),
        Value::Object(_) => Some(AdditionalProperties::Schema(Box::new(json_schema_from_value(value)))),
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
