//! Dynamic external integrations.
//!
//! Cokra keeps the kernel small and lets MCP, CLI, and API integrations load
//! at runtime through manifests. After loading, every integration projects into
//! the same tool runtime surface.

pub mod bootstrap;
pub mod loader;
pub mod manifest;
pub mod projector;
pub mod providers;

pub use bootstrap::IntegrationBootstrapStatus;
pub use loader::IntegrationCatalog;
pub use loader::LoadedIntegrationManifest;
pub use loader::discover_integrations;
pub use manifest::IntegrationKind;
pub use manifest::IntegrationManifest;
pub use projector::ProjectedIntegrations;
pub use projector::project_integrations;
pub use projector::projected_tool_definitions;
