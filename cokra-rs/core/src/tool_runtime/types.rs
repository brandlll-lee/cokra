use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use super::ToolApproval;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolSource {
  #[default]
  Builtin,
  Mcp,
  Cli,
  Api,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
  pub id: String,
  pub name: String,
  pub description: String,
  pub input_schema: Value,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub output_schema: Option<Value>,
  pub source: ToolSource,
  #[serde(default)]
  pub aliases: Vec<String>,
  #[serde(default)]
  pub tags: Vec<String>,
  pub approval: ToolApproval,
  #[serde(default)]
  pub enabled: bool,
  #[serde(default)]
  pub supports_parallel: bool,
  #[serde(default)]
  pub mutates_state: bool,
  #[serde(default)]
  pub input_keys: Vec<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub provider_id: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub source_kind: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub server_name: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub remote_name: Option<String>,
}

impl ToolDefinition {
  pub fn matches_name(&self, name: &str) -> bool {
    self.id == name || self.name == name || self.aliases.iter().any(|alias| alias == name)
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
  pub tool_id: String,
  pub input: Value,
  pub call_id: String,
  pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolResultMetadata {
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub duration_ms: Option<u64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub exit_code: Option<i32>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub command: Option<Vec<String>>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub http_status: Option<u16>,
  pub source: ToolSource,
  #[serde(default)]
  pub artifacts: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
  pub ok: bool,
  pub content: Value,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub error: Option<String>,
  pub metadata: ToolResultMetadata,
}
