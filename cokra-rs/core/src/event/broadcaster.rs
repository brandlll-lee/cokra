// Event Broadcaster
use cokra_protocol::EventMsg;

/// Event structure
#[derive(Debug, Clone)]
pub struct Event {
    /// Event ID (submission ID)
    pub id: String,
    /// Event message
    pub msg: EventMsg,
}

impl Event {
    /// Create a new event
    pub fn new(id: String, msg: EventMsg) -> Self {
        Self { id, msg }
    }
}

/// Event broadcaster configuration
pub struct EventBroadcasterConfig {
    /// Channel capacity
    pub capacity: usize,
}

impl Default for EventBroadcasterConfig {
    fn default() -> Self {
        Self {
            capacity: 256,
        }
    }
}
