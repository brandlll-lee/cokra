// Ollama Provider (Local)

use async_trait::async_trait;

use crate::provider::{ModelProvider, LanguageModel, Credentials, ModelNotFoundError};
use crate::types::{GenerateRequest, GenerateResponse, Message, ChatOptions, ChatResponse, ModelInfo, ModelCapabilities};

/// Ollama provider (local)
pub struct OllamaProvider {
    base_url: String,
    models: Vec<ModelInfo>,
}

impl OllamaProvider {
    pub fn new() -> Self {
        Self {
            base_url: "http://localhost:11434".to_string(),
            models: Self::default_models(),
        }
    }

    pub fn with_base_url(base_url: String) -> Self {
        Self {
            base_url,
            models: Self::default_models(),
        }
    }

    fn default_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "llama3".to_string(),
                provider_id: "ollama".to_string(),
                name: "Llama 3".to_string(),
                capabilities: ModelCapabilities {
                    temperature: true,
                    reasoning: false,
                    attachment: false,
                    tool_call: true,
                    input: crate::types::InputModalities {
                        text: true,
                        image: false,
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
                id: "codellama".to_string(),
                provider_id: "ollama".to_string(),
                name: "Code Llama".to_string(),
                capabilities: ModelCapabilities {
                    temperature: true,
                    reasoning: false,
                    attachment: false,
                    tool_call: false,
                    input: crate::types::InputModalities {
                        text: true,
                        image: false,
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
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for OllamaProvider {
    fn id(&self) -> &str { "ollama" }
    fn name(&self) -> &str { "Ollama (Local)" }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        // In production, would query /api/tags endpoint
        Ok(self.models.clone())
    }

    fn get_model(&self, model_id: &str) -> anyhow::Result<Box<dyn LanguageModel>> {
        let model_info = self.models.iter()
            .find(|m| m.id == model_id)
            .cloned()
            .ok_or_else(|| ModelNotFoundError {
                provider_id: "ollama".to_string(),
                model_id: model_id.to_string(),
                suggestions: self.models.iter().map(|m| m.id.clone()).collect(),
            })?;

        Ok(Box::new(OllamaModel {
            model_id: model_id.to_string(),
            model_info,
            base_url: self.base_url.clone(),
        }))
    }

    async fn is_authenticated(&self) -> bool {
        // Ollama doesn't require authentication
        true
    }

    async fn authenticate(&mut self, _credentials: Credentials) -> anyhow::Result<()> {
        // Ollama doesn't require authentication
        Ok(())
    }
}

/// Ollama language model
pub struct OllamaModel {
    model_id: String,
    model_info: ModelInfo,
    base_url: String,
}

#[async_trait]
impl LanguageModel for OllamaModel {
    fn id(&self) -> &str { &self.model_id }
    fn capabilities(&self) -> &ModelCapabilities { &self.model_info.capabilities }

    async fn generate(&self, _request: GenerateRequest) -> anyhow::Result<GenerateResponse> {
        Ok(GenerateResponse {
            content: "Ollama response".to_string(),
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
                    text: "Ollama response".to_string(),
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
