// Event Module
pub mod broadcaster;

pub use broadcaster::{Event, EventBroadcaster};

use cokra_protocol::EventMsg;

/// Create an event from a message
pub fn create_event(id: String, msg: EventMsg) -> Event {
    Event { id, msg }
}

/// Event ID generator
pub fn generate_event_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
