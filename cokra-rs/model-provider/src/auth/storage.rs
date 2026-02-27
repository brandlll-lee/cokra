// Auth Storage
// Persistent storage for authentication

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::auth::AuthInfo;

/// Authentication storage trait
pub trait AuthStorage: Send + Sync {
    /// Get auth info for provider
    fn get(&self, provider_id: &str) -> anyhow::Result<Option<AuthInfo>>;

    /// Set auth info for provider
    fn set(&self, provider_id: &str, auth: AuthInfo) -> anyhow::Result<()>;

    /// Delete auth info for provider
    fn delete(&self, provider_id: &str) -> anyhow::Result<()>;

    /// List all stored providers
    fn list(&self) -> anyhow::Result<Vec<String>>;
}

/// In-memory auth storage
pub struct MemoryAuthStorage {
    data: Mutex<HashMap<String, AuthInfo>>,
}

impl MemoryAuthStorage {
    pub fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for MemoryAuthStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthStorage for MemoryAuthStorage {
    fn get(&self, provider_id: &str) -> anyhow::Result<Option<AuthInfo>> {
        let data = self.data.lock().unwrap();
        Ok(data.get(provider_id).cloned())
    }

    fn set(&self, provider_id: &str, auth: AuthInfo) -> anyhow::Result<()> {
        let mut data = self.data.lock().unwrap();
        data.insert(provider_id.to_string(), auth);
        Ok(())
    }

    fn delete(&self, provider_id: &str) -> anyhow::Result<()> {
        let mut data = self.data.lock().unwrap();
        data.remove(provider_id);
        Ok(())
    }

    fn list(&self) -> anyhow::Result<Vec<String>> {
        let data = self.data.lock().unwrap();
        Ok(data.keys().cloned().collect())
    }
}

/// File-based auth storage
pub struct FileAuthStorage {
    path: PathBuf,
}

impl FileAuthStorage {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn load(&self) -> anyhow::Result<HashMap<String, AuthInfo>> {
        if !self.path.exists() {
            return Ok(HashMap::new());
        }

        let content = std::fs::read_to_string(&self.path)?;
        let data: HashMap<String, AuthInfo> = toml::from_str(&content)?;
        Ok(data)
    }

    fn save(&self, data: &HashMap<String, AuthInfo>) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(data)?;
        std::fs::write(&self.path, content)?;
        Ok(())
    }
}

impl AuthStorage for FileAuthStorage {
    fn get(&self, provider_id: &str) -> anyhow::Result<Option<AuthInfo>> {
        let data = self.load()?;
        Ok(data.get(provider_id).cloned())
    }

    fn set(&self, provider_id: &str, auth: AuthInfo) -> anyhow::Result<()> {
        let mut data = self.load()?;
        data.insert(provider_id.to_string(), auth);
        self.save(&data)
    }

    fn delete(&self, provider_id: &str) -> anyhow::Result<()> {
        let mut data = self.load()?;
        data.remove(provider_id);
        self.save(&data)
    }

    fn list(&self) -> anyhow::Result<Vec<String>> {
        let data = self.load()?;
        Ok(data.keys().cloned().collect())
    }
}
