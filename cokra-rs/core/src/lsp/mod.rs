mod client;
mod server;

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::sync::Notify;

use self::client::LspClient;
pub(crate) use self::client::LspDiagnostic;
pub use self::server::LspAuditEvent;
pub(crate) use self::server::LspManagerConfig;
use self::server::ResolvedServer;
use self::server::canonicalize_existing_path;
pub(crate) use self::server::path_to_uri;
use self::server::push_audit_event;
pub use self::server::recent_audit_events;
use self::server::resolve_server_for_path;
pub(crate) use self::server::uri_to_path;

const DEFAULT_MAX_DIAGNOSTICS: usize = 50;

#[derive(Debug, thiserror::Error)]
pub enum LspError {
  #[error("LSP is disabled by configuration")]
  Disabled,
  #[error("file not found: {0}")]
  FileNotFound(String),
  #[error("no LSP server available for {0}")]
  UnsupportedFile(String),
  #[error("{0}")]
  RequestFailed(String),
  #[error("{0}")]
  ServerUnavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ClientKey {
  server_id: String,
  root: PathBuf,
}

impl ClientKey {
  fn display(&self) -> String {
    format!("{}@{}", self.server_id, self.root.display())
  }
}

#[derive(Debug, Clone, Serialize)]
pub struct LspClientStatus {
  pub server_id: String,
  pub program: String,
  pub root: String,
  pub status: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LspManagerStatus {
  pub enabled: bool,
  pub auto_install: bool,
  pub request_timeout_ms: u64,
  pub diagnostics_timeout_ms: u64,
  pub clients: Vec<LspClientStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LspRestartReport {
  pub restarted_clients: usize,
  pub cleared_broken_entries: usize,
  pub keys: Vec<String>,
}

#[derive(Default)]
struct LspManagerState {
  clients: HashMap<ClientKey, Arc<LspClient>>,
  spawning: HashMap<ClientKey, Arc<Notify>>,
  broken: HashMap<ClientKey, String>,
}

pub struct LspManager {
  config: LspManagerConfig,
  state: Mutex<LspManagerState>,
}

impl Default for LspManager {
  fn default() -> Self {
    Self::new(LspManagerConfig::default())
  }
}

impl LspManager {
  pub fn new(config: LspManagerConfig) -> Self {
    Self {
      config,
      state: Mutex::new(LspManagerState::default()),
    }
  }

  pub async fn touch_file(
    &self,
    path: impl AsRef<Path>,
    wait_for_diagnostics: bool,
  ) -> Result<(), LspError> {
    let path = canonicalize_existing_path(path.as_ref()).await?;
    let client = self.client_for_path(&path).await?;
    if let Err(err) = client.touch_file(&path, wait_for_diagnostics).await {
      self
        .mark_client_broken(client_key_for(&client), err.to_string())
        .await;
      return Err(err);
    }
    Ok(())
  }

  pub async fn diagnostics(
    &self,
    path: impl AsRef<Path>,
    max_diagnostics: usize,
  ) -> Result<Vec<LspDiagnostic>, LspError> {
    let path = canonicalize_existing_path(path.as_ref()).await?;
    let client = self.client_for_path(&path).await?;
    if let Err(err) = client.touch_file(&path, true).await {
      self
        .mark_client_broken(client_key_for(&client), err.to_string())
        .await;
      return Err(err);
    }

    let mut diagnostics = client.diagnostics_for_path(&path).await;
    diagnostics.truncate(max_diagnostics);
    Ok(diagnostics)
  }

  pub async fn text_document_request(
    &self,
    path: impl AsRef<Path>,
    method: &str,
    params: Value,
    touch_before: bool,
  ) -> Result<Value, LspError> {
    let path = canonicalize_existing_path(path.as_ref()).await?;
    let client = self.client_for_path(&path).await?;
    if touch_before && let Err(err) = client.touch_file(&path, false).await {
      self
        .mark_client_broken(client_key_for(&client), err.to_string())
        .await;
      return Err(err);
    }
    let result = client.request(method, params).await;
    if let Err(err) = &result {
      self
        .mark_client_broken(client_key_for(&client), err.to_string())
        .await;
    }
    result
  }

  pub async fn workspace_request(
    &self,
    path: impl AsRef<Path>,
    method: &str,
    params: Value,
  ) -> Result<Value, LspError> {
    let path = canonicalize_existing_path(path.as_ref()).await?;
    let client = self.client_for_path(&path).await?;
    let result = client.request(method, params).await;
    if let Err(err) = &result {
      self
        .mark_client_broken(client_key_for(&client), err.to_string())
        .await;
    }
    result
  }

  pub async fn status(&self) -> LspManagerStatus {
    let state = self.state.lock().await;
    let mut clients = state
      .clients
      .values()
      .map(|client| LspClientStatus {
        server_id: client.server_id().to_string(),
        program: client.program().to_string(),
        root: client.root().display().to_string(),
        status: "connected".to_string(),
        error: None,
      })
      .collect::<Vec<_>>();
    clients.extend(state.broken.iter().map(|(key, reason)| LspClientStatus {
      server_id: key.server_id.clone(),
      program: key.server_id.clone(),
      root: key.root.display().to_string(),
      status: "broken".to_string(),
      error: Some(reason.clone()),
    }));
    clients.sort_by(|left, right| {
      left
        .server_id
        .cmp(&right.server_id)
        .then(left.root.cmp(&right.root))
    });
    LspManagerStatus {
      enabled: self.config.enabled,
      auto_install: self.config.auto_install,
      request_timeout_ms: self.config.request_timeout.as_millis() as u64,
      diagnostics_timeout_ms: self.config.diagnostics_timeout.as_millis() as u64,
      clients,
    }
  }

  pub async fn restart(
    &self,
    file_path: Option<&Path>,
    server_id: Option<&str>,
  ) -> Result<LspRestartReport, LspError> {
    let exact_key = if let Some(file_path) = file_path {
      let path = canonicalize_existing_path(file_path).await?;
      let resolved = resolve_server_for_path(&path, &self.config).await?;
      Some(ClientKey {
        server_id: resolved.definition.id.to_string(),
        root: resolved.root,
      })
    } else {
      None
    };

    let (clients, cleared_broken, keys) = {
      let mut state = self.state.lock().await;
      let broken_keys = state
        .broken
        .keys()
        .filter(|key| matches_restart_target(key, exact_key.as_ref(), server_id))
        .cloned()
        .collect::<Vec<_>>();
      let cleared_broken = broken_keys.len();
      for key in &broken_keys {
        state.broken.remove(key);
      }

      let client_keys = state
        .clients
        .keys()
        .filter(|key| matches_restart_target(key, exact_key.as_ref(), server_id))
        .cloned()
        .collect::<Vec<_>>();
      let keys = client_keys
        .iter()
        .chain(broken_keys.iter())
        .map(ClientKey::display)
        .collect::<Vec<_>>();
      let clients = client_keys
        .into_iter()
        .filter_map(|key| state.clients.remove(&key))
        .collect::<Vec<_>>();
      (clients, cleared_broken, keys)
    };

    for client in &clients {
      client.shutdown().await;
    }

    Ok(LspRestartReport {
      restarted_clients: clients.len(),
      cleared_broken_entries: cleared_broken,
      keys,
    })
  }

  async fn client_for_path(&self, path: &Path) -> Result<Arc<LspClient>, LspError> {
    if !self.config.enabled {
      return Err(LspError::Disabled);
    }

    let resolved = resolve_server_for_path(path, &self.config).await?;
    let key = ClientKey {
      server_id: resolved.definition.id.to_string(),
      root: resolved.root.clone(),
    };

    loop {
      let (existing, pending, broken) = {
        let state = self.state.lock().await;
        (
          state.clients.get(&key).cloned(),
          state.spawning.get(&key).cloned(),
          state.broken.get(&key).cloned(),
        )
      };

      if let Some(client) = existing {
        return Ok(client);
      }
      if let Some(reason) = broken {
        return Err(LspError::ServerUnavailable(reason));
      }
      if let Some(pending) = pending {
        pending.notified().await;
        continue;
      }

      let notify = Arc::new(Notify::new());
      {
        let mut state = self.state.lock().await;
        if let Some(client) = state.clients.get(&key).cloned() {
          return Ok(client);
        }
        if let Some(reason) = state.broken.get(&key).cloned() {
          return Err(LspError::ServerUnavailable(reason));
        }
        if let Some(pending) = state.spawning.get(&key).cloned() {
          drop(state);
          pending.notified().await;
          continue;
        }
        state.spawning.insert(key.clone(), Arc::clone(&notify));
      }

      return self.finish_spawn(key, resolved, notify).await;
    }
  }

  async fn finish_spawn(
    &self,
    key: ClientKey,
    resolved: ResolvedServer,
    notify: Arc<Notify>,
  ) -> Result<Arc<LspClient>, LspError> {
    match LspClient::spawn(resolved, &self.config).await {
      Ok(client) => {
        let mut state = self.state.lock().await;
        state.clients.insert(key.clone(), Arc::clone(&client));
        state.spawning.remove(&key);
        state.broken.remove(&key);
        notify.notify_waiters();
        push_audit_event(
          "client_connected",
          Some(&key.server_id),
          Some(key.root.display().to_string()),
          None,
        )
        .await;
        Ok(client)
      }
      Err(err) => {
        let mut state = self.state.lock().await;
        state.spawning.remove(&key);
        state.broken.insert(key, err.to_string());
        notify.notify_waiters();
        push_audit_event("client_connect_failed", None, None, Some(err.to_string())).await;
        Err(err)
      }
    }
  }

  async fn mark_client_broken(&self, key: ClientKey, reason: String) {
    let detail = reason.clone();
    let client = {
      let mut state = self.state.lock().await;
      state.broken.insert(key.clone(), reason);
      state.clients.remove(&key)
    };
    push_audit_event(
      "client_broken",
      Some(&key.server_id),
      Some(key.root.display().to_string()),
      Some(detail),
    )
    .await;
    if let Some(client) = client {
      client.shutdown().await;
    }
  }
}

pub fn manager() -> &'static LspManager {
  static MANAGER: OnceLock<LspManager> = OnceLock::new();
  MANAGER.get_or_init(LspManager::default)
}

pub async fn diagnostics_for_path(
  path: &Path,
  max_diagnostics: usize,
) -> Result<Vec<LspDiagnostic>, LspError> {
  manager().diagnostics(path, max_diagnostics).await
}

pub async fn collect_file_diagnostics(path: &Path) -> String {
  match diagnostics_for_path(path, DEFAULT_MAX_DIAGNOSTICS).await {
    Ok(diagnostics) if !diagnostics.is_empty() => {
      format!("\n\n{}", format_diagnostics(path, &diagnostics))
    }
    _ => String::new(),
  }
}

pub fn format_diagnostics(path: &Path, diagnostics: &[LspDiagnostic]) -> String {
  let mut lines = vec![format!(
    "Diagnostics for {} ({} found):\n",
    path.display(),
    diagnostics.len()
  )];
  for diagnostic in diagnostics {
    let severity = match diagnostic.severity {
      Some(1) => "ERROR",
      Some(2) => "WARN",
      Some(3) => "INFO",
      Some(4) => "HINT",
      _ => "ERROR",
    };
    let line = diagnostic.range.start.line + 1;
    let column = diagnostic.range.start.character + 1;
    let source = diagnostic
      .source
      .as_deref()
      .map(|value| format!("[{value}] "))
      .unwrap_or_default();
    lines.push(format!(
      "{severity} [{line}:{column}] {source}{}",
      diagnostic.message
    ));
  }
  lines.join("\n")
}

fn client_key_for(client: &Arc<LspClient>) -> ClientKey {
  ClientKey {
    server_id: client.server_id().to_string(),
    root: client.root().to_path_buf(),
  }
}

fn matches_restart_target(
  key: &ClientKey,
  exact: Option<&ClientKey>,
  server_id: Option<&str>,
) -> bool {
  if let Some(exact) = exact
    && key != exact
  {
    return false;
  }
  if let Some(server_id) = server_id
    && key.server_id != server_id
  {
    return false;
  }
  true
}
