//! **两份 MOSS-Image 模板的活体转换抽查**（默认 `#[ignore]`：要本机 ComfyUI 在跑）。
//!
//! 盯的是 `comfyui/workflows/comfyhub_moss_t2i.json` 与 `comfyhub_moss_layout.json`
//! （用户 2026-10 从 ComfyUI 存出来的真工作流）：
//!
//! | 现场 | 为什么容易坏 |
//! | --- | --- |
//! | `TextGenerate.sampling_mode` 是 `COMFY_DYNAMICCOMBO_V3` | 子项键名带点，拼错就静默退回默认值 |
//! | `ComfySwitchNode` / `PreviewAny` 串联 | `CLIPTextEncode.text` 被连线覆盖，直接改控件无效 |
//! | `Fast Groups Bypasser (rgthree)` | 不在 `/object_info`，要按前端直通处理 |
//! | `mode=4` 的旁路节点 | 旁路的不执行，别把 `LoadImage` 当成真的参考图输入 |
//!
//! 判据：转换缺口为零 + 蓝图体检 0 error。
//!
//! ```powershell
//! cargo test --manifest-path server-rs\Cargo.toml --test moss_templates -- --ignored --nocapture
//! ```
//!
//! `COMFYHUB_SPOT_COMFY` 可以换实例地址（默认 `http://127.0.0.1:8188`）。

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::{Map, Value};

use comfyhub_server_rs::workflow::workflow_submit::WorkflowEdit;
use comfyhub_server_rs::workflow::{workflow_blueprint, workflow_convert};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().expect("项目根").to_path_buf()
}

async fn object_info() -> Option<Map<String, Value>> {
    let base = std::env::var("COMFYHUB_SPOT_COMFY")
        .unwrap_or_else(|_| "http://127.0.0.1:8188".to_string());
    let base = base.trim().trim_end_matches('/').to_string();
    match comfyhub_server_rs::comfy::comfy_client::get_json(&format!("{base}/object_info")).await {
        Ok(value) => value.as_object().cloned(),
        Err(error) => {
            eprintln!("跳过：连不上 {base}（{error}）—— 这份抽查要 ComfyUI 在跑");
            None
        }
    }
}

fn load(path: &str) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读不到 {path}：{e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path} 不是合法 JSON：{e}"))
}

#[tokio::test]
#[ignore = "活体抽查：要 ComfyUI 在跑（见文件头）"]
async fn moss_两份模板开模拟用户行为之后都没有缺口() {
    let Some(info) = object_info().await else { return };
    let root = project_root();
    for name in ["comfyhub_moss_t2i.json", "comfyhub_moss_layout.json"] {
        let path = root.join("comfyui").join("workflows").join(name);
        let ui = load(&path.display().to_string());
        let ui_object = ui.as_object().expect("界面格式工作流");

        let converted = workflow_convert::to_api_graph(ui_object, &info, true)
            .unwrap_or_else(|e| panic!("{name} 转换失败：{e}"));
        eprintln!(
            "== {name}：{} 个节点 → API 图 {} 个节点；前端改写 {} 处，广播 {} 条",
            ui_object["nodes"].as_array().map(|list| list.len()).unwrap_or(0),
            converted.node_count,
            converted.frontend_rewritten.len(),
            converted.broadcast_lines.len()
        );
        assert!(
            converted.unsupported_nodes.is_empty(),
            "{name} 有没转换出来的节点：{:?}",
            converted.unsupported_nodes
        );
        assert!(
            converted.unresolved_inputs.is_empty(),
            "{name} 有悬挂引用：{:?}",
            converted.unresolved_inputs
        );

        // ③ 正面提示词注入：t2i 应写进 34 并摘掉 32 TextGenerate；layout 应写进 6。
        let graph = Value::Object(converted.graph.clone());
        let (patched, changed, bypassed) =
            WorkflowEdit::apply_positive_prompt(&graph, "最终提示词").unwrap();
        eprintln!("   positivePrompt → {changed:?}；TextGenerate 摘掉 {bypassed:?}");
        if name == "comfyhub_moss_t2i.json" {
            assert_eq!(patched["34"]["inputs"]["text"], "最终提示词");
            assert!(bypassed.contains(&"32".to_string()), "t2i 的 TextGenerate 是节点 32");
        } else {
            assert_eq!(patched["6"]["inputs"]["text"], "最终提示词");
        }

        let blueprint = workflow_blueprint::build(&ui, Some(&info), true);
        for issue in &blueprint.issues {
            eprintln!("   [{}] {} —— {}", issue.severity, issue.title, issue.detail);
        }
        assert_eq!(
            blueprint.count(workflow_blueprint::SEV_ERROR),
            0,
            "{name} 体检有错误：{:?}",
            blueprint
                .issues
                .iter()
                .filter(|i| i.severity == workflow_blueprint::SEV_ERROR)
                .map(|i| i.title.clone())
                .collect::<Vec<_>>()
        );

        let output_nodes: HashSet<&str> = blueprint
            .nodes
            .iter()
            .filter(|node| {
                info.get(&node.class)
                    .and_then(Value::as_object)
                    .and_then(|entry| entry.get("output_node"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .map(|node| node.class.as_str())
            .collect();
        assert!(!output_nodes.is_empty(), "{name} 没有任何输出节点");
        eprintln!("   输出节点：{output_nodes:?}");
    }
}
