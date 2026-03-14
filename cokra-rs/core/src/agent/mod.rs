//! Internal agent and team coordination state.
//!
//! The only collaboration state center is [`team_runtime`]. Other modules here
//! support turn execution and guardrails, but are not part of the public kernel.

pub(crate) mod control;
pub(crate) mod guards;
pub(crate) mod role;
pub(crate) mod status;
pub(crate) mod team_runtime;
pub(crate) mod team_state;

pub use control::AgentControl;
pub use control::Turn;
pub use guards::Guards;
pub use status::AgentStatus;
