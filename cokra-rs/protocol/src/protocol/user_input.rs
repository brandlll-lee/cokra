// User Input Types
// Types for user input content

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// User input content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserInput {
  /// Text input with optional formatting
  Text {
    text: String,
    text_elements: Vec<TextElement>,
  },
  /// Image from URL
  Image { image_url: String },
  /// Local image file
  LocalImage { path: PathBuf },
  /// Skill invocation
  Skill { name: String, path: PathBuf },
  /// @mention reference
  Mention { name: String, path: String },
}

/// Text element with formatting info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextElement {
  pub byte_range: ByteRange,
  pub placeholder: Option<String>,
}

/// Byte range for text formatting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ByteRange {
  pub start: usize,
  pub end: usize,
}

/// Selectable option for a request_user_input question.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestUserInputQuestionOption {
  pub label: String,
  pub description: String,
}

/// One question in a request_user_input prompt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestUserInputQuestion {
  pub id: String,
  pub header: String,
  pub question: String,
  #[serde(rename = "isOther", default)]
  pub is_other: bool,
  #[serde(rename = "isSecret", default)]
  pub is_secret: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub options: Option<Vec<RequestUserInputQuestionOption>>,
}

/// Tool arguments for request_user_input.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestUserInputArgs {
  pub questions: Vec<RequestUserInputQuestion>,
}

/// One answered question in a request_user_input response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestUserInputAnswer {
  pub answers: Vec<String>,
}

/// Response returned by request_user_input.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RequestUserInputResponse {
  pub answers: HashMap<String, RequestUserInputAnswer>,
}
