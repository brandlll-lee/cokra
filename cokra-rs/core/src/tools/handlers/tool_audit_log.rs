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
  network_events: Vec<crate::tools::network_approval::NetworkAuditEvent>,
  lsp_events: Vec<crate::lsp::LspAuditEvent>,
}

#[derive(Debug, Serialize)]
struct ToolAuditEntry {
  call_id: String,
  tool_name: String,
  source_kind: Option<String>,
  approval_mode: Option<String>,
  risk_level: Option<String>,
  network_backends: Vec<String>,
  semantic_lsp: bool,
  interactive_exec: bool,
  observed_backend: Option<String>,
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
          let observed_backend = output.as_deref().and_then(parse_observed_backend);
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
            risk_level: spec.map(|spec| {
              let approval = crate::tool_runtime::ToolApproval::from_permissions(
                &spec.permissions,
                spec.permission_key.clone(),
                spec.mutates_state,
              );
              match approval.risk_level {
                crate::tool_runtime::ToolRiskLevel::Low => "low",
                crate::tool_runtime::ToolRiskLevel::Medium => "medium",
                crate::tool_runtime::ToolRiskLevel::High => "high",
              }
              .to_string()
            }),
            network_backends: spec
              .map(|spec| {
                crate::tool_runtime::ToolCapabilityFacets::for_tool_name(
                  &spec.name,
                  spec.permissions.allow_network,
                )
                .network_backends
              })
              .unwrap_or_default(),
            semantic_lsp: spec
              .map(|spec| {
                crate::tool_runtime::ToolCapabilityFacets::for_tool_name(
                  &spec.name,
                  spec.permissions.allow_network,
                )
                .semantic_lsp
              })
              .unwrap_or(false),
            interactive_exec: spec
              .map(|spec| {
                crate::tool_runtime::ToolCapabilityFacets::for_tool_name(
                  &spec.name,
                  spec.permissions.allow_network,
                )
                .interactive_exec
              })
              .unwrap_or(false),
            observed_backend,
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
    let network_events =
      crate::tools::network_approval::recent_network_audit_events(args.limit).await;
    let lsp_events = crate::lsp::recent_audit_events(args.limit).await;
    let content = serde_json::to_string(&ToolAuditLogResponse {
      total_entries,
      entries,
      network_events,
      lsp_events,
    })
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

fn parse_observed_backend(output: &str) -> Option<String> {
  let value: serde_json::Value = serde_json::from_str(output).ok()?;
  value
    .get("backend")
    .and_then(serde_json::Value::as_str)
    .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
  use std::collections::BTreeMap;
  use std::path::Path;
  use std::sync::Arc;
  use std::time::Duration;

  use super::*;
  use crate::session::Session;
  use crate::tools::context::ToolPayload;
  use crate::tools::context::ToolRuntimeContext;
  use crate::tools::registry::ToolRegistry;
  use crate::tools::spec::JsonSchema;
  use crate::tools::spec::ToolHandlerType;
  use crate::tools::spec::ToolPermissions;
  use crate::tools::spec::ToolSpec;
  use crate::turn::response_items::ResponseItem;
  use cokra_protocol::AskForApproval;
  use cokra_protocol::ReviewDecision;

  fn tool_spec(name: &str, permissions: ToolPermissions) -> ToolSpec {
    ToolSpec::new(
      name,
      "test tool",
      JsonSchema::Object {
        properties: BTreeMap::new(),
        required: Some(Vec::new()),
        additional_properties: Some(false.into()),
      },
      None,
      ToolHandlerType::Function,
      permissions,
    )
  }

  fn runtime_context(
    session: Arc<Session>,
    tool_registry: Arc<ToolRegistry>,
    network_attempt_id: Option<String>,
  ) -> Arc<ToolRuntimeContext> {
    Arc::new(ToolRuntimeContext {
      session,
      tool_registry,
      tx_event: None,
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      approval_policy: AskForApproval::OnRequest,
      model_provider_id: Some("openai".to_string()),
      model_runtime_kind: Some("openai_codex".to_string()),
      supports_native_web_search: true,
      has_managed_network_requirements: true,
      allowed_domains: Vec::new(),
      denied_domains: Vec::new(),
      network_attempt_id,
    })
  }

  #[tokio::test]
  async fn includes_observed_backend_and_network_audit_events() {
    let mut registry = ToolRegistry::new();
    registry.register_spec(tool_spec(
      "web_search",
      ToolPermissions {
        requires_approval: true,
        allow_network: true,
        allow_fs_write: false,
      },
    ));
    let tool_registry = Arc::new(registry);
    let session = Arc::new(Session::new());
    session
      .append_response_items(vec![
        ResponseItem::FunctionCall {
          id: "call-1".to_string(),
          name: "web_search".to_string(),
          arguments: serde_json::json!({ "query": "rust lsp" }).to_string(),
        },
        ResponseItem::FunctionCallOutput {
          call_id: "call-1".to_string(),
          output: serde_json::json!({
            "backend": "searxng",
            "results": []
          })
          .to_string(),
          is_error: false,
        },
      ])
      .await;

    let runtime = runtime_context(
      Arc::clone(&session),
      Arc::clone(&tool_registry),
      Some("attempt-1".to_string()),
    );
    let host = format!("audit-log-{}.cokra.invalid", uuid::Uuid::new_v4().simple());
    let approval_id = format!("network#https#{host}#443");
    let url = format!("https://{host}/search");
    let approval_runtime = Arc::clone(&runtime);
    let approval_session = Arc::clone(&session);
    let authorize = tokio::spawn(async move {
      crate::tools::network_approval::authorize_http_url(
        approval_runtime.as_ref(),
        Path::new("."),
        &url,
        &[],
      )
      .await
    });

    let mut notified = false;
    for _ in 0..40 {
      if approval_session
        .notify_exec_approval(&approval_id, ReviewDecision::Always)
        .await
      {
        notified = true;
        break;
      }
      tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(notified);
    assert!(authorize.await.expect("authorize task").is_ok());

    let output = ToolAuditLogHandler
      .handle_async(ToolInvocation {
        id: "audit-1".to_string(),
        name: "tool_audit_log".to_string(),
        payload: ToolPayload::Function {
          arguments: serde_json::json!({ "limit": 5 }).to_string(),
        },
        cwd: std::env::temp_dir(),
        runtime: Some(runtime),
      })
      .await
      .expect("audit succeeds");

    let parsed: serde_json::Value =
      serde_json::from_str(&output.text_content()).expect("valid json");
    assert_eq!(parsed["entries"][0]["observed_backend"], "searxng");
    assert!(
      parsed["network_events"]
        .as_array()
        .is_some_and(|events| events.iter().any(|event| event["host"] == host))
    );
    assert!(parsed["lsp_events"].is_array());
  }
}
