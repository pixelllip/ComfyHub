//! ComfyHub 桌面服务端（Rust 版）的库入口。
//!
//! 二进制（`main.rs`）只是"读参数 → 开库 → 起服务"的壳；
//! 真正的实现都在库里，**这样集成测试能直接 `use comfyhub_server_rs::...`**
//! （只写 bin 的 crate 没法被 `tests/` 引用）。
//!
//! # 模块地图（按大功能分域，目录结构对齐 Flutter 前端）
//!
//! 80 个模块分在 10 个域里，**判据是"这功能归谁管"**，不是文件名字面。
//! 每个域一个目录 + `mod.rs`（目录里那份 `mod.rs` 只写"这个域装了什么"，
//! 每个模块自己的 `tests.rs` 与它并排放在 `<域>/<模块>/tests.rs`）。
//!
//! | 域 | 装什么 | 一眼能认出的模块 |
//! | --- | --- | --- |
//! | [`ai`] | AI 会话、执行与提示词 | `executor`（Run 主循环）、`runs`、`conversations`、`memory`、`skills`、`system_prompt` |
//! | [`provider`] | 上游模型接入 | `protocol`（三种方言）、`upstream`、`credentials`、`capability_*`、`builtin` |
//! | [`tools`] | AI 工具层 | `tools`（清单）、`policy`（权限与路径准入）、`mcp`（comfy-mcp 桥）、`find_model` |
//! | [`comfy`] | ComfyUI 实例与输入准备 | `comfy_launch`（起停 + 写权限闸门）、`comfy_models`、`capture_*`、`image_edit`（投放/遮罩） |
//! | [`workflow`] | 工作流 | `workflow_convert`（界面↔API 图）、`workflow_fit`、`workflow_prune`、`workflow_recipe`、`workflow_submit` |
//! | [`media`] | 媒体与文件 | `media`（产物 + 文件流）、`media_upload`、`thumbs`、`attachments`、`strict_intake` |
//! | [`library`] | 本地内容库 | `repo`（标签/提示词仓储）、`character_cards`（角色卡） |
//! | [`infra`] | 基础设施 | `db`（事务纪律 R1/R2）、`confinement`（沙箱自查）、`outbound`（代理）、`testpaths` |
//! | [`mobile`] | 跨端（手机） | `auth`（配对闸门）、`sync`、`submit_queue`（排队→桌面提交）、`net_urls` |
//! | [`train`] | 本地 LoRA 训练引擎（可选能力） | `mod.rs`（探测 anima-lora-next / 起停守护进程与 WebUI / 状态） |
//! | [`web`] | HTTP 层 | `routes` / `write_routes`（装配）、`models` / `inputs`（形状）、`endpoint_guard`（SSRF） |
//!
//! 三条纪律（改分层图时一起守）：
//!
//! 1. **模块全名（`crate::comfy::image_edit`）是唯一写法** —— 别在别处再 `pub use` 一个短路径
//!    出来（同一个模块两条路会让人不知道哪条是真的）。
//! 2. **搬模块就要改全**：代码里的 `crate::`/`comfyhub_server_rs::`、集成测试、`include_str!`
//!    的相对深度、以及 `docs/` 里的模块引用，一处都不能漏（`.run/reorg-modules.py` 是那次的脚本）。
//! 3. **域的边界按职责，不按名字前缀**：`capture_*` 归 `comfy`（它抓的是 ComfyUI 的运行），
//!    `mcp` 归 `tools`（它是工具桥，不是 AI 核心），`testpaths` 归 `infra`（它服务所有测试）。

pub mod ai;
pub mod comfy;
pub mod infra;
pub mod library;
pub mod media;
pub mod mobile;
pub mod provider;
pub mod tools;
pub mod train;
pub mod web;
pub mod workflow;
