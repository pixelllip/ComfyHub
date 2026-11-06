//! `web/` —— HTTP 层。
//!
//! 路由装配（读/写）、端点的 JSON 形状与输入形状、端口文件、端点信任闸门（SSRF）。

pub mod api_port_file;
pub mod backup;
pub mod endpoint_guard;
pub mod inputs;
pub mod models;
pub mod routes;
pub mod write_routes;
