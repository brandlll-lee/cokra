pub mod control;
pub mod role;
pub mod status;

pub use control::{AgentControl, Turn};
pub use role::{AgentRole, ROLE_CODING, ROLE_PLANNING, ROLE_REVIEW};
pub use status::AgentStatus;

pub const MAX_THREAD_SPAWN_DEPTH: usize = 5;
