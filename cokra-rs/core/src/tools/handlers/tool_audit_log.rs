use std::collections::HashMap;

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use crate::turn::response_items::ResponseItem;

pub struct ToolAuditLogHandler;

#[derive(Debug, Deserialize)]
struct ToolAuditLogArgs {
  #[serde(default = "default_limit")]
  limit: usize,
  tool_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct ToolAuditLogResponse {
  total_entries: usize,
  entries: Vec<ToolAuditEntry>,
}

#[derive(Debug, Serialize)]
struct ToolAuditEntry {
  call_id: String,
  tool_name: String,
  source_kind: Option<String>,
  approval_mode: Option<String>,
  arguments: String,
  output: Option<String>,
  status: String,
}

fn default_limit() -> usize {
  20
}

#[async_trait]
impl ToolHandler for ToolAuditLogHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: ToolAuditLogArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("tool_audit_log missing runtime context".to_string())
    })?;
    let history = runtime.session.clone_response_history().await;
    let mut outputs = HashMap::new();
    for item in &history {
      if let ResponseItem::FunctionCallOutput {
        call_id,
        output,
        is_error,
      } = item
      {
        outputs.insert(call_id.clone(), (output.clone(), *is_error));
      }
    }
    let mut entries = Vec::new();

    for item in history {
      match item {
        ResponseItem::FunctionCallOutput { .. } => {}
        ResponseItem::FunctionCall {
          id,
          name,
          arguments,
        } => {
          if let Some(filter) = &args.tool_name
            && filter.trim() != name
          {
            continue;
          }
          let spec = runtime.tool_registry.get_spec(&name);
          let (output, status) = outputs
            .remove(&id)
            .map(|(output, is_error)| {
              (
                Some(output),
                if is_error { "failed" } else { "succeeded" }.to_string(),
              )
            })
            .unwrap_or((None, "pending".to_string()));
          entries.push(ToolAuditEntry {
            call_id: id,
            tool_name: name.clone(),
            source_kind: spec.map(|spec| source_kind_label(spec.source_kind).to_string()),
            approval_mode: spec.map(|spec| {
              if spec.permissions.requires_approval {
                "manual".to_string()
              } else {
                "auto".to_string()
              }
            }),
            arguments,
            output,
            status,
          });
        }
        ResponseItem::Message { .. } => {}
      }
    }

    entries.reverse();
    let total_entries = entries.len();
    entries.truncate(args.limit.max(1));
    let content = serde_json::to_string(&ToolAuditLogResponse { total_entries, entries })
      .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize tool_audit_log: {err}"))
      })?;
    Ok(ToolOutput::success(content).with_id(invocation.id))
  }
}

fn source_kind_label(source_kind: crate::tools::spec::ToolSourceKind) -> &'static str {
  match source_kind {
    crate::tools::spec::ToolSourceKind::BuiltinPrimitive => "builtin_primitive",
    crate::tools::spec::ToolSourceKind::BuiltinCollaboration => "builtin_collaboration",
    crate::tools::spec::ToolSourceKind::BuiltinWorkflow => "builtin_workflow",
    crate::tools::spec::ToolSourceKind::Cli => "cli",
    crate::tools::spec::ToolSourceKind::Api => "api",
    crate::tools::spec::ToolSourceKind::Mcp => "mcp",
  }
}
