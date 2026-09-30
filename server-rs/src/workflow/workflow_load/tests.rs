//! `workflow_load` 的单元测试（主体代码在 [`workflow_load`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn node_graph_detection_matches_the_capture_side() {
    let api: Map<String, Value> =
        serde_json::from_str(r#"{"1":{"class_type":"KSampler","inputs":{}}}"#).unwrap();
    assert!(is_node_graph(&api));
    assert!(!is_node_graph(&Map::new()), "空图不算节点图");
    let ui: Map<String, Value> = serde_json::from_str(r#"{"nodes":[],"links":[]}"#).unwrap();
    assert!(!is_node_graph(&ui));
    // 混着 `extra_data` 那种（`prompt` + `extra_data`）不算节点图
    let mixed: Map<String, Value> =
        serde_json::from_str(r#"{"prompt":{},"extra_data":{}}"#).unwrap();
    assert!(!is_node_graph(&mixed));
}

/// **自己写的 API 图被拒时，错误要说清是哪个键不像节点**（2026-09-27 活实例踩出来的）。
///
/// 现场：AI 搭的工作流第一版顶层带了 `_comment`、节点里也塞了注释 → `NOT_A_WORKFLOW`。
/// 它自己改成纯净格式才读进去（白花一轮）。这条用例钉住"提示里点得出那个键"。
#[test]
fn api_格式提示要点出哪个顶层键不像节点() {
    // 顶层多了一个字符串键 —— 这正是 AI 第一版的样子
    let polluted: Map<String, Value> = serde_json::from_str(
        r#"{"_comment":"给人看的注释","1":{"class_type":"KSampler","inputs":{}}}"#,
    )
    .unwrap();
    assert!(!is_node_graph(&polluted), "有一个非节点键就不算 API 图");
    let hint = api_format_hint(&polluted);
    assert!(hint.contains("_comment"), "要点出是哪个键：{hint}");
    assert!(hint.contains("class_type"), "要说清合法节点长什么样：{hint}");

    // 纯净的 API 图：不需要提示（正常情况下也走不到这条错误上）
    let clean: Map<String, Value> =
        serde_json::from_str(r#"{"1":{"class_type":"KSampler","inputs":{}}}"#).unwrap();
    assert!(api_format_hint(&clean).is_empty());
    // ⚠ 节点**内部**的 `_meta` 是允许的（项目自带的四份工作流每个节点都带它）
    assert!(is_node_graph(&clean));
    let with_meta: Map<String, Value> = serde_json::from_str(
        r#"{"1":{"_meta":{"title":"采样"},"class_type":"KSampler","inputs":{}}}"#,
    )
    .unwrap();
    assert!(is_node_graph(&with_meta), "节点内的 _meta 不该被判死（自带工作流都这么写）");
}

#[test]
fn summarize_splits_widgets_from_linked_inputs() {
    let graph: Map<String, Value> = serde_json::from_str(
        r#"{"5":{"class_type":"KSampler","_meta":{"title":"采样"},"inputs":{
             "model":["1",0],"seed":123,"steps":20}}}"#,
    )
    .unwrap();
    let summary = summarize_nodes(&graph);
    let node = &summary[0];
    assert_eq!(node["id"], "5");
    assert_eq!(node["classType"], "KSampler");
    assert_eq!(node["title"], "采样");
    assert_eq!(node["widgets"], json!(["seed", "steps"]));
    assert_eq!(node["linked"], json!(["model"]));
}

#[test]
fn sha256_is_stable_and_content_addressed() {
    let a = sha256_hex(b"hello");
    assert_eq!(a, sha256_hex(b"hello"));
    assert_ne!(a, sha256_hex(b"hello "));
    assert_eq!(a.len(), 64);
}
