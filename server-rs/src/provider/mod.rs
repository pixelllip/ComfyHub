//! `provider/` —— 上游模型接入。
//!
//! 协议适配（completions / anthropic / responses）、HTTP 客户端、连接测试与模型发现、凭据、能力目录与实测校准。

pub mod builtin;
pub mod capability_catalog;
pub mod capability_probe;
pub mod capability_refresh;
pub mod capability_rules;
pub mod credentials;
pub mod protocol;
pub mod provider_diagnose;
pub mod provider_probe;
pub mod request_extras;
pub mod upstream;
