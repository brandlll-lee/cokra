use super::auth::AuthProviderDescriptor;
use super::auth::auth_provider_descriptors;
use super::auth::find_auth_provider;
use super::oauth_connect::OAuthProviderKind;
use super::provider::ProviderConnectMethod;

pub use super::auth::RuntimeRegistrationKind;

#[derive(Debug, Clone)]
pub struct ConnectProviderCatalogEntry {
  pub id: &'static str,
  pub name: &'static str,
  pub connect_method: ProviderConnectMethod,
  pub env_vars: Vec<String>,
  pub default_models: Vec<String>,
  pub runtime_registration: RuntimeRegistrationKind,
  pub runtime_provider_id: Option<&'static str>,
  pub oauth_provider: Option<OAuthProviderKind>,
}

impl ConnectProviderCatalogEntry {
  fn from_descriptor(descriptor: &AuthProviderDescriptor) -> Self {
    Self {
      id: descriptor.id,
      name: descriptor.name,
      connect_method: descriptor.connect_method.clone(),
      // Tradeoff: keep the catalog's owned Vec fields for compatibility with the
      // existing TUI/runtime callers while provider auth metadata lives in a static registry.
      env_vars: descriptor.env_vars(),
      default_models: descriptor.default_models(),
      runtime_registration: descriptor.runtime_registration,
      runtime_provider_id: descriptor.runtime_provider_id,
      oauth_provider: descriptor.oauth_provider,
    }
  }

  pub fn primary_model_provider_id(&self) -> Option<String> {
    if let Some(runtime_provider_id) = self.runtime_provider_id {
      return Some(runtime_provider_id.to_string());
    }
    self
      .default_models
      .first()
      .and_then(|model| model.split('/').next())
      .map(ToString::to_string)
  }

  pub fn supports_model_runtime(&self) -> bool {
    self.runtime_registration != RuntimeRegistrationKind::None
  }
}

pub fn connect_provider_catalog() -> Vec<ConnectProviderCatalogEntry> {
  auth_provider_descriptors()
    .iter()
    .filter(|descriptor| descriptor.visible_in_connect_catalog)
    .map(ConnectProviderCatalogEntry::from_descriptor)
    .collect()
}

pub fn find_connect_provider(provider_id: &str) -> Option<ConnectProviderCatalogEntry> {
  let descriptor = find_auth_provider(provider_id)?;
  descriptor
    .visible_in_connect_catalog
    .then(|| ConnectProviderCatalogEntry::from_descriptor(descriptor))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn hidden_auth_descriptors_are_not_connect_catalog_entries() {
    assert!(find_connect_provider("github").is_none());
    assert!(find_connect_provider("github-copilot-enterprise").is_none());
  }
}
