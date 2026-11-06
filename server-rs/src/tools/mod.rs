//! `tools/` —— AI 工具层。
//!
//! 工具清单（桌面/手机）、权限策略与路径准入、MCP 桥（comfy-mcp 子进程）、联网工具、找模型。

pub mod civitai;
pub mod danbooru;
pub mod desktop_tools;
pub mod find_model;
pub mod mcp;
pub mod mobile_tools;
pub mod policy;
pub mod reference_images;
pub mod tool_cache;
// 同上：`tools::tools` 是工具清单那一层的老名字。
#[allow(clippy::module_inception)]
pub mod tools;
pub mod web_tools;
