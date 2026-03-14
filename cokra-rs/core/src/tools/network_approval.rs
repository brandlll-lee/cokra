use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;

use serde::Serialize;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::session::Session;
use crate::tools::context::ToolRuntimeContext;
use crate::tools::sandboxing::ToolError;
use cokra_protocol::AskForApproval;
use cokra_protocol::ReviewDecision;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkApprovalMode {
  Immediate,
  Deferred,
}

#[derive(Clone, Debug)]
pub struct NetworkApprovalSpec {
  pub mode: NetworkApprovalMode,
}

#[derive(Clone, Debug)]
pub struct DeferredNetworkApproval {
  attempt_id: String,
}

impl DeferredNetworkApproval {
  pub fn attempt_id(&self) -> &str {
    &self.attempt_id
  }
}

#[derive(Debug)]
pub struct ActiveNetworkApproval {
  attempt_id: Option<String>,
  mode: NetworkApprovalMode,
}

impl ActiveNetworkApproval {
  pub fn attempt_id(&self) -> Option<&str> {
    self.attempt_id.as_deref()
  }

  pub fn mode(&self) -> NetworkApprovalMode {
    self.mode
  }

  pub fn into_deferred(self) -> Option<DeferredNetworkApproval> {
    match (self.mode, self.attempt_id) {
      (NetworkApprovalMode::Deferred, Some(attempt_id)) => {
        Some(DeferredNetworkApproval { attempt_id })
      }
      _ => None,
    }
  }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NetworkApprovalOutcome {
  DeniedByUser,
  DeniedByPolicy(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct NetworkAuditEvent {
  pub timestamp: String,
  pub host: String,
  pub protocol: String,
  pub port: u16,
  pub decision: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct HostApprovalKey {
  host: String,
  protocol: String,
  port: u16,
}

impl HostApprovalKey {
  fn new(host: &str, protocol: &str, port: u16) -> Self {
    Self {
      host: normalize_domain(host),
      protocol: protocol.to_ascii_lowercase(),
      port,
    }
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingApprovalDecision {
  AllowOnce,
  AllowForSession,
  Deny,
}

struct PendingHostApproval {
  decision: Mutex<Option<PendingApprovalDecision>>,
  notify: Notify,
}

impl PendingHostApproval {
  fn new() -> Self {
    Self {
      decision: Mutex::new(None),
      notify: Notify::new(),
    }
  }

  async fn wait_for_decision(&self) -> PendingApprovalDecision {
    loop {
      let notified = self.notify.notified();
      if let Some(decision) = *self.decision.lock().await {
        return decision;
      }
      notified.await;
    }
  }

  async fn set_decision(&self, decision: PendingApprovalDecision) {
    *self.decision.lock().await = Some(decision);
    self.notify.notify_waiters();
  }
}

#[derive(Default)]
struct NetworkApprovalService {
  attempts: Mutex<HashMap<String, Option<NetworkApprovalOutcome>>>,
  pending_host_approvals: Mutex<HashMap<HostApprovalKey, Arc<PendingHostApproval>>>,
  session_approved_hosts: Mutex<HashSet<HostApprovalKey>>,
  session_denied_hosts: Mutex<HashSet<HostApprovalKey>>,
  audit_log: Mutex<Vec<NetworkAuditEvent>>,
}

impl NetworkApprovalService {
  async fn register_attempt(&self, attempt_id: String) {
    self.attempts.lock().await.insert(attempt_id, None);
  }

  async fn unregister_attempt(&self, attempt_id: &str) {
    self.attempts.lock().await.remove(attempt_id);
  }

  async fn set_outcome(&self, attempt_id: &str, outcome: NetworkApprovalOutcome) {
    if let Some(slot) = self.attempts.lock().await.get_mut(attempt_id) {
      *slot = Some(outcome);
    }
  }

  async fn take_outcome(&self, attempt_id: &str) -> Option<NetworkApprovalOutcome> {
    let mut attempts = self.attempts.lock().await;
    attempts.get_mut(attempt_id).and_then(Option::take)
  }

  async fn get_or_create_pending_approval(
    &self,
    key: HostApprovalKey,
  ) -> (Arc<PendingHostApproval>, bool) {
    let mut pending = self.pending_host_approvals.lock().await;
    if let Some(existing) = pending.get(&key).cloned() {
      return (existing, false);
    }

    let created = Arc::new(PendingHostApproval::new());
    pending.insert(key, Arc::clone(&created));
    (created, true)
  }

  async fn push_audit_event(&self, event: NetworkAuditEvent) {
    let mut audit_log = self.audit_log.lock().await;
    audit_log.push(event);
    if audit_log.len() > 200 {
      let drain = audit_log.len() - 200;
      audit_log.drain(..drain);
    }
  }
}

fn service() -> &'static NetworkApprovalService {
  static SERVICE: OnceLock<NetworkApprovalService> = OnceLock::new();
  SERVICE.get_or_init(NetworkApprovalService::default)
}

pub async fn begin_network_approval(
  _session: &Session,
  _turn_id: &str,
  _call_id: &str,
  has_managed_network_requirements: bool,
  spec: Option<NetworkApprovalSpec>,
) -> Option<ActiveNetworkApproval> {
  let spec = spec?;
  if !has_managed_network_requirements {
    return None;
  }

  let attempt_id = Uuid::new_v4().to_string();
  service().register_attempt(attempt_id.clone()).await;

  Some(ActiveNetworkApproval {
    attempt_id: Some(attempt_id),
    mode: spec.mode,
  })
}

pub async fn authorize_http_url(
  runtime: &ToolRuntimeContext,
  cwd: &Path,
  url: &str,
  default_allowed_domains: &[&str],
) -> Result<(), String> {
  let parsed = reqwest::Url::parse(url).map_err(|err| format!("invalid url: {err}"))?;
  let host = parsed
    .host_str()
    .ok_or_else(|| "url is missing a host".to_string())?;
  let protocol = parsed.scheme().to_ascii_lowercase();
  let port = parsed
    .port_or_known_default()
    .ok_or_else(|| "url is missing a port".to_string())?;
  let key = HostApprovalKey::new(host, &protocol, port);

  if host_matches_any(&key.host, &runtime.denied_domains) {
    let reason = format!(
      "Network access to \"{}://{}:{}\" was blocked by denied_domains policy.",
      key.protocol, key.host, key.port
    );
    record_network_audit_event(&key, "policy_denied", Some(reason.clone())).await;
    record_runtime_outcome(
      runtime,
      NetworkApprovalOutcome::DeniedByPolicy(reason.clone()),
    )
    .await;
    return Err(reason);
  }

  if host_matches_any(&key.host, &runtime.allowed_domains)
    || host_matches_any(&key.host, default_allowed_domains)
  {
    return Ok(());
  }

  {
    let denied = service().session_denied_hosts.lock().await;
    if denied.contains(&key) {
      record_runtime_outcome(runtime, NetworkApprovalOutcome::DeniedByUser).await;
      return Err(format!(
        "Network access to \"{}://{}:{}\" was rejected by the user.",
        key.protocol, key.host, key.port
      ));
    }
  }

  {
    let allowed = service().session_approved_hosts.lock().await;
    if allowed.contains(&key) {
      return Ok(());
    }
  }

  if !runtime.has_managed_network_requirements && runtime.allowed_domains.is_empty() {
    return Ok(());
  }

  if !allows_network_prompt(runtime.approval_policy.clone()) {
    let reason = format!(
      "Network access to \"{}://{}:{}\" is blocked by approval policy.",
      key.protocol, key.host, key.port
    );
    record_network_audit_event(&key, "policy_denied", Some(reason.clone())).await;
    record_runtime_outcome(
      runtime,
      NetworkApprovalOutcome::DeniedByPolicy(reason.clone()),
    )
    .await;
    return Err(reason);
  }

  let (pending, is_owner) = service().get_or_create_pending_approval(key.clone()).await;
  if !is_owner {
    return resolve_pending_decision(&key, pending.wait_for_decision().await).await;
  }

  let approval_id = format!("network#{}#{}#{}", key.protocol, key.host, key.port);
  let decision = runtime
    .session
    .request_exec_approval(
      runtime.thread_id.clone(),
      runtime.turn_id.clone(),
      approval_id,
      "network_access".to_string(),
      format!("{}://{}:{}", key.protocol, key.host, key.port),
      cwd.to_path_buf(),
      runtime.tx_event.clone(),
    )
    .await;

  let resolved = match decision {
    ReviewDecision::Approved => PendingApprovalDecision::AllowOnce,
    ReviewDecision::Always => PendingApprovalDecision::AllowForSession,
    ReviewDecision::Denied => PendingApprovalDecision::Deny,
  };

  match resolved {
    PendingApprovalDecision::AllowOnce => {
      record_network_audit_event(&key, "allow_once", None).await;
    }
    PendingApprovalDecision::AllowForSession => {
      record_network_audit_event(&key, "allow_session", None).await;
    }
    PendingApprovalDecision::Deny => {
      record_network_audit_event(&key, "deny", None).await;
    }
  }

  if matches!(resolved, PendingApprovalDecision::AllowForSession) {
    service().session_denied_hosts.lock().await.remove(&key);
    service()
      .session_approved_hosts
      .lock()
      .await
      .insert(key.clone());
  }

  if matches!(resolved, PendingApprovalDecision::Deny) {
    service().session_approved_hosts.lock().await.remove(&key);
    service()
      .session_denied_hosts
      .lock()
      .await
      .insert(key.clone());
    record_runtime_outcome(runtime, NetworkApprovalOutcome::DeniedByUser).await;
  }

  pending.set_decision(resolved).await;
  service().pending_host_approvals.lock().await.remove(&key);
  resolve_pending_decision(&key, resolved).await
}

pub async fn record_network_approval_outcome(attempt_id: &str, outcome: NetworkApprovalOutcome) {
  service().set_outcome(attempt_id, outcome).await;
}

pub async fn record_network_policy_denial(attempt_id: &str, message: impl Into<String>) {
  service()
    .set_outcome(
      attempt_id,
      NetworkApprovalOutcome::DeniedByPolicy(message.into()),
    )
    .await;
}

pub async fn is_network_approval_attempt_active(attempt_id: &str) -> bool {
  service().attempts.lock().await.contains_key(attempt_id)
}

pub async fn recent_network_audit_events(limit: usize) -> Vec<NetworkAuditEvent> {
  let audit_log = service().audit_log.lock().await;
  let mut events = audit_log
    .iter()
    .rev()
    .take(limit.max(1))
    .cloned()
    .collect::<Vec<_>>();
  events.reverse();
  events
}

pub async fn finish_immediate_network_approval(
  _session: &Session,
  active: ActiveNetworkApproval,
) -> Result<(), ToolError> {
  let Some(attempt_id) = active.attempt_id() else {
    return Ok(());
  };

  let outcome = service().take_outcome(attempt_id).await;
  service().unregister_attempt(attempt_id).await;

  match outcome {
    Some(NetworkApprovalOutcome::DeniedByUser) => {
      Err(ToolError::Rejected("rejected by user".to_string()))
    }
    Some(NetworkApprovalOutcome::DeniedByPolicy(message)) => Err(ToolError::Rejected(message)),
    None => Ok(()),
  }
}

pub async fn finish_deferred_network_approval(
  _session: &Session,
  deferred: Option<DeferredNetworkApproval>,
) {
  let Some(deferred) = deferred else {
    return;
  };
  service().unregister_attempt(deferred.attempt_id()).await;
}

async fn resolve_pending_decision(
  key: &HostApprovalKey,
  decision: PendingApprovalDecision,
) -> Result<(), String> {
  match decision {
    PendingApprovalDecision::AllowOnce | PendingApprovalDecision::AllowForSession => Ok(()),
    PendingApprovalDecision::Deny => Err(format!(
      "Network access to \"{}://{}:{}\" was rejected by the user.",
      key.protocol, key.host, key.port
    )),
  }
}

fn normalize_domain(domain: &str) -> String {
  domain.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn host_matches_any(host: &str, patterns: &[impl AsRef<str>]) -> bool {
  let host = normalize_domain(host);
  patterns.iter().any(|pattern| {
    let pattern = normalize_domain(pattern.as_ref());
    host == pattern || host.ends_with(&format!(".{pattern}"))
  })
}

fn allows_network_prompt(policy: AskForApproval) -> bool {
  !matches!(policy, AskForApproval::Never)
}

async fn record_runtime_outcome(runtime: &ToolRuntimeContext, outcome: NetworkApprovalOutcome) {
  if let Some(attempt_id) = runtime.network_attempt_id.as_deref() {
    service().set_outcome(attempt_id, outcome).await;
  }
}

async fn record_network_audit_event(key: &HostApprovalKey, decision: &str, reason: Option<String>) {
  service()
    .push_audit_event(NetworkAuditEvent {
      timestamp: chrono::Utc::now().to_rfc3339(),
      host: key.host.clone(),
      protocol: key.protocol.clone(),
      port: key.port,
      decision: decision.to_string(),
      reason,
    })
    .await;
}

#[cfg(test)]
mod tests {
  use super::*;

  fn runtime(session: Arc<Session>) -> ToolRuntimeContext {
    ToolRuntimeContext {
      session,
      tool_registry: Arc::new(crate::tools::registry::ToolRegistry::new()),
      tx_event: None,
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      approval_policy: AskForApproval::OnRequest,
      model_provider_id: None,
      model_runtime_kind: None,
      supports_native_web_search: false,
      has_managed_network_requirements: true,
      allowed_domains: Vec::new(),
      denied_domains: Vec::new(),
      network_attempt_id: Some("attempt-1".to_string()),
    }
  }

  #[tokio::test]
  async fn immediate_finalize_propagates_user_denial() {
    let session = Session::new();
    let active = begin_network_approval(
      &session,
      "turn-1",
      "call-1",
      true,
      Some(NetworkApprovalSpec {
        mode: NetworkApprovalMode::Immediate,
      }),
    )
    .await
    .expect("active approval");

    let attempt_id = active.attempt_id().expect("attempt id").to_string();
    record_network_approval_outcome(&attempt_id, NetworkApprovalOutcome::DeniedByUser).await;

    let result = finish_immediate_network_approval(&session, active).await;
    assert!(matches!(result, Err(ToolError::Rejected(_))));
  }

  #[tokio::test]
  async fn deferred_finalize_unregisters_attempt() {
    let session = Session::new();
    let active = begin_network_approval(
      &session,
      "turn-1",
      "call-1",
      true,
      Some(NetworkApprovalSpec {
        mode: NetworkApprovalMode::Deferred,
      }),
    )
    .await
    .expect("active approval");

    let deferred = active.into_deferred();
    let attempt_id = deferred
      .as_ref()
      .expect("deferred handle")
      .attempt_id()
      .to_string();

    finish_deferred_network_approval(&session, deferred).await;
    assert!(!is_network_approval_attempt_active(&attempt_id).await);
  }

  #[tokio::test]
  async fn pending_approvals_are_deduped_per_host_protocol_and_port() {
    let key = HostApprovalKey::new("example.com", "https", 443);
    let (first, first_is_owner) = service().get_or_create_pending_approval(key.clone()).await;
    let (second, second_is_owner) = service().get_or_create_pending_approval(key).await;

    assert!(first_is_owner);
    assert!(!second_is_owner);
    assert!(Arc::ptr_eq(&first, &second));
  }

  #[tokio::test]
  async fn authorize_http_url_blocks_denied_domains() {
    let session = Arc::new(Session::new());
    let mut runtime = runtime(session);
    runtime.denied_domains = vec!["example.com".to_string()];

    let result =
      authorize_http_url(&runtime, Path::new("."), "https://example.com/docs", &[]).await;
    assert!(result.is_err());
  }

  #[tokio::test]
  async fn authorize_http_url_caches_session_allow() {
    let session = Arc::new(Session::new());
    let runtime = runtime(Arc::clone(&session));

    let waiter = {
      let runtime = runtime.clone();
      tokio::spawn(async move {
        authorize_http_url(&runtime, Path::new("."), "https://docs.example.com", &[]).await
      })
    };

    let mut notified = false;
    for _ in 0..20 {
      if session
        .notify_exec_approval("network#https#docs.example.com#443", ReviewDecision::Always)
        .await
      {
        notified = true;
        break;
      }
      tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(notified);

    assert!(waiter.await.expect("waiter task").is_ok());
    assert!(
      authorize_http_url(
        &runtime,
        Path::new("."),
        "https://docs.example.com/reference",
        &[],
      )
      .await
      .is_ok()
    );
  }
}
