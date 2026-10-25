//! `ai/` —— AI 会话、执行与提示词。
//!
//! 一次 Run 的主循环与生命周期、会话/消息/提问、事件总线、记忆、Skills、系统提示（含 NSFW 那两段）。

pub mod ai;
pub mod ai_settings;
pub mod conversations;
pub mod events;
pub mod executor;
pub mod memory;
pub mod nsfw_prompt;
pub mod pending;
pub mod questions;
pub mod reference_attach;
pub mod run_kit;
pub mod run_lifecycle;
pub mod runs;
pub mod skills;
pub mod step_mark;
pub mod system_prompt;
pub mod title_strip;
