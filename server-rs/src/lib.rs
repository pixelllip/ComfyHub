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
pub mod ai_settings;
pub mod api_port_file;
pub mod attachments;
pub mod auth;
pub mod builtin;
pub mod capability_catalog;
pub mod capability_probe;
pub mod capability_refresh;
pub mod capability_rules;
pub mod capture;
pub mod capture_import;
pub mod capture_ingest;
pub mod capture_poller;
pub mod character_cards;
pub mod civitai;
pub mod comfy_client;
pub mod comfy_dirs;
pub mod comfy_models;
pub mod comfy_progress;
pub mod conversations;
pub mod credentials;
pub mod danbooru;
pub mod db;
pub mod desktop_tools;
pub mod endpoint_guard;
pub mod events;
pub mod file_kind;
pub mod executor;
pub mod find_model;
pub mod graph_parse;
pub mod image_edit;
pub mod inputs;
pub mod iso_time;
pub mod mcp;
pub mod media;
pub mod media_upload;
pub mod media_write;
pub mod memory;
pub mod mobile_tools;
pub mod models;
pub mod net_urls;
pub mod nsfw_prompt;
pub mod outbound;
pub mod pending;
pub mod png_text;
pub mod policy;
pub mod poster;
pub mod protocol;
pub mod provider_probe;
pub mod questions;
pub mod repo;
pub mod routes;
pub mod run_kit;
pub mod run_lifecycle;
pub mod runs;
pub mod skills;
pub mod strict_intake;
pub mod submit_queue;
pub mod sync;
pub mod system_prompt;
pub mod thumbs;
pub mod title_strip;
pub mod tools;
pub mod upstream;
pub mod web_tools;
pub mod workflow_audit;
pub mod workflow_convert;
pub mod workflow_fit;
pub mod workflow_frontend;
pub mod workflow_load;
pub mod workflow_prune;
pub mod workflow_recipe;
pub mod workflow_search;
pub mod workflow_submit;
pub mod write_routes;
