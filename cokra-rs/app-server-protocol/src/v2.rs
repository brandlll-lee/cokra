// Cokra App Server Protocol V2
// Current API version definitions

use serde::{Deserialize, Serialize};

/// Thread start request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadStartParams {
  pub query: String,
}

/// Thread start response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadStartResponse {
  pub thread_id: String,
}
