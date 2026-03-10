use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;

use cokra_protocol::EventMsg;
use cokra_protocol::McpInvocation;
use cokra_protocol::McpToolCallBeginEvent;
use cokra_protocol::McpToolCallEndEvent;
use cokra_protocol::McpToolCallResult as ProtocolMcpToolCallResult;

use crate::mcp::McpConnectionManager;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct McpHandler {
  manager: Arc<McpConnectionManager>,
}

impl McpHandler {
  pub fn new(manager: Arc<McpConnectionManager>) -> Self {
    Self { manager }
  }
}

#[async_trait]
impl ToolHandler for McpHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Mcp
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let Some((server, tool)) = self.manager.resolve_tool_name(&invocation.name) else {
      return Err(FunctionCallError::ToolNotFound(format!(
        "unknown MCP tool `{}`",
        invocation.name
      )));
    };

    let arguments = invocation.parse_arguments_value().ok();
    let invocation_event = McpInvocation {
      server: server.to_string(),
      tool: tool.to_string(),
      arguments: arguments.clone(),
    };

    if let Some(runtime) = &invocation.runtime {
      let begin = EventMsg::McpToolCallBegin(McpToolCallBeginEvent {
        call_id: invocation.id.clone(),
        invocation: invocation_event.clone(),
      });
      runtime.session.emit_event(begin.clone());
      if let Some(tx_event) = &runtime.tx_event {
        let _ = tx_event.send(begin).await;
      }
    }

    let start = Instant::now();
    let result = self.manager.call_tool(&invocation.name, arguments).await;
    let duration_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;

    if let Some(runtime) = &invocation.runtime {
      let end = EventMsg::McpToolCallEnd(McpToolCallEndEvent {
        call_id: invocation.id.clone(),
        invocation: invocation_event,
        duration_ms,
        result: match &result {
          Ok(result) => ProtocolMcpToolCallResult::Ok {
            content: result
              .content
              .iter()
              .filter_map(|value| serde_json::from_value(value.clone()).ok())
              .collect(),
          },
          Err(err) => ProtocolMcpToolCallResult::Err(err.to_string()),
        },
      });
      runtime.session.emit_event(end.clone());
      if let Some(tx_event) = &runtime.tx_event {
        let _ = tx_event.send(end).await;
      }
    }

    Ok(ToolOutput::Mcp {
      id: invocation.id,
      result: result.map_err(|err| err.to_string()),
    })
  }
}
