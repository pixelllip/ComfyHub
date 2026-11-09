//! `workflow/` —— 工作流。
//!
//! 界面格式 ↔ API 节点图、跑之前体检、摘/拼节点、配方（学/列/绑）、提交（R10 的落点）、
//! 蓝图（可绘制的节点-连线模型 + 接线体检）、参考图（`LoadImage` 指向的真实文件）。

pub mod auto_select;
pub mod graph_parse;
pub mod object_info_cache;
pub mod reference_images;
pub mod workflow_audit;
pub mod workflow_blueprint;
pub mod workflow_convert;
pub mod workflow_fit;
pub mod workflow_frontend;
pub mod workflow_load;
pub mod workflow_prune;
pub mod workflow_recipe;
pub mod workflow_search;
pub mod workflow_submit;
