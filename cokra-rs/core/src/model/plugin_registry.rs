use super::connect_catalog::ConnectProviderCatalogEntry;
use super::connect_catalog::connect_provider_catalog;
use super::connect_catalog::find_connect_provider;

pub struct PluginRegistry;

impl PluginRegistry {
  pub fn entries() -> Vec<ConnectProviderCatalogEntry> {
    connect_provider_catalog()
  }

  pub fn find(provider_id: &str) -> Option<ConnectProviderCatalogEntry> {
    find_connect_provider(provider_id)
  }
}
