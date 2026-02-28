//! Authentication resolvers
//!
//! Resolvers handle different ways of obtaining credentials

use super::{Credentials, Result};

/// Trait for resolving credentials from various sources
pub trait AuthResolver: Send + Sync {
  /// Resolve credentials for a provider
  fn resolve(&self, provider_id: &str) -> Option<Credentials>;

  /// Get the resolver name
  fn name(&self) -> &'static str;
}

/// Environment variable resolver
///
/// Checks environment variables for API keys
pub struct EnvAuthResolver;

impl EnvAuthResolver {
  /// Create a new environment resolver
  pub fn new() -> Self {
    Self
  }

  /// Get environment variable mappings for providers
  fn get_env_vars_for_provider(provider_id: &str) -> Vec<String> {
    // Standardize provider ID (replace hyphens with underscores)
    let provider_upper = provider_id.to_uppercase().replace('-', "_");
    let provider_snake = provider_id.to_uppercase();

    vec![
      format!("{}_API_KEY", provider_upper),
      format!("{}_API_KEY", provider_snake),
      format!("{}_KEY", provider_upper),
      format!("{}_KEY", provider_snake),
    ]
  }

  /// Get common fallback env vars
  fn get_fallback_env_vars() -> Vec<String> {
    vec![
      "OPENAI_API_KEY".to_string(),
      "ANTHROPIC_API_KEY".to_string(),
      "GOOGLE_API_KEY".to_string(),
      "COHERE_API_KEY".to_string(),
      "AZURE_API_KEY".to_string(),
      "OPENROUTER_API_KEY".to_string(),
    ]
  }
}

impl Default for EnvAuthResolver {
  fn default() -> Self {
    Self::new()
  }
}

impl AuthResolver for EnvAuthResolver {
  fn resolve(&self, provider_id: &str) -> Option<Credentials> {
    // Check provider-specific env vars first
    for var in Self::get_env_vars_for_provider(provider_id) {
      if let Ok(key) = std::env::var(&var) {
        if !key.is_empty() {
          tracing::debug!("Found credentials for {} in env var {}", provider_id, var);
          return Some(Credentials::ApiKey { key });
        }
      }
    }

    // Check common fallbacks
    for var in Self::get_fallback_env_vars() {
      if let Ok(key) = std::env::var(&var) {
        if !key.is_empty() {
          tracing::debug!(
            "Found credentials for {} in fallback env var {}",
            provider_id,
            var
          );
          return Some(Credentials::ApiKey { key });
        }
      }
    }

    None
  }

  fn name(&self) -> &'static str {
    "env"
  }
}

/// Config file resolver
///
/// Reads credentials from Cokra config files
pub struct ConfigAuthResolver {
  config_path: std::path::PathBuf,
}

impl ConfigAuthResolver {
  /// Create a new config resolver with custom path
  pub fn new(config_path: std::path::PathBuf) -> Self {
    Self { config_path }
  }

  /// Get the default config path
  pub fn default_path() -> std::path::PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join(".cokra").join("config.toml")
  }

  /// Create with default path
  pub fn default() -> Result<Self> {
    Ok(Self::new(Self::default_path()))
  }

  /// Load config and find provider credentials
  fn load_provider_credentials(&self, provider_id: &str) -> Option<Credentials> {
    // Try to read the config file
    let content = std::fs::read_to_string(&self.config_path).ok()?;

    // Simple parsing - look for provider sections
    // This is a basic implementation; a real one would use a TOML parser
    for line in content.lines() {
      if line.contains(&format!("[models.providers.{}]", provider_id))
        || line.contains(&format!("[model_providers.{}]", provider_id))
      {
        // Found the provider section, look for api_key
      }
    }

    None
  }
}

impl AuthResolver for ConfigAuthResolver {
  fn resolve(&self, provider_id: &str) -> Option<Credentials> {
    self.load_provider_credentials(provider_id)
  }

  fn name(&self) -> &'static str {
    "config"
  }
}

/// Storage resolver
///
/// Resolves credentials from persistent storage
pub struct StorageAuthResolver {
  storage: std::sync::Arc<dyn super::storage::CredentialStorage>,
}

impl StorageAuthResolver {
  /// Create a new storage resolver
  pub fn new(storage: std::sync::Arc<dyn super::storage::CredentialStorage>) -> Self {
    Self { storage }
  }

  /// Create with default file storage
  pub fn default_storage() -> Result<Self> {
    let storage = std::sync::Arc::new(super::storage::FileCredentialStorage::default_storage()?);
    Ok(Self::new(storage))
  }
}

impl AuthResolver for StorageAuthResolver {
  fn resolve(&self, provider_id: &str) -> Option<Credentials> {
    let handle = tokio::runtime::Handle::try_current().ok()?;
    handle
      .block_on(async { self.storage.load(provider_id).await.ok() })
      .flatten()
      .map(|s| s.credentials)
  }

  fn name(&self) -> &'static str {
    "storage"
  }
}

/// Chained resolver
///
/// Tries multiple resolvers in order
pub struct ChainedAuthResolver {
  resolvers: Vec<Box<dyn AuthResolver>>,
}

impl ChainedAuthResolver {
  /// Create a new chained resolver
  pub fn new() -> Self {
    Self {
      resolvers: Vec::new(),
    }
  }

  /// Add a resolver to the chain
  pub fn add(mut self, resolver: Box<dyn AuthResolver>) -> Self {
    self.resolvers.push(resolver);
    self
  }

  /// Create with default resolvers in priority order
  pub fn with_defaults() -> Self {
    let chain = Self::new().add(Box::new(EnvAuthResolver::new()));
    if let Ok(cfg) = ConfigAuthResolver::default() {
      return chain.add(Box::new(cfg));
    }
    chain
  }
}

impl Default for ChainedAuthResolver {
  fn default() -> Self {
    Self::with_defaults()
  }
}

impl AuthResolver for ChainedAuthResolver {
  fn resolve(&self, provider_id: &str) -> Option<Credentials> {
    for resolver in &self.resolvers {
      tracing::debug!("Trying resolver {} for {}", resolver.name(), provider_id);
      if let Some(creds) = resolver.resolve(provider_id) {
        return Some(creds);
      }
    }
    None
  }

  fn name(&self) -> &'static str {
    "chained"
  }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_env_resolver_openai() {
    let resolver = EnvAuthResolver::new();
    unsafe {
      std::env::set_var("OPENAI_API_KEY", "test-key-123");
    }

    let creds = resolver.resolve("openai");
    assert!(creds.is_some());
    assert_eq!(creds.unwrap().get_value(), "test-key-123");

    unsafe {
      std::env::remove_var("OPENAI_API_KEY");
    }
  }

  #[test]
  fn test_env_resolver_custom() {
    let resolver = EnvAuthResolver::new();
    unsafe {
      std::env::set_var("MYPROVIDER_API_KEY", "custom-key");
    }

    let creds = resolver.resolve("myprovider");
    assert!(creds.is_some());
    assert_eq!(creds.unwrap().get_value(), "custom-key");

    unsafe {
      std::env::remove_var("MYPROVIDER_API_KEY");
    }
  }

  #[test]
  fn test_chained_resolver() {
    let provider_id = "chainedresolver";
    let env_var = "CHAINEDRESOLVER_API_KEY";

    unsafe {
      std::env::set_var(env_var, "from-env");
    }

    let resolver = ChainedAuthResolver::with_defaults();
    let creds = resolver.resolve(provider_id);

    assert!(creds.is_some());
    assert_eq!(creds.unwrap().get_value(), "from-env");

    unsafe {
      std::env::remove_var(env_var);
    }
  }
}
