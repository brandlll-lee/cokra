//! Credential storage
//!
//! Handles persistent storage of credentials

use super::{AuthError, Credentials, Result, StoredCredentials};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Credential storage trait
#[async_trait::async_trait]
pub trait CredentialStorage: Send + Sync {
  /// Load credentials for a provider
  async fn load(&self, provider_id: &str) -> Result<Option<StoredCredentials>>;

  /// Save credentials
  async fn save(&self, credentials: StoredCredentials) -> Result<()>;

  /// Delete credentials
  async fn delete(&self, provider_id: &str) -> Result<()>;

  /// List all stored provider IDs
  async fn list(&self) -> Result<Vec<String>>;
}

/// File-based credential storage
pub struct FileCredentialStorage {
  /// Path to the storage file
  storage_path: PathBuf,
}

impl FileCredentialStorage {
  /// Create a new file storage
  pub fn new(storage_path: impl AsRef<Path>) -> Self {
    Self {
      storage_path: storage_path.as_ref().to_path_buf(),
    }
  }

  /// Get the default Cokra auth storage path
  pub fn default_path() -> Result<PathBuf> {
    let home = dirs::home_dir()
      .ok_or_else(|| AuthError::StorageError("No home directory found".to_string()))?;

    Ok(home.join(".cokra").join("auth.json"))
  }

  /// Create with default path
  pub fn default_storage() -> Result<Self> {
    Ok(Self::new(Self::default_path()?))
  }

  /// Load the storage file
  fn load_file(&self) -> Result<CredentialStore> {
    if !self.storage_path.exists() {
      return Ok(CredentialStore::default());
    }

    let content = std::fs::read_to_string(&self.storage_path)
      .map_err(|e| AuthError::StorageError(format!("Failed to read auth file: {}", e)))?;

    let store: CredentialStore = serde_json::from_str(&content)
      .map_err(|e| AuthError::StorageError(format!("Failed to parse auth file: {}", e)))?;

    Ok(store)
  }

  /// Save the storage file
  fn save_file(&self, store: &CredentialStore) -> Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = self.storage_path.parent() {
      std::fs::create_dir_all(parent)
        .map_err(|e| AuthError::StorageError(format!("Failed to create auth directory: {}", e)))?;
    }

    let content = serde_json::to_string_pretty(store)
      .map_err(|e| AuthError::StorageError(format!("Failed to serialize auth: {}", e)))?;

    std::fs::write(&self.storage_path, content)
      .map_err(|e| AuthError::StorageError(format!("Failed to write auth file: {}", e)))?;

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
      provider_id.clone(),
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

/// In-memory credential storage (for testing)
#[derive(Default)]
pub struct MemoryCredentialStorage {
  credentials: std::sync::Arc<std::sync::Mutex<HashMap<String, StoredCredentialData>>>,
}

impl MemoryCredentialStorage {
  /// Create a new memory storage
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

/// Internal credential store structure
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CredentialStore {
  credentials: HashMap<String, StoredCredentialData>,
  version: u32,
}

/// Stored credential data (simplified version for storage)
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

  #[tokio::test]
  async fn test_memory_storage() {
    let storage = MemoryCredentialStorage::new();
    let creds = StoredCredentials::new(
      "test",
      Credentials::ApiKey {
        key: "test-key".to_string(),
      },
    );

    tokio::runtime::Runtime::new().unwrap().block_on(async {
      storage.save(creds.clone()).await.unwrap();
      let loaded = storage.load("test").await.unwrap().unwrap();
      assert_eq!(loaded.credentials.get_value(), "test-key");

      storage.delete("test").await.unwrap();
      assert!(storage.load("test").await.unwrap().is_none());
    });
  }

  #[test]
  fn test_credentials_expiry() {
    let creds = Credentials::OAuth {
      access_token: "test".to_string(),
      refresh_token: "refresh".to_string(),
      expires_at: 0, // Expired
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
}
