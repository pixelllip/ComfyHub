//! ComfyHub 桌面服务端（Rust 版）的库入口。
//!
//! 二进制（`main.rs`）只是"读参数 → 开库 → 起服务"的壳；
//! 真正的实现都在库里，**这样集成测试能直接 `use comfyhub_server_rs::...`**
//! （只写 bin 的 crate 没法被 `tests/` 引用）。
//!
//! 模块划分与 Kotlin 侧的对应关系：
//!
//! | 本模块 | Kotlin 侧 |
//! | --- | --- |
//! | [`db`] | `Db.kt`（PRAGMA 自检 / `BEGIN IMMEDIATE` / `user_version`） |
//! | [`models`] | `Models.kt` 的 DTO（JSON 形状逐字一致） |
//! | [`routes`] | `TagRoutes.kt` / `PromptRoutes.kt` / `…Repo` 的读端点 |

pub mod ai;
pub mod auth;
pub mod capture;
pub mod comfy_dirs;
pub mod conversations;
pub mod db;
pub mod events;
pub mod executor;
pub mod inputs;
pub mod iso_time;
pub mod media;
pub mod media_upload;
pub mod media_write;
pub mod memory;
pub mod mobile_tools;
pub mod models;
pub mod nsfw_prompt;
pub mod pending;
pub mod policy;
pub mod repo;
pub mod routes;
pub mod run_kit;
pub mod run_lifecycle;
pub mod runs;
pub mod skills;
pub mod sync;
pub mod system_prompt;
pub mod thumbs;
pub mod write_routes;
