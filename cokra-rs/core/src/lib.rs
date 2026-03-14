#![allow(dead_code)]
// Rust 1.93.1 currently ICEs in dead_code analysis on this crate after the
// kernel-facing export surface is tightened. Keep this narrow lint guard until
// the toolchain bug is no longer present.

//! Cokra core is intentionally small.
//!
//! The public kernel is built around:
//! - [`Cokra`] as the runtime entry point
//! - [`integrations`] as the dynamic MCP/CLI/API integration loader
//! - [`tools`] as the tool kernel
//! - [`tool_runtime`] as the unified runtime tool model
//! - [`mcp`] as the dynamic MCP bridge
//! - [`skills`] as the prompt-surface loader and injector
//!
//! Everything else is crate-internal plumbing that supports the kernel.

// Temporary lint guard for Rust 1.93.1 dead_code ICEs on crate-internal modules.
#[allow(dead_code)]
pub(crate) mod agent;
pub mod cokra;
pub(crate) mod config;
pub(crate) mod exec;
pub(crate) mod exec_policy;
pub mod integrations;
pub mod mcp;
#[allow(dead_code)]
pub mod model;
pub(crate) mod prompts;
pub(crate) mod sandbox_manager;
pub(crate) mod session;
pub(crate) mod shell;
pub mod skills;
pub mod tool_runtime;
pub(crate) mod thread_manager;
pub mod tools;
pub(crate) mod truncate;
#[allow(dead_code)]
pub(crate) mod turn;

pub use cokra::Cokra;
pub use cokra::CokraSpawnOk;
pub use cokra::StreamEvent;
pub use cokra::TurnResult;
pub use session::Session;
pub use turn::TurnConfig;
pub use turn::TurnExecutor;
pub use turn::UserInput;
