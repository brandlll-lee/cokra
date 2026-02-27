// Cokra Model Provider
// Multi-model provider support for 20+ LLM providers
// Inspired by opencode provider system

pub mod provider;
pub mod registry;
pub mod router;
pub mod types;
pub mod providers;
pub mod auth;
pub mod streaming;

pub use provider::{ModelProvider, LanguageModel, ChatModel};
pub use registry::ModelRegistry;
pub use router::ModelRouter;
pub use types::{Model, ModelInfo, ProviderInfo, ModelCapabilities};
pub use streaming::{StreamPart, StreamTransform};

/// Default model if none specified
pub const DEFAULT_MODEL: &str = "openai/gpt-4o";

/// Default provider if none specified
pub const DEFAULT_PROVIDER: &str = "openai";
