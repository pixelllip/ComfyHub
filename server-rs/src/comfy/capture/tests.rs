//! `capture` 的单元测试（主体代码在 [`capture`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

/// T3（2026-11-09）：来源档始终给、配方 id 可空则省略（Java `explicitNulls=false` 的老规矩）。
#[test]
fn provenance_to_json_带来源档_配方可空则省略() {
    let info = CaptureRunInfo {
        run_key: "run-1".into(),
        prompt_id: Some(1456),
        status: "success".into(),
        media_count: 1,
        title: None,
        error: None,
        captured_at: None,
        source_kind: "recipe".into(),
        recipe_id: Some(7),
    };
    let json = info.to_json();
    assert_eq!(json["sourceKind"], "recipe");
    assert_eq!(json["recipeId"], 7);

    let info = CaptureRunInfo {
        source_kind: "ComfyUI".into(),
        recipe_id: None,
        ..info
    };
    let json = info.to_json();
    assert_eq!(json["sourceKind"], "ComfyUI", "外部捕获默认值要能读出来");
    assert!(json.get("recipeId").is_none(), "没有配方就不出现这个键");
}

#[test]
fn normalize_clamps_and_trims_like_kotlin() {
    let cfg = normalize(CaptureConfig {
        enabled: false,
        comfy_url: "  http://127.0.0.1:8188/  ".into(),
        output_dir: Some("   ".into()),
        poll_seconds: 99_999,
        auto_tag: "  ComfyUI  ".into(),
        max_per_poll: 0,
        download_fallback: false,
    });
    assert_eq!(cfg.comfy_url, "http://127.0.0.1:8188");
    assert_eq!(cfg.output_dir, None, "空白目录当没配");
    assert_eq!(
        cfg.poll_seconds, 600,
        "轮询间隔夹到上限（太快会把 ComfyUI 问崩）"
    );
    assert_eq!(cfg.max_per_poll, 1, "每次最多处理条数夹到下限");
    assert_eq!(cfg.auto_tag, "ComfyUI");
    assert!(!cfg.enabled);
    assert!(!cfg.download_fallback);
}

#[test]
fn empty_comfy_url_falls_back_to_the_default() {
    let cfg = normalize(CaptureConfig {
        comfy_url: "   ".into(),
        ..CaptureConfig::default()
    });
    assert_eq!(cfg.comfy_url, "http://127.0.0.1:8188");
}

#[test]
fn config_round_trips_with_camel_case_keys() {
    let json = serde_json::to_string(&CaptureConfig::default()).unwrap();
    // 前端读的是驼峰键（Kotlin 的 @Serializable 也是驼峰默认名）
    for key in [
        "enabled",
        "comfyUrl",
        "pollSeconds",
        "autoTag",
        "maxPerPoll",
        "downloadFallback",
    ] {
        assert!(json.contains(key), "缺键 {key}：{json}");
    }
    // ⚠ `outputDir` 是 `String?`：没配时**键不出现**（Kotlin `explicitNulls = false`），
    // 配了才出现 —— 与真库副本上两个实现的对照结果一致。
    assert!(
        !json.contains("outputDir"),
        "没配输出目录时不该有 outputDir 键：{json}"
    );
    let with_dir = CaptureConfig {
        output_dir: Some("D:\\out".into()),
        ..CaptureConfig::default()
    };
    let json = serde_json::to_string(&with_dir).unwrap();
    assert!(json.contains("outputDir"), "配了就要有：{json}");

    let back: CaptureConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back, with_dir);
    // 缺 `outputDir` 键也要能解析回 None（`#[serde(default)]`，手机/老版本客户端都认）
    let back: CaptureConfig =
        serde_json::from_str(&serde_json::to_string(&CaptureConfig::default()).unwrap()).unwrap();
    assert_eq!(back, CaptureConfig::default());
}

// -----------------------------------------------------------------------
//  HistoryEntry（api-graph 的取数核心）
// -----------------------------------------------------------------------

fn graph(id: &str) -> serde_json::Value {
    serde_json::json!({ id: { "class_type": "KSampler", "inputs": { "seed": 1 } } })
}

#[test]
fn history_parses_the_flat_form() {
    // {prompt: <图>}：最常见的那种
    let entry = serde_json::json!({ "prompt": graph("3") });
    let parsed = history::parse(&entry);
    assert_eq!(parsed.graph, Some(graph("3")));
    assert!(parsed.extra_data.is_none(), "整张图不是 extra_data");
    assert!(parsed.workflow.is_none());
}

#[test]
fn history_parses_the_named_form_and_pulls_out_the_workflow() {
    // {prompt: {prompt: <图>, extra_data: {extra_pnginfo: {workflow: {...}}}}}
    let entry = serde_json::json!({
        "prompt": {
            "prompt": graph("3"),
            "extra_data": { "extra_pnginfo": { "workflow": { "nodes": [] } }, "client_id": "abc" }
        }
    });
    let parsed = history::parse(&entry);
    // ⚠ 这里最容易错：把**外层**当成图（外层每个值都是对象，但它们的值里没有 class_type）
    assert_eq!(parsed.graph, Some(graph("3")), "具名形式要取里层那一份");
    assert_eq!(
        parsed.workflow,
        Some(serde_json::json!({ "nodes": [] })),
        "界面格式工作流在 extra_pnginfo.workflow"
    );
}

#[test]
fn history_parses_the_array_form() {
    // {prompt: [<图>, <extra_data>]}：数组形式，两个元素都要认出来
    let entry =
        serde_json::json!({ "prompt": [graph("3"), { "client_id": "abc", "create_time": 1 }] });
    let parsed = history::parse(&entry);
    assert_eq!(parsed.graph, Some(graph("3")));
    assert_eq!(
        parsed.extra_data,
        Some(serde_json::json!({ "client_id": "abc", "create_time": 1 }))
    );
}

#[test]
fn history_refuses_to_guess_when_there_is_no_graph() {
    // 没有 prompt 字段 / 形状不认识 → 一律 None（**绝不猜**：猜错就是拿 extra_data 去提交）
    assert_eq!(
        history::parse(&serde_json::json!({ "outputs": {} })).graph,
        None
    );
    assert_eq!(
        history::parse(&serde_json::json!({ "prompt": "oops" })).graph,
        None
    );
    assert_eq!(
        history::parse(&serde_json::json!({ "prompt": {} })).graph,
        None,
        "空对象不是节点图"
    );
    // 值是对象但没有 class_type → 不是图
    assert_eq!(
        history::parse(&serde_json::json!({ "prompt": { "a": { "inputs": {} } } })).graph,
        None
    );
}

#[test]
fn extra_data_is_not_mistaken_for_a_node_graph() {
    // extra_data 的每个值常常也是对象，但它绝不该被当成节点图
    let extra = serde_json::json!({ "extra_pnginfo": { "workflow": { "nodes": [] } } });
    assert!(!history::is_node_graph(extra.as_object().unwrap()));
    let entry = serde_json::json!({ "prompt": { "prompt": graph("3"), "extra_data": extra } });
    let parsed = history::parse(&entry);
    assert_ne!(parsed.graph, Some(extra.clone()), "图不能取成 extra_data");
}
