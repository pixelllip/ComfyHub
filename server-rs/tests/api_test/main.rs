//! Rust 端点的形状与纪律用例（P4 灰度第一批）。
//!
//! # 这一组用例在防什么
//!
//! P4 的验收前提是"**Flutter 一行不改**"，而 Flutter 读的是 JSON 字段名。
//! 字段写错**不会报错**：界面那个字段就是空的（比如把 `positivePrompt` 写成 `positive_prompt`，
//! 提示词详情页的正文会整段消失）。所以：
//!
//! 1. 每个 DTO 都断言**精确的键集合**（不是"包含某几个键"）——
//!    这样"少一个键"和"多一个键"都会红；
//! 2. 断言 Rust 与 Kotlin 的**分页信封**一致（`items/total/page/size/pages`）；
//! 3. R1 / R2 / R14 各有一条（这些防线在 Rust 侧同样要重建，总纲 §6）。
//!
//! 数据是**测试自己灌的**（内存库 + 真 schema），不依赖机器上那份 `comfyhub.db`。
//!
//! # 怎么分的（2026-09-30 拆分：原来一个文件 10,684 行 / 150 个用例）
//!
//! **一个测试二进制 + 按域分子模块**（不是拆成多个二进制）：`cargo test --test api_test`
//! 与报告形状都不变，用例一条没删。子模块布局按 Cargo 的规矩用 `tests/api_test/main.rs`
//! 当根（这样 `mod foo;` 找的是同目录的 `foo.rs`，不需要 `#[path]`）。
//!
//! | 文件 | 装什么 |
//! | --- | --- |
//! | `common.rs` | 共享夹具：`test_conn`（真 schema + 灌数据）、`state`、`app`、`router_for`、四种请求助手、假 ComfyUI、上传/落盘助手 |
//! | `basics.rs` | 健康检查、tags / prompts 读侧、错误体形状、外键、未迁移端点 |
//! | `media.rs` | 产物列表 / 文件流（Range + ETag）/ 缩略图 / 写侧 / 上传 / stats |
//! | `capture.rs` | 本机工作流文件、捕获配置与工作流原文、轮询入库、产物下载兜底 |
//! | `sync.rs` | 同步 API（manifest / changes / push / bootstrap）、可达地址、凭据上传 |
//! | `auth.rs` | 配对闸门与密钥下行：开关、ticket、claim、限流、授权判据 |
//! | `ai_conversations.rs` | 会话与消息的读写侧、pending、Run 持久化、事件重放、启动恢复 |
//! | `ai_executor.rs` | 一次 Run 的主循环、流中断、审批闸门、提问、轮次与预算上限、重试 |
//! | `ai_providers.rs` | Provider CRUD、模型目录、内置目录、能力三层、凭据、联网搜索凭据 |
//! | `ai_run_kit.rs` | run-kit 工件与 Skills / 记忆 |
//! | `cards.rs` | 角色卡 / 画风卡：双版本、示例产物、错误码 |
//! | `tools.rs` | 桌面文件五件、工具清单与作用域、资料库检索 |
//! | `comfy.rs` | comfy 查询两件、状态/队列/进度、读工作流、体检/审核/摘拼、图像编辑投放、起停实例 |
//!
//! 拆分的判据是**文件名与用例一一对应、搬完数目不变**：144 个 ASCII 名 + 6 个中文名 = 150。

mod common;
mod basics;
mod media;
mod capture;
mod sync;
mod auth;
mod ai_conversations;
mod ai_executor;
mod ai_providers;
mod ai_run_kit;
mod cards;
mod tools;
mod comfy;
