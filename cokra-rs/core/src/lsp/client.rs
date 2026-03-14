use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::ChildStdin;
use tokio::process::ChildStdout;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::RwLock;
use tokio::sync::oneshot;
use tokio::time::timeout;

use super::LspError;
use super::server::LspManagerConfig;
use super::server::ResolvedServer;
use super::server::path_to_uri;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LspPosition {
  pub line: u32,
  pub character: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LspRange {
  pub start: LspPosition,
  pub end: LspPosition,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LspDiagnostic {
  pub message: String,
  pub range: LspRange,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub severity: Option<u8>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub code: Option<Value>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub source: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PublishDiagnosticsParams {
  uri: String,
  diagnostics: Vec<LspDiagnostic>,
}

#[derive(Serialize)]
struct LspRequestMessage<'a> {
  jsonrpc: &'static str,
  id: u64,
  method: &'a str,
  params: Value,
}

#[derive(Serialize)]
struct LspNotificationMessage<'a> {
  jsonrpc: &'static str,
  method: &'a str,
  params: Value,
}

#[derive(Deserialize)]
struct LspMessage {
  method: Option<String>,
  params: Option<Value>,
  id: Option<Value>,
  result: Option<Value>,
  error: Option<Value>,
}

struct ClientState {
  stdin: Mutex<ChildStdin>,
  child: Mutex<Option<Child>>,
  pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
  diagnostics: RwLock<HashMap<String, Vec<LspDiagnostic>>>,
  diagnostic_revisions: Mutex<HashMap<String, u64>>,
  document_versions: Mutex<HashMap<String, i32>>,
  diagnostics_notify: Notify,
  next_request_id: AtomicU64,
  request_timeout: std::time::Duration,
  diagnostics_timeout: std::time::Duration,
}

pub(crate) struct LspClient {
  server_id: String,
  root: PathBuf,
  program: String,
  language_id: &'static str,
  state: Arc<ClientState>,
}

impl LspClient {
  pub(crate) async fn spawn(
    resolved: ResolvedServer,
    config: &LspManagerConfig,
  ) -> Result<Arc<Self>, LspError> {
    let mut child = Command::new(&resolved.program)
      .args(&resolved.args)
      .current_dir(&resolved.root)
      .stdin(std::process::Stdio::piped())
      .stdout(std::process::Stdio::piped())
      .stderr(std::process::Stdio::null())
      .kill_on_drop(true)
      .spawn()
      .map_err(|err| {
        LspError::ServerUnavailable(format!(
          "failed to spawn {}: {err}",
          resolved.program.display()
        ))
      })?;

    let stdin = child
      .stdin
      .take()
      .ok_or_else(|| LspError::ServerUnavailable("failed to capture LSP stdin".to_string()))?;
    let stdout = child
      .stdout
      .take()
      .ok_or_else(|| LspError::ServerUnavailable("failed to capture LSP stdout".to_string()))?;

    let state = Arc::new(ClientState {
      stdin: Mutex::new(stdin),
      child: Mutex::new(Some(child)),
      pending: Mutex::new(HashMap::new()),
      diagnostics: RwLock::new(HashMap::new()),
      diagnostic_revisions: Mutex::new(HashMap::new()),
      document_versions: Mutex::new(HashMap::new()),
      diagnostics_notify: Notify::new(),
      next_request_id: AtomicU64::new(1),
      request_timeout: config.request_timeout,
      diagnostics_timeout: config.diagnostics_timeout,
    });

    tokio::spawn(run_reader_loop(stdout, Arc::clone(&state)));

    let client = Arc::new(Self {
      server_id: resolved.definition.id.to_string(),
      root: resolved.root.clone(),
      program: resolved.program.display().to_string(),
      language_id: resolved.definition.language_id,
      state,
    });

    client
      .request(
        "initialize",
        serde_json::json!({
          "processId": std::process::id(),
          "rootUri": path_to_uri(&resolved.root),
          "workspaceFolders": [{
            "name": resolved.root.file_name().and_then(|name| name.to_str()).unwrap_or("workspace"),
            "uri": path_to_uri(&resolved.root)
          }],
          "capabilities": {
            "textDocument": {
              "publishDiagnostics": {
                "relatedInformation": true
              }
            },
            "workspace": {
              "configuration": false,
              "workspaceFolders": true
            }
          },
          "clientInfo": {
            "name": "cokra",
            "version": env!("CARGO_PKG_VERSION")
          }
        }),
      )
      .await?;
    client.notify("initialized", serde_json::json!({})).await?;

    Ok(client)
  }

  pub(crate) fn server_id(&self) -> &str {
    &self.server_id
  }

  pub(crate) fn root(&self) -> &Path {
    &self.root
  }

  pub(crate) fn program(&self) -> &str {
    &self.program
  }

  pub(crate) async fn request(&self, method: &str, params: Value) -> Result<Value, LspError> {
    let id = self.state.next_request_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    self.state.pending.lock().await.insert(id, tx);

    if let Err(err) = self
      .write_message(&LspRequestMessage {
        jsonrpc: "2.0",
        id,
        method,
        params,
      })
      .await
    {
      self.state.pending.lock().await.remove(&id);
      return Err(err);
    }

    match timeout(self.state.request_timeout, rx).await {
      Ok(Ok(Ok(result))) => Ok(result),
      Ok(Ok(Err(err))) => Err(LspError::RequestFailed(err)),
      Ok(Err(_)) => Err(LspError::RequestFailed(format!(
        "LSP request channel closed for {}@{}",
        self.server_id,
        self.root.display()
      ))),
      Err(_) => {
        self.state.pending.lock().await.remove(&id);
        Err(LspError::RequestFailed(format!(
          "LSP request '{method}' timed out after {}ms",
          self.state.request_timeout.as_millis()
        )))
      }
    }
  }

  pub(crate) async fn notify(&self, method: &str, params: Value) -> Result<(), LspError> {
    self
      .write_message(&LspNotificationMessage {
        jsonrpc: "2.0",
        method,
        params,
      })
      .await
  }

  pub(crate) async fn touch_file(
    &self,
    path: &Path,
    wait_for_diagnostics: bool,
  ) -> Result<(), LspError> {
    let file_content = tokio::fs::read_to_string(path).await.map_err(|err| {
      LspError::RequestFailed(format!("failed to read {}: {err}", path.display()))
    })?;
    let uri = path_to_uri(path);
    let previous_revision = self
      .state
      .diagnostic_revisions
      .lock()
      .await
      .get(&uri)
      .copied()
      .unwrap_or(0);

    let version = {
      let mut versions = self.state.document_versions.lock().await;
      let next = versions.get(&uri).copied().unwrap_or(0) + 1;
      versions.insert(uri.clone(), next);
      next
    };

    if version == 1 {
      self
        .notify(
          "textDocument/didOpen",
          serde_json::json!({
            "textDocument": {
              "uri": uri,
              "languageId": self.language_id,
              "version": version,
              "text": file_content
            }
          }),
        )
        .await?;
    } else {
      self
        .notify(
          "textDocument/didChange",
          serde_json::json!({
            "textDocument": {
              "uri": uri,
              "version": version
            },
            "contentChanges": [{
              "text": file_content
            }]
          }),
        )
        .await?;
    }

    if wait_for_diagnostics {
      self
        .wait_for_diagnostics_revision(&uri, previous_revision)
        .await?;
    }

    Ok(())
  }

  pub(crate) async fn diagnostics_for_path(&self, path: &Path) -> Vec<LspDiagnostic> {
    self.diagnostics_for_uri(&path_to_uri(path)).await
  }

  pub(crate) async fn diagnostics_for_uri(&self, uri: &str) -> Vec<LspDiagnostic> {
    self
      .state
      .diagnostics
      .read()
      .await
      .get(uri)
      .cloned()
      .unwrap_or_default()
  }

  pub(crate) async fn shutdown(&self) {
    let _ = self.request("shutdown", Value::Null).await;
    let _ = self.notify("exit", Value::Null).await;
    if let Some(mut child) = self.state.child.lock().await.take() {
      let _ = child.kill().await;
    }
  }

  async fn write_message<T: Serialize>(&self, message: &T) -> Result<(), LspError> {
    let body = serde_json::to_vec(message)
      .map_err(|err| LspError::RequestFailed(format!("failed to encode LSP message: {err}")))?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut stdin = self.state.stdin.lock().await;
    stdin
      .write_all(header.as_bytes())
      .await
      .map_err(|err| LspError::RequestFailed(format!("failed to write LSP header: {err}")))?;
    stdin
      .write_all(&body)
      .await
      .map_err(|err| LspError::RequestFailed(format!("failed to write LSP body: {err}")))?;
    stdin
      .flush()
      .await
      .map_err(|err| LspError::RequestFailed(format!("failed to flush LSP stdin: {err}")))
  }

  async fn wait_for_diagnostics_revision(
    &self,
    uri: &str,
    previous_revision: u64,
  ) -> Result<(), LspError> {
    timeout(self.state.diagnostics_timeout, async {
      loop {
        let notified = self.state.diagnostics_notify.notified();
        let current = self
          .state
          .diagnostic_revisions
          .lock()
          .await
          .get(uri)
          .copied()
          .unwrap_or(0);
        if current > previous_revision {
          return;
        }
        notified.await;
      }
    })
    .await
    .map_err(|_| {
      LspError::RequestFailed(format!(
        "timed out waiting for diagnostics after {}ms",
        self.state.diagnostics_timeout.as_millis()
      ))
    })
  }
}

async fn run_reader_loop(stdout: ChildStdout, state: Arc<ClientState>) {
  let mut reader = BufReader::new(stdout);
  loop {
    match read_lsp_message(&mut reader).await {
      Ok(message) => {
        if message.method.as_deref() == Some("textDocument/publishDiagnostics")
          && let Some(params) = message.params
          && let Ok(params) = serde_json::from_value::<PublishDiagnosticsParams>(params)
        {
          state
            .diagnostics
            .write()
            .await
            .insert(params.uri.clone(), params.diagnostics);
          let mut revisions = state.diagnostic_revisions.lock().await;
          let next = revisions.get(&params.uri).copied().unwrap_or(0) + 1;
          revisions.insert(params.uri, next);
          drop(revisions);
          state.diagnostics_notify.notify_waiters();
          continue;
        }

        if let Some(id) = message.id.and_then(|value| value.as_u64())
          && let Some(sender) = state.pending.lock().await.remove(&id)
        {
          let payload = if let Some(error) = message.error {
            Err(error.to_string())
          } else {
            Ok(message.result.unwrap_or(Value::Null))
          };
          let _ = sender.send(payload);
        }
      }
      Err(err) => {
        let mut pending = state.pending.lock().await;
        for (_, sender) in pending.drain() {
          let _ = sender.send(Err(err.clone()));
        }
        break;
      }
    }
  }
}

async fn read_lsp_message<R>(reader: &mut BufReader<R>) -> Result<LspMessage, String>
where
  R: AsyncRead + Unpin,
{
  let mut content_length: Option<usize> = None;
  loop {
    let mut line = String::new();
    let read = reader
      .read_line(&mut line)
      .await
      .map_err(|err| format!("failed to read LSP header: {err}"))?;
    if read == 0 {
      return Err("LSP stream closed".to_string());
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
      break;
    }
    if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
      content_length = rest.trim().parse().ok();
    }
  }

  let length = content_length.ok_or_else(|| "missing Content-Length".to_string())?;
  let mut body = vec![0u8; length];
  reader
    .read_exact(&mut body)
    .await
    .map_err(|err| format!("failed to read LSP body: {err}"))?;
  serde_json::from_slice::<LspMessage>(&body)
    .map_err(|err| format!("failed to decode LSP message: {err}"))
}
