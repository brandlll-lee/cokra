use std::sync::Arc;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolOutput;
use crate::tools::router::ToolCall;
use crate::tools::router::ToolRouter;
use crate::tools::router::ToolRunContext;

/// Runtime gate for parallel tool execution.
///
/// Non-mutating tools run concurrently under a read lock.
/// Mutating tools acquire a write lock to serialize execution.
#[derive(Clone)]
pub(crate) struct ToolCallRuntime {
  router: Arc<ToolRouter>,
  parallel_execution: Arc<RwLock<()>>,
}

impl ToolCallRuntime {
  pub(crate) fn new(router: Arc<ToolRouter>) -> Self {
    Self {
      router,
      parallel_execution: Arc::new(RwLock::new(())),
    }
  }

  pub(crate) async fn handle_tool_call_with_cancellation(
    &self,
    call: ToolCall,
    run_ctx: ToolRunContext,
    cancellation_token: CancellationToken,
  ) -> Result<ToolOutput, FunctionCallError> {
    if self.router.tool_supports_parallel(&call) {
      tokio::select! {
        _ = cancellation_token.cancelled() => Err(FunctionCallError::Fatal("tool call cancelled".to_string())),
        out = async {
          let _guard = self.parallel_execution.read().await;
          self.router.dispatch_tool_call(call, run_ctx).await
        } => out,
      }
    } else {
      tokio::select! {
        _ = cancellation_token.cancelled() => Err(FunctionCallError::Fatal("tool call cancelled".to_string())),
        out = async {
          let _guard = self.parallel_execution.write().await;
          self.router.dispatch_tool_call(call, run_ctx).await
        } => out,
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;
  use std::sync::atomic::AtomicUsize;
  use std::sync::atomic::Ordering;

  use async_trait::async_trait;

  use super::*;
  use crate::session::Session;
  use crate::tools::context::ToolInvocation;
  use crate::tools::context::ToolOutput;
  use crate::tools::registry::ToolHandler;
  use crate::tools::registry::ToolKind;
  use crate::tools::registry::ToolRegistry;
  use crate::tools::router::ToolRouter;
  use crate::tools::validation::ToolValidator;
  use cokra_config::ApprovalMode;
  use cokra_config::ApprovalPolicy;
  use cokra_config::PatchApproval;
  use cokra_config::SandboxConfig;
  use cokra_config::SandboxMode;
  use cokra_config::ShellApproval;
  use cokra_protocol::AskForApproval;
  use cokra_protocol::ReadOnlyAccess;
  use cokra_protocol::SandboxPolicy;

  struct CountingHandler {
    current: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    mutating: bool,
  }

  #[async_trait]
  impl ToolHandler for CountingHandler {
    fn kind(&self) -> ToolKind {
      ToolKind::Function
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
      self.mutating
    }

    async fn handle_async(
      &self,
      invocation: ToolInvocation,
    ) -> Result<ToolOutput, crate::tools::context::FunctionCallError> {
      let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
      loop {
        let peak = self.peak.load(Ordering::SeqCst);
        if now <= peak
          || self
            .peak
            .compare_exchange(peak, now, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
          break;
        }
      }

      tokio::time::sleep(std::time::Duration::from_millis(30)).await;
      self.current.fetch_sub(1, Ordering::SeqCst);

      Ok(ToolOutput::success("ok").with_id(invocation.id))
    }
  }

  fn validator() -> Arc<ToolValidator> {
    Arc::new(ToolValidator::new(
      SandboxConfig {
        mode: SandboxMode::Permissive,
        network_access: false,
      },
      ApprovalPolicy {
        policy: ApprovalMode::Auto,
        shell: ShellApproval::OnFailure,
        patch: PatchApproval::OnRequest,
      },
    ))
  }

  fn run_ctx(session: Arc<Session>) -> ToolRunContext {
    ToolRunContext::new(
      session,
      "thread-1".to_string(),
      "turn-1".to_string(),
      std::path::PathBuf::from("."),
      AskForApproval::OnRequest,
      SandboxPolicy::ReadOnly {
        access: ReadOnlyAccess::FullAccess,
      },
    )
  }

  #[tokio::test]
  async fn mutating_tool_is_serialized() {
    let current = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register_handler(
      "write_file",
      Arc::new(CountingHandler {
        current: Arc::clone(&current),
        peak: Arc::clone(&peak),
        mutating: true,
      }),
    );

    let router = Arc::new(ToolRouter::new(Arc::new(registry), validator()));
    let runtime = ToolCallRuntime::new(router);
    let session = Arc::new(Session::new());

    let c1 = ToolCall {
      tool_name: "write_file".to_string(),
      call_id: "call-1".to_string(),
      args: serde_json::json!({}),
    };
    let c2 = ToolCall {
      tool_name: "write_file".to_string(),
      call_id: "call-2".to_string(),
      args: serde_json::json!({}),
    };

    let f1 = runtime.handle_tool_call_with_cancellation(
      c1,
      run_ctx(Arc::clone(&session)),
      CancellationToken::new(),
    );
    let f2 =
      runtime.handle_tool_call_with_cancellation(c2, run_ctx(session), CancellationToken::new());
    let _ = tokio::join!(f1, f2);

    assert_eq!(peak.load(Ordering::SeqCst), 1);
  }

  #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
  async fn readonly_tools_run_in_parallel() {
    let current = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register_handler(
      "read_file",
      Arc::new(CountingHandler {
        current: Arc::clone(&current),
        peak: Arc::clone(&peak),
        mutating: false,
      }),
    );

    let router = Arc::new(ToolRouter::new(Arc::new(registry), validator()));
    let runtime = ToolCallRuntime::new(router);
    let session = Arc::new(Session::new());

    let c1 = ToolCall {
      tool_name: "read_file".to_string(),
      call_id: "call-1".to_string(),
      args: serde_json::json!({}),
    };
    let c2 = ToolCall {
      tool_name: "read_file".to_string(),
      call_id: "call-2".to_string(),
      args: serde_json::json!({}),
    };

    let f1 = runtime.handle_tool_call_with_cancellation(
      c1,
      run_ctx(Arc::clone(&session)),
      CancellationToken::new(),
    );
    let f2 =
      runtime.handle_tool_call_with_cancellation(c2, run_ctx(session), CancellationToken::new());
    let _ = tokio::join!(f1, f2);

    assert!(
      peak.load(Ordering::SeqCst) >= 2,
      "expected readonly tools to overlap, saw peak={}",
      peak.load(Ordering::SeqCst)
    );
  }
}
