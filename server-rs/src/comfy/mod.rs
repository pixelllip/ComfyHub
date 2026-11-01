//! `comfy/` —— ComfyUI 实例与输入准备。
//!
//! 实例起停与写权限闸门、本机模型清单、目录探测、实时进度、自动捕获四条、投放与遮罩（image_edit）。

pub mod capture;
pub mod capture_import;
pub mod capture_ingest;
pub mod capture_poller;
pub mod comfy_client;
pub mod comfy_dirs;
pub mod comfy_launch;
pub mod comfy_models;
pub mod comfy_progress;
pub mod image_edit;
pub mod media_view;
