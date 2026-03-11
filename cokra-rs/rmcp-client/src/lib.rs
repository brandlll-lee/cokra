use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use rmcp::ClientHandler;
use rmcp::model::CallToolRequestParams;
use rmcp::model::CallToolResult;
use rmcp::model::ClientInfo;
use rmcp::model::ClientRequest;
use rmcp::model::Extensions;
use rmcp::model::InitializeRequestParams;
use rmcp::model::InitializeResult;
use rmcp::model::ListToolsResult;
use rmcp::model::PaginatedRequestParams;
use rmcp::model::ServerResult;
use rmcp::service::RoleClient;
use rmcp::service::RunningService;
use rmcp::service;
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::streamable_http_client::StreamableHttpClientWorker;
use rmcp::transport::worker::WorkerTransport;
use serde_json::Value;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time;

type StreamableHttpTransport = WorkerTransport<StreamableHttpClientWorker<reqwest::Client>>;

enum PendingTransport {
  ChildProcess {
    transport: TokioChildProcess,
  },
  StreamableHttp {
    transport: StreamableHttpTransport,
  },
}

enum ClientState {
  Connecting {
    transport: Option<PendingTransport>,
  },
  Ready {
    service: Arc<RunningService<RoleClient, NoopClientHandler>>,
  },
}

#[derive(Clone)]
struct NoopClientHandler {
  client_info: ClientInfo,
}

impl ClientHandler for NoopClientHandler {
  fn get_info(&self) -> ClientInfo {
    self.client_info.clone()
  }
}

pub struct RmcpClient {
  state: Mutex<ClientState>,
}

impl RmcpClient {
  pub async fn new_stdio_client(
    program: OsString,
    args: Vec<OsString>,
    env: Option<HashMap<String, String>>,
    cwd: Option<PathBuf>,
  ) -> Result<Self> {
    let mut command = Command::new(program);
    command
      .kill_on_drop(true)
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::null())
      .args(args);
    if let Some(env) = env {
      command.envs(env);
    }
    if let Some(cwd) = cwd {
      command.current_dir(cwd);
    }

    let (transport, _stderr) = TokioChildProcess::builder(command)
      .stderr(Stdio::null())
      .spawn()?;

    Ok(Self {
      state: Mutex::new(ClientState::Connecting {
        transport: Some(PendingTransport::ChildProcess { transport }),
      }),
    })
  }

  pub async fn new_streamable_http_client(
    url: &str,
    bearer_token: Option<String>,
    headers: Option<HashMap<String, String>>,
  ) -> Result<Self> {
    let mut reqwest_builder = reqwest::Client::builder();

    let mut default_headers = reqwest::header::HeaderMap::new();
    if let Some(token) = bearer_token {
      let value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|e| anyhow!("invalid bearer token header value: {e}"))?;
      default_headers.insert(reqwest::header::AUTHORIZATION, value);
    }
    if let Some(hdrs) = headers {
      for (key, value) in hdrs {
        let hname = reqwest::header::HeaderName::from_bytes(key.as_bytes())
          .map_err(|e| anyhow!("invalid header name '{key}': {e}"))?;
        let hvalue = reqwest::header::HeaderValue::from_str(&value)
          .map_err(|e| anyhow!("invalid header value for '{key}': {e}"))?;
        default_headers.insert(hname, hvalue);
      }
    }
    reqwest_builder = reqwest_builder.default_headers(default_headers);

    let client = reqwest_builder
      .build()
      .map_err(|e| anyhow!("failed to build HTTP client for {url}: {e}"))?;

    let config = StreamableHttpClientTransportConfig::with_uri(url);
    let worker = StreamableHttpClientWorker::new(client, config);
    let transport = WorkerTransport::spawn(worker);

    Ok(Self {
      state: Mutex::new(ClientState::Connecting {
        transport: Some(PendingTransport::StreamableHttp { transport }),
      }),
    })
  }

  pub async fn initialize(
    &self,
    params: InitializeRequestParams,
    timeout: Option<Duration>,
  ) -> Result<InitializeResult> {
    let handler = NoopClientHandler {
      client_info: ClientInfo {
        meta: params.meta.clone(),
        protocol_version: params.protocol_version,
        capabilities: params.capabilities.clone(),
        client_info: params.client_info.clone(),
      },
    };

    let transport = {
      let mut guard = self.state.lock().await;
      match &mut *guard {
        ClientState::Connecting { transport } => transport
          .take()
          .ok_or_else(|| anyhow!("client already initializing"))?,
        ClientState::Ready { .. } => return Err(anyhow!("client already initialized")),
      }
    };

    let service = match transport {
      PendingTransport::ChildProcess { transport } => {
        match timeout {
          Some(duration) => time::timeout(duration, service::serve_client(handler.clone(), transport))
            .await
            .map_err(|_| anyhow!("timed out handshaking with MCP server after {duration:?}"))??,
          None => service::serve_client(handler.clone(), transport).await?,
        }
      }
      PendingTransport::StreamableHttp { transport } => {
        match timeout {
          Some(duration) => time::timeout(duration, service::serve_client(handler.clone(), transport))
            .await
            .map_err(|_| anyhow!("timed out handshaking with MCP server after {duration:?}"))??,
          None => service::serve_client(handler.clone(), transport).await?,
        }
      }
    };

    let initialize_result = service
      .peer()
      .peer_info()
      .ok_or_else(|| anyhow!("handshake succeeded but server info was missing"))?;
    let initialize_result = initialize_result.clone();

    let mut guard = self.state.lock().await;
    *guard = ClientState::Ready {
      service: Arc::new(service),
    };

    Ok(initialize_result)
  }

  pub async fn list_tools(
    &self,
    params: Option<PaginatedRequestParams>,
    timeout: Option<Duration>,
  ) -> Result<ListToolsResult> {
    let service = self.service().await?;
    let fut = service.list_tools(params);
    Ok(match timeout {
      Some(duration) => time::timeout(duration, fut)
        .await
        .map_err(|_| anyhow!("tools/list timed out after {duration:?}"))??,
      None => fut.await?,
    })
  }

  pub async fn call_tool(
    &self,
    name: String,
    arguments: Option<Value>,
    timeout: Option<Duration>,
  ) -> Result<CallToolResult> {
    let service = self.service().await?;
    let arguments = match arguments {
      Some(Value::Object(map)) => Some(map),
      Some(other) => {
        return Err(anyhow!(
          "MCP tool arguments must be a JSON object, got {other}"
        ));
      }
      None => None,
    };
    let fut = service.call_tool(CallToolRequestParams {
      meta: None,
      name: name.into(),
      arguments,
      task: None,
    });
    Ok(match timeout {
      Some(duration) => time::timeout(duration, fut)
        .await
        .map_err(|_| anyhow!("tools/call timed out after {duration:?}"))??,
      None => fut.await?,
    })
  }

  pub async fn send_custom_request(
    &self,
    method: &str,
    params: Option<Value>,
  ) -> Result<ServerResult> {
    let service = self.service().await?;
    Ok(
      service
        .send_request(ClientRequest::CustomRequest(rmcp::model::CustomRequest {
          method: method.to_string(),
          params,
          extensions: Extensions::new(),
        }))
        .await?,
    )
  }

  async fn service(&self) -> Result<Arc<RunningService<RoleClient, NoopClientHandler>>> {
    let guard = self.state.lock().await;
    match &*guard {
      ClientState::Ready { service } => Ok(Arc::clone(service)),
      ClientState::Connecting { .. } => Err(anyhow!("MCP client not initialized")),
    }
  }
}
