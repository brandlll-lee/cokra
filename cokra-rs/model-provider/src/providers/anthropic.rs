// Anthropic Provider

use async_trait::async_trait;

use crate::provider::{ModelProvider, LanguageModel, Credentials, ProviderError};
use crate::types::{GenerateRequest, GenerateResponse, Message, ChatOptions, ChatResponse, ModelInfo, ModelCapabilities};

/// Anthropic provider
pub struct AnthropicProvider {
    api_key: Option<String>,
    base_url: String,
    models: Vec<ModelInfo>,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        Self {
            api_key: None,
            base_url: "https://api.anthropic.com/v1".to_string(),
            models: Self::builtin_models(),
        }
    }

    fn builtin_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "claude-sonnet-4".to_string(),
                provider_id: "anthropic".to_string(),
                name: "Claude Sonnet 4".to_string(),
                capabilities: ModelCapabilities {
                    temperature: true,
                    reasoning: true,
                    attachment: true,
                    tool_call: true,
                    input: crate::types::InputModalities {
                        text: true,
                        image: true,
                        audio: false,
                        video: false,
                        pdf: false,
                    },
                    output: crate::types::OutputModalities {
                        text: true,
                        image: false,
                        audio: false,
                        video: false,
                    },
                    interleaved: None,
                },
            },
            ModelInfo {
                id: "claude-3-5-sonnet".to_string(),
                provider_id: "anthropic".to_string(),
                name: "Claude 3.5 Sonnet".to_string(),
                capabilities: ModelCapabilities {
                    temperature: true,
                    reasoning: false,
                    attachment: true,
                    tool_call: true,
                    input: crate::types::InputModalities {
                        text: true,
                        image: true,
                        audio: false,
                        video: false,
                        pdf: false,
                    },
                    output: crate::types::OutputModalities {
                        text: true,
                        image: false,
                        audio: false,
                        video: false,
                    },
                    interleaved: None,
                },
            },
            ModelInfo {
                id: "claude-3-opus".to_string(),
                provider_id: "anthropic".to_string(),
                name: "Claude 3 Opus".to_string(),
                capabilities: ModelCapabilities {
                    temperature: true,
                    reasoning: false,
                    attachment: true,
                    tool_call: true,
                    input: crate::types::InputModalities {
                        text: true,
                        image: true,
                        audio: false,
                        video: false,
                        pdf: false,
                    },
                    output: crate::types::OutputModalities {
                        text: true,
                        image: false,
                        audio: false,
                        video: false,
                    },
                    interleaved: None,
                },
            },
        ]
    }

    fn get_api_key(&self) -> Result<String, ProviderError> {
        self.api_key.clone()
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
            .ok_or_else(|| ProviderError::AuthenticationRequired("anthropic".to_string()))
    }
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn id(&self) -> &str { "anthropic" }
    fn name(&self) -> &str { "Anthropic" }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(self.models.clone())
    }

    fn get_model(&self, model_id: &str) -> anyhow::Result<Box<dyn LanguageModel>> {
        let model_info = self.models.iter()
            .find(|m| m.id == model_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        Ok(Box::new(AnthropicModel {
            model_id: model_id.to_string(),
            model_info,
            api_key: self.get_api_key()?,
            base_url: self.base_url.clone(),
        }))
    }

    async fn is_authenticated(&self) -> bool {
        self.api_key.is_some() || std::env::var("ANTHROPIC_API_KEY").is_ok()
    }

    async fn authenticate(&mut self, credentials: Credentials) -> anyhow::Result<()> {
        match credentials {
            Credentials::ApiKey { key } => {
                self.api_key = Some(key);
                Ok(())
            }
            _ => anyhow::bail!("Anthropic only supports API key authentication"),
        }
    }
}

/// Anthropic language model
pub struct AnthropicModel {
    model_id: String,
    model_info: ModelInfo,
    api_key: String,
    base_url: String,
}

#[async_trait]
impl LanguageModel for AnthropicModel {
    fn id(&self) -> &str { &self.model_id }

    fn capabilities(&self) -> &ModelCapabilities { &self.model_info.capabilities }

    async fn generate(&self, _request: GenerateRequest) -> anyhow::Result<GenerateResponse> {
        Ok(GenerateResponse {
            content: "Anthropic response".to_string(),
            tool_calls: vec![],
            finish_reason: crate::types::FinishReason::Stop,
            usage: crate::types::Usage::new(),
        })
    }

    async fn generate_stream(
        &self,
        _request: GenerateRequest,
    ) -> anyhow::Result<std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<crate::streaming::ProviderChunk>> + Send>>> {
        use futures::stream;
        Ok(Box::pin(stream::empty()))
    }

    async fn chat(&self, _messages: Vec<Message>, _options: ChatOptions) -> anyhow::Result<ChatResponse> {
        Ok(ChatResponse {
            message: Message {
                role: crate::types::MessageRole::Assistant,
                content: vec![crate::types::ContentPart::Text {
                    text: "Anthropic response".to_string(),
                }],
            },
            tool_calls: vec![],
            finish_reason: crate::types::FinishReason::Stop,
            usage: crate::types::Usage::new(),
        })
    }

    async fn chat_stream(
        &self,
        _messages: Vec<Message>,
        _options: ChatOptions,
    ) -> anyhow::Result<std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<crate::provider::ChatChunk>> + Send>>> {
        use futures::stream;
        Ok(Box::pin(stream::empty()))
    }
}
