// LM Studio Provider (Local)

use async_trait::async_trait;

use crate::provider::{ModelProvider, LanguageModel, Credentials};
use crate::types::{GenerateRequest, GenerateResponse, Message, ChatOptions, ChatResponse, ModelInfo, ModelCapabilities};

/// LM Studio provider (local)
pub struct LMStudioProvider {
    base_url: String,
    models: Vec<ModelInfo>,
}

impl LMStudioProvider {
    pub fn new() -> Self {
        Self {
            base_url: "http://localhost:1234/v1".to_string(),
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
                id: "local-model".to_string(),
                provider_id: "lmstudio".to_string(),
                name: "Local Model".to_string(),
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

impl Default for LMStudioProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for LMStudioProvider {
    fn id(&self) -> &str { "lmstudio" }
    fn name(&self) -> &str { "LM Studio (Local)" }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(self.models.clone())
    }

    fn get_model(&self, model_id: &str) -> anyhow::Result<Box<dyn LanguageModel>> {
        let model_info = self.models.iter()
            .find(|m| m.id == model_id)
            .cloned()
            .unwrap_or_else(|| ModelInfo {
                id: model_id.to_string(),
                provider_id: "lmstudio".to_string(),
                name: model_id.to_string(),
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
            });

        Ok(Box::new(LMStudioModel {
            model_id: model_id.to_string(),
            model_info,
            base_url: self.base_url.clone(),
        }))
    }

    async fn is_authenticated(&self) -> bool { true }

    async fn authenticate(&mut self, _credentials: Credentials) -> anyhow::Result<()> { Ok(()) }
}

/// LM Studio model
pub struct LMStudioModel {
    model_id: String,
    model_info: ModelInfo,
    base_url: String,
}

#[async_trait]
impl LanguageModel for LMStudioModel {
    fn id(&self) -> &str { &self.model_id }
    fn capabilities(&self) -> &ModelCapabilities { &self.model_info.capabilities }

    async fn generate(&self, _request: GenerateRequest) -> anyhow::Result<GenerateResponse> {
        Ok(GenerateResponse {
            content: "LM Studio response".to_string(),
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
                    text: "LM Studio response".to_string(),
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
