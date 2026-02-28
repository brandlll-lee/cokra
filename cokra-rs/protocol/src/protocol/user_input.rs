// User Input Types
// Types for user input content

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

/// Request user input response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestUserInputResponse {
  pub response: String,
}
