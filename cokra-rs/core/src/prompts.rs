//! System prompt constants for the Cokra agent.
//!
//! Mirrors the architecture used by Codex (`include_str!("../../prompt.md")`)
//! and OpenCode (`import PROMPT_CODEX from "./prompt/codex_header.txt"`):
//! prompt text lives in standalone `.md` files under `src/prompts/` and is
//! embedded at compile time via `include_str!`.  Editing a prompt no longer
//! requires touching Rust source code or dealing with escaped string literals.
//!
//! # File layout
//! ```text
//! core/src/prompts/
//!   base.md          — main agent identity, personality, planning, task
//!                      execution, and tool guidelines (loaded by all roles)
//!   agent_leader.md  — appended when the agent runs as an orchestrator
//!                      (config.agents.max_threads > 1)
//!   agent_spawned.md — injected as a suffix when a spawned teammate is
//!                      created by build_spawned_agent_system_prompt()
//! ```

/// Base system prompt shared by every agent role.
pub const BASE: &str = include_str!("prompts/base.md");

/// Suffix appended to the leader / orchestrator agent's system prompt.
pub const AGENT_LEADER_SUFFIX: &str = include_str!("prompts/agent_leader.md");

/// Suffix appended to spawned teammate agents' system prompts.
pub const AGENT_SPAWNED_SUFFIX: &str = include_str!("prompts/agent_spawned.md");
