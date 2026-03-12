//! Authentication credential storage and persisted auth records.

use crate::model::auth::AuthError;
use crate::model::auth::Result;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

/// Credentials for authenticating with a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Credentials {
  /// API Key authentication.
  #[serde(rename = "api_key")]
  ApiKey { key: String },

  /// OAuth authentication.
  #[serde(rename = "oauth")]
  OAuth {
    /// Access token.
    access_token: String,

    /// Refresh token.
    refresh_token: String,

    /// Expiration timestamp (Unix seconds).
    expires_at: u64,

    /// Optional account ID.
    #[serde(default)]
    account_id: Option<String>,

    /// Optional enterprise URL.
    #[serde(default)]
    enterprise_url: Option<String>,
  },

  /// Bearer token authentication.
  #[serde(rename = "bearer")]
  Bearer { token: String },

  /// Device code for OAuth flow.
  #[serde(rename = "device_code")]
  DeviceCode {
    /// Device code.
    device_code: String,

    /// User code.
    user_code: String,

    /// Verification URL.
    verification_url: String,

    /// Expiration time.
    expires_in: u64,

    /// Polling interval.
    interval: u64,
  },
}

impl Credentials {
  /// Get the actual credential value for HTTP requests.
  pub fn get_value(&self) -> String {
    match self {
      Credentials::ApiKey { key } => key.clone(),
      Credentials::OAuth { access_token, .. } => access_token.clone(),
      Credentials::Bearer { token } => token.clone(),
      Credentials::DeviceCode { device_code, .. } => device_code.clone(),
    }
  }

  /// Check if credentials are expired (for OAuth).
  pub fn is_expired(&self) -> bool {
    match self {
      Credentials::OAuth { expires_at, .. } => *expires_at < chrono::Utc::now().timestamp() as u64,
      _ => false,
    }
  }

  /// Get the Authorization header value.
  pub fn get_auth_header(&self) -> String {
    match self {
      Credentials::ApiKey { key } => format!("Bearer {key}"),
      Credentials::OAuth { access_token, .. } => format!("Bearer {access_token}"),
      Credentials::Bearer { token } => format!("Bearer {token}"),
      Credentials::DeviceCode { device_code, .. } => format!("Bearer {device_code}"),
    }
  }
}

/// Stored credentials with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCredentials {
  /// Provider ID.
  pub provider_id: String,

  /// Actual credentials.
  pub credentials: Credentials,

  /// When these credentials were stored.
  pub stored_at: u64,

  /// Display name for the account.
  #[serde(default)]
  pub account_name: Option<String>,

  /// Optional metadata.
  #[serde(default)]
  pub metadata: serde_json::Value,
}

impl StoredCredentials {
  /// Create new stored credentials.
  pub fn new(provider_id: impl Into<String>, credentials: Credentials) -> Self {
    Self {
      provider_id: provider_id.into(),
      credentials,
      stored_at: chrono::Utc::now().timestamp() as u64,
      account_name: None,
      metadata: serde_json::json!({}),
    }
  }

  /// Set account name.
  pub fn with_account_name(mut self, name: impl Into<String>) -> Self {
    self.account_name = Some(name.into());
    self
  }

  /// Set metadata.
  pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
    self.metadata = metadata;
    self
  }
}

/// Credential storage trait.
#[async_trait::async_trait]
pub trait CredentialStorage: Send + Sync {
  /// Load credentials for a provider.
  async fn load(&self, provider_id: &str) -> Result<Option<StoredCredentials>>;

  /// Save credentials.
  async fn save(&self, credentials: StoredCredentials) -> Result<()>;

  /// Delete credentials.
  async fn delete(&self, provider_id: &str) -> Result<()>;

  /// List all stored provider IDs.
  async fn list(&self) -> Result<Vec<String>>;
}

/// File-based credential storage.
pub struct FileCredentialStorage {
  /// Path to the storage file.
  storage_path: PathBuf,
}

impl FileCredentialStorage {
  /// Create a new file storage.
  pub fn new(storage_path: impl AsRef<Path>) -> Self {
    Self {
      storage_path: storage_path.as_ref().to_path_buf(),
    }
  }

  /// Get the default Cokra auth storage path.
  pub fn default_path() -> Result<PathBuf> {
    let home = dirs::home_dir()
      .ok_or_else(|| AuthError::StorageError("No home directory found".to_string()))?;

    Ok(home.join(".cokra").join("auth.json"))
  }

  /// Create with default path.
  pub fn default_storage() -> Result<Self> {
    Ok(Self::new(Self::default_path()?))
  }

  /// Load the storage file.
  fn load_file(&self) -> Result<CredentialStore> {
    if !self.storage_path.exists() {
      return Ok(CredentialStore::default());
    }

    let content = std::fs::read_to_string(&self.storage_path)
      .map_err(|e| AuthError::StorageError(format!("Failed to read auth file: {e}")))?;

    if content.trim().is_empty() {
      return Ok(CredentialStore::default());
    }

    let store: CredentialStore = match serde_json::from_str(&content) {
      Ok(store) => store,
      Err(err) => {
        tracing::warn!(
          path = %self.storage_path.display(),
          error = %err,
          "auth storage file is invalid; treating it as empty"
        );
        CredentialStore::default()
      }
    };

    Ok(store)
  }

  /// Save the storage file.
  fn save_file(&self, store: &CredentialStore) -> Result<()> {
    if let Some(parent) = self.storage_path.parent() {
      std::fs::create_dir_all(parent)
        .map_err(|e| AuthError::StorageError(format!("Failed to create auth directory: {e}")))?;
    }

    let content = serde_json::to_string_pretty(store)
      .map_err(|e| AuthError::StorageError(format!("Failed to serialize auth: {e}")))?;

    std::fs::write(&self.storage_path, content)
      .map_err(|e| AuthError::StorageError(format!("Failed to write auth file: {e}")))?;

    Ok(())
  }
}

#[async_trait::async_trait]
impl CredentialStorage for FileCredentialStorage {
  async fn load(&self, provider_id: &str) -> Result<Option<StoredCredentials>> {
    let store = self.load_file()?;
    Ok(
      store
        .credentials
        .get(provider_id)
        .map(|data| StoredCredentials {
          provider_id: provider_id.to_string(),
          credentials: data.credentials.clone(),
          stored_at: data.stored_at,
          account_name: data.account_name.clone(),
          metadata: data.metadata.clone(),
        }),
    )
  }

  async fn save(&self, credentials: StoredCredentials) -> Result<()> {
    let mut store = self.load_file()?;
    let provider_id = credentials.provider_id.clone();
    store.credentials.insert(
      provider_id,
      StoredCredentialData {
        credentials: credentials.credentials,
        stored_at: credentials.stored_at,
        account_name: credentials.account_name,
        metadata: credentials.metadata,
      },
    );
    self.save_file(&store)
  }

  async fn delete(&self, provider_id: &str) -> Result<()> {
    let mut store = self.load_file()?;
    store.credentials.remove(provider_id);
    self.save_file(&store)
  }

  async fn list(&self) -> Result<Vec<String>> {
    let store = self.load_file()?;
    Ok(store.credentials.keys().cloned().collect())
  }
}

/// In-memory credential storage (for testing).
#[derive(Default)]
pub struct MemoryCredentialStorage {
  credentials: std::sync::Arc<std::sync::Mutex<HashMap<String, StoredCredentialData>>>,
}

impl MemoryCredentialStorage {
  /// Create a new memory storage.
  pub fn new() -> Self {
    Self::default()
  }
}

#[async_trait::async_trait]
impl CredentialStorage for MemoryCredentialStorage {
  async fn load(&self, provider_id: &str) -> Result<Option<StoredCredentials>> {
    Ok(
      self
        .credentials
        .lock()
        .unwrap()
        .get(provider_id)
        .map(|data| StoredCredentials {
          provider_id: provider_id.to_string(),
          credentials: data.credentials.clone(),
          stored_at: data.stored_at,
          account_name: data.account_name.clone(),
          metadata: data.metadata.clone(),
        }),
    )
  }

  async fn save(&self, credentials: StoredCredentials) -> Result<()> {
    let provider_id = credentials.provider_id.clone();
    self.credentials.lock().unwrap().insert(
      provider_id,
      StoredCredentialData {
        credentials: credentials.credentials,
        stored_at: credentials.stored_at,
        account_name: credentials.account_name,
        metadata: credentials.metadata,
      },
    );
    Ok(())
  }

  async fn delete(&self, provider_id: &str) -> Result<()> {
    self.credentials.lock().unwrap().remove(provider_id);
    Ok(())
  }

  async fn list(&self) -> Result<Vec<String>> {
    Ok(self.credentials.lock().unwrap().keys().cloned().collect())
  }
}

/// Internal credential store structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CredentialStore {
  credentials: HashMap<String, StoredCredentialData>,
  version: u32,
}

/// Stored credential data (simplified version for storage).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCredentialData {
  credentials: Credentials,
  stored_at: u64,
  account_name: Option<String>,
  metadata: serde_json::Value,
}

impl Default for CredentialStore {
  fn default() -> Self {
    Self {
      credentials: HashMap::new(),
      version: 1,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use pretty_assertions::assert_eq;

  #[tokio::test]
  async fn test_memory_storage() {
    let storage = MemoryCredentialStorage::new();
    let creds = StoredCredentials::new(
      "test",
      Credentials::ApiKey {
        key: "test-key".to_string(),
      },
    );

    storage.save(creds.clone()).await.unwrap();
    let loaded = storage.load("test").await.unwrap().unwrap();
    assert_eq!(loaded.credentials.get_value(), "test-key");

    storage.delete("test").await.unwrap();
    assert!(storage.load("test").await.unwrap().is_none());
  }

  #[test]
  fn test_credentials_expiry() {
    let creds = Credentials::OAuth {
      access_token: "test".to_string(),
      refresh_token: "refresh".to_string(),
      expires_at: 0,
      account_id: None,
      enterprise_url: None,
    };

    assert!(creds.is_expired());

    let creds = Credentials::ApiKey {
      key: "test".to_string(),
    };

    assert!(!creds.is_expired());
  }

  #[test]
  fn test_auth_header() {
    let creds = Credentials::ApiKey {
      key: "sk-test123".to_string(),
    };

    assert_eq!(creds.get_auth_header(), "Bearer sk-test123");
  }

  #[test]
  fn empty_auth_file_is_treated_as_empty_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("auth.json");
    std::fs::write(&path, "").expect("write empty file");

    let storage = FileCredentialStorage::new(&path);
    let store = storage.load_file().expect("load should succeed");
    assert!(store.credentials.is_empty());
  }
}
