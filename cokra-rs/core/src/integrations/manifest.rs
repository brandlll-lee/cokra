use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::tool_runtime::ApprovalMode;
use crate::tool_runtime::ToolRiskLevel;

fn default_enabled() -> bool {
  true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationKind {
  Mcp,
  Cli,
  Api,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationManifest {
  pub name: String,
  pub kind: IntegrationKind,
  #[serde(default = "default_enabled")]
  pub enabled: bool,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub install: Option<IntegrationInstall>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub auth: Option<IntegrationAuth>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub healthcheck: Option<IntegrationHealthcheck>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub discovery: Option<IntegrationDiscovery>,
  #[serde(default)]
  pub tools: Vec<IntegrationToolManifest>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub command: Option<String>,
  #[serde(default)]
  pub args: Vec<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub env: Option<HashMap<String, String>>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub cwd: Option<PathBuf>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub url: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub bearer_token: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub headers: Option<HashMap<String, String>>,
  #[serde(default)]
  pub required: bool,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub startup_timeout_sec: Option<u64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub tool_timeout_sec: Option<u64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub enabled_tools: Option<Vec<String>>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub disabled_tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationInstall {
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub check: Option<Vec<String>>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub run: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationAuth {
  #[serde(default)]
  pub env: Vec<String>,
  #[serde(default)]
  pub optional: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IntegrationHealthcheck {
  Command { run: Vec<String> },
  Http {
    method: Option<String>,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
  },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationDiscovery {
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub r#type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationToolManifest {
  pub id: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub name: Option<String>,
  pub description: String,
  pub input_schema: Value,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub output_schema: Option<Value>,
  #[serde(default)]
  pub aliases: Vec<String>,
  #[serde(default)]
  pub tags: Vec<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub permission_key: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub approval_mode: Option<ApprovalMode>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub risk_level: Option<ToolRiskLevel>,
  #[serde(default)]
  pub allow_network: bool,
  #[serde(default)]
  pub allow_fs_write: bool,
  #[serde(default = "default_enabled")]
  pub enabled: bool,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub supports_parallel: Option<bool>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub mutates_state: Option<bool>,
  #[serde(flatten)]
  pub execution: IntegrationToolExecution,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IntegrationToolExecution {
  Command {
    command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workdir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>,
    #[serde(default)]
    env: HashMap<String, String>,
  },
  Http {
    method: String,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    query: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>,
  },
}
