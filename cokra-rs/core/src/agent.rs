// Cokra Agent Module
// Agent system for multi-agent coordination

pub mod control;
pub mod role;
pub mod status;
pub mod guards;

pub use control::AgentControl;
pub use role::{AgentRole, AgentRoleConfig};
pub use status::AgentStatus;
pub use guards::{Guards, SpawnReservation};

/// Maximum agent spawn depth
pub const MAX_THREAD_SPAWN_DEPTH: i32 = 1;
