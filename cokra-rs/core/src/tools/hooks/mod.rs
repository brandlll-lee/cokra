//! 工具 Hooks 系统 — before/after 工具调用生命周期钩子。
//!
//! 1:1 复刻 codex `hooks` crate 设计，适配 cokra Rust 架构。
//!
//! ## 事件类型
//! - `BeforeToolCall`  — 在工具执行前触发，可阻断执行
//! - `AfterToolCall`   — 在工具执行后触发（成功或失败）
//! - `AfterTurn`       — 在一次完整 Turn 结束后触发
//!
//! ## Hook 实现类型
//! - `CommandHook` — 运行外部命令，传入 JSON payload
//! - `RuntimeHook` — Rust closure，零开销，适合内置指标采集
//!
//! ## Hook 结果
//! - `Success`           — 继续执行
//! - `FailedContinue`    — 记录错误但继续后续 hooks
//! - `FailedAbort`       — 停止整条 hook 链，中断工具调用

pub mod config;
pub mod registry;
pub mod runner;
pub mod types;
