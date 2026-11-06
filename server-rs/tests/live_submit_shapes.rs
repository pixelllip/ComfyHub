//! **活体提交抽查**：把一份**界面格式**工作流转成 API 图，真的交给 ComfyUI 的 `/prompt` 验一次。
//!
//! 默认 `#[ignore]`：要本机 ComfyUI 在跑，而且要用 `COMFYHUB_LIVE_WORKFLOW` 指一份真工作流
//! （界面格式：`{nodes, links}`）。没指就**跳过并打印一句**，不假装通过。
//!
//! ```powershell
//! $env:COMFYHUB_LIVE_WORKFLOW = 'D:\Comfy-Desktop\...\workflows\krea2 r2i workflow.json'
//! cargo test --manifest-path server-rs\Cargo.toml --test live_submit_shapes -- --ignored --nocapture
//! ```
//!
//! # 它盯的是一条**只能靠真机发现**的回归
//!
//! 2026-10-25：转换出来的连线引用写成了**数字**（`"model": [128, 0]`），而 ComfyUI 的
//! `validate_prompt` 拿引用里的 id **直接索引 prompt 字典**（键永远是字符串）→
//! `KeyError`，被 `/prompt` 包成
//! `prompt_outputs_failed_validation … {"exception_type":"KeyError","details":"128"}`。
//! 单元测试只能钉住"我们写的是字符串"，**"ComfyUI 收不收"只有真机能答**。
//!
//! 提交成功后**立刻撤单**（`POST /queue {"delete": [...]}`）：这一步只验"收不收"，
//! 不占 GPU、不落图。

use std::collections::HashSet;

use serde_json::{json, Map, Value};

use comfyhub_server_rs::workflow::workflow_convert;

fn comfy_base() -> String {
    std::env::var("COMFYHUB_SPOT_COMFY")
        .unwrap_or_else(|_| "http://127.0.0.1:8188".to_string())
        .trim()
        .trim_end_matches('/')
        .to_string()
}

fn ui_workflow() -> Option<Map<String, Value>> {
    let Some(path) = std::env::var("COMFYHUB_LIVE_WORKFLOW")
        .ok()
        .filter(|p| !p.trim().is_empty())
    else {
        eprintln!("跳过：没有设置 COMFYHUB_LIVE_WORKFLOW（本用例要一份界面格式工作流的路径）");
        return None;
    };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {path}：{e}"));
    let value: Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path} 不是合法 JSON：{e}"));
    let Some(map) = value.as_object() else {
        eprintln!("跳过：{path} 顶层不是对象");
        return None;
    };
    if !map.contains_key("nodes") || !map.contains_key("links") {
        eprintln!("跳过：{path} 不是界面格式（没有 nodes / links）");
        return None;
    }
    Some(map.clone())
}

/// 转换 → 看点 → `/prompt`；**要求 200**（400 的正文原样打出来，指得到具体节点）。
#[tokio::test]
#[ignore = "活体抽查：要 ComfyUI 在跑，且 COMFYHUB_LIVE_WORKFLOW 指向一份界面格式工作流"]
async fn 转出来的图_comfyui_收得下() {
    let base = comfy_base();
    let Some(ui) = ui_workflow() else { return };
    let object_info =
        match comfyhub_server_rs::comfy::comfy_client::get_json(&format!("{base}/object_info"))
            .await
        {
            Ok(value) => value.as_object().cloned().unwrap_or_default(),
            Err(error) => {
                eprintln!("跳过：连不上 {base}（{error}）—— 这份抽查要 ComfyUI 在跑");
                return;
            }
        };

    let converted = workflow_convert::to_api_graph(&ui, &object_info, true)
        .unwrap_or_else(|e| panic!("转换失败：{e}"));
    assert!(
        converted.unsupported_nodes.is_empty(),
        "本机缺节点类，这份工作流要先把缺口补齐：{:?}",
        converted.unsupported_nodes
    );

    // ① 先把"引用一定是字符串"钉死（数字引用的现场就是 KeyError）
    let mut numeric: Vec<String> = Vec::new();
    for (id, node) in &converted.graph {
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        for (name, value) in inputs {
            let Some(list) = value.as_array() else {
                continue;
            };
            if list.len() == 2 && list[0].is_number() {
                numeric.push(format!("{id}.{name}"));
            }
        }
    }
    assert!(
        numeric.is_empty(),
        "连线引用写成了数字（ComfyUI 会 KeyError）：{numeric:?}"
    );

    // ② 真提交一次（`client_id` 随便给一个稳定值，方便在队列里认出它）
    let body = json!({ "prompt": converted.graph, "client_id": "comfyhub-live-submit-check" });
    let outcome =
        comfyhub_server_rs::comfy::comfy_client::post_json(&format!("{base}/prompt"), &body).await;
    let prompt_id = match outcome {
        Ok(value) => {
            let id = value
                .get("prompt_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            eprintln!(
                "ComfyUI 收下了：prompt_id={id}（节点 {}）",
                converted.node_count
            );
            id
        }
        Err(error) => panic!("ComfyUI 拒了这张图（说明转换出来的形状还是不对）：{error}"),
    };

    // ③ 立刻撤单：这一步只验"收不收"，不占 GPU
    if !prompt_id.is_empty() {
        let _ = comfyhub_server_rs::comfy::comfy_client::post_json(
            &format!("{base}/queue"),
            &json!({ "delete": [prompt_id] }),
        )
        .await;
    }

    // ④ 顺带盯一眼"转换没把前端节点漏进来"（真前端类在 /object_info 里查不到）
    let mut leaked: HashSet<String> = HashSet::new();
    for node in converted.graph.values() {
        let class = node
            .get("class_type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !object_info.contains_key(class) {
            leaked.insert(class.to_string());
        }
    }
    assert!(leaked.is_empty(), "图里还有本机没有的节点类：{leaked:?}");
}
