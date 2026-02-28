use serde::{Deserialize, Serialize};

/// Agent role definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRole {
  pub name: String,
  pub description: String,
  pub system_prompt: String,
  pub allowed_tools: Vec<String>,
  pub temperature: Option<f32>,
  pub max_tokens: Option<u32>,
}

pub const ROLE_CODING: &str = "coding";
pub const ROLE_PLANNING: &str = "planning";
pub const ROLE_REVIEW: &str = "review";

impl AgentRole {
  pub fn coding() -> Self {
    Self {
      name: ROLE_CODING.to_string(),
      description: "General coding role with tool access".to_string(),
      system_prompt: "You are a pragmatic coding agent.".to_string(),
      allowed_tools: Vec::new(),
      temperature: Some(0.2),
      max_tokens: Some(4096),
    }
  }
}
