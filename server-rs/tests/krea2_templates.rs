//! **两份 krea2 模板的活体转换抽查**（默认 `#[ignore]`：要本机 ComfyUI 在跑）。
//!
//! # 它盯的是什么
//!
//! `comfyui/workflows/comfyhub_t2i_krea2.json` 与 `comfyhub_i2i_krea2.json` 是用户
//! 2026-10-17 从 ComfyUI 里存出来的两份真工作流（界面格式）。它们**依赖三件只有前端才会
//! 做的事**，而这三件事以前一件都做不了 —— 表现是"图看着没问题，提交上去缺一堆必填输入"
//! 或者"转换直接报错、整份工作流进不来"：
//!
//! | 现场 | 以前的结果 | 现在的判据 |
//! | --- | --- | --- |
//! | `Anything Everywhere`（cg-use-everywhere）在本机**注册成零输出桩** | 查得到就原样保留 → 广播一路都填不上 | `workflow_frontend::emulate` 对"注册了但 `output` 为空"的广播节点照样改写 |
//! | `Fast Bypasser (rgthree)`（i2i 里那个） | `/object_info` 里没有 → `105.model ← 节点 90（没有转换出来）` | `workflow_convert` 的 `FRONTEND_PASSTHROUGH` 把 1 进 1 出的开关当直通 |
//! | `TextGenerate.sampling_mode`（`COMFY_DYNAMICCOMBO_V3`） | 当成插口 → "必填输入空着"；子项被丢掉 | `is_widget_input` 认动态下拉框 + `fill_widgets` 收 `名字.子项` |
//!
//! 判据都不是猜的：本机 ComfyUI 的 `/history` 里留着那次 i2i 运行**真实提交的 API 图**
//! （`105.model = ["89", 0]`、`TextGenerate.inputs` 里就是 `sampling_mode.temperature` 这些带点的键）。
//!
//! # 怎么跑
//!
//! ```powershell
//! pwsh -File scripts\backend.ps1 status          # 先确认 ComfyUI 在 8188
//! cargo test --manifest-path server-rs\Cargo.toml --test krea2_templates -- --ignored --nocapture
//! ```
//!
//! `COMFYHUB_SPOT_COMFY` 可以换实例地址（默认 `http://127.0.0.1:8188`）。
//! 连不上就**跳过并打印一句**，不假装通过。

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::{Map, Value};

use comfyhub_server_rs::workflow::{workflow_blueprint, workflow_convert};

/// 项目根（`server-rs/` 的上一级）。
fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().expect("项目根").to_path_buf()
}

/// 取一次 `/object_info`（连不上回 `None` + 打印原因）。
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

/// 两份模板各跑一遍转换：**缺口必须是零**。
///
/// "缺口"= `unsupported_nodes`（本机没有的类）/ `unresolved_inputs`（悬挂引用）/
/// 还空着的**必填**输入。最后一项由蓝图体检判（它知道哪个插口必填）。
#[tokio::test]
#[ignore = "活体抽查：要 ComfyUI 在跑（见文件头）"]
async fn krea2_两份模板开模拟用户行为之后都没有缺口() {
    let Some(info) = object_info().await else { return };
    let root = project_root();
    for name in ["comfyhub_t2i_krea2.json", "comfyhub_i2i_krea2.json"] {
        let path = root.join("comfyui").join("workflows").join(name);
        let ui = load(&path.display().to_string());
        let ui_object = ui.as_object().expect("界面格式工作流");

        // ① 转换本身：没有认不出的类、没有悬挂引用
        let converted = workflow_convert::to_api_graph(ui_object, &info, true)
            .unwrap_or_else(|e| panic!("{name} 转换失败：{e}"));
        eprintln!(
            "== {name}：{} 个节点 → API 图 {} 个节点；前端改写 {} 处，广播 {} 条",
            ui_object["nodes"].as_array().map(|list| list.len()).unwrap_or(0),
            converted.node_count,
            converted.frontend_rewritten.len(),
            converted.broadcast_lines.len()
        );
        for line in &converted.frontend_rewritten {
            eprintln!("   前端改写：{line}");
        }
        for line in &converted.broadcast_lines {
            eprintln!("   广播补线：{line}");
        }
        assert!(converted.unsupported_nodes.is_empty(), "{name} 有没转换出来的节点：{:?}", converted.unsupported_nodes);
        assert!(converted.unresolved_inputs.is_empty(), "{name} 有悬挂引用：{:?}", converted.unresolved_inputs);

        // ② 接线体检：不许有 error（warn 可以 —— 比如"这些参数对不齐"）
        let blueprint = workflow_blueprint::build(&ui, Some(&info), true);
        for issue in &blueprint.issues {
            eprintln!("   [{}] {} —— {}", issue.severity, issue.title, issue.detail);
        }
        assert_eq!(
            blueprint.count(workflow_blueprint::SEV_ERROR),
            0,
            "{name} 体检有错误：{:?}",
            blueprint.issues.iter().filter(|i| i.severity == workflow_blueprint::SEV_ERROR).map(|i| i.title.clone()).collect::<Vec<_>>()
        );

        // ③ 一个输出节点都没有 = 跑不出东西（这两份都必须有 SaveImage）
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
