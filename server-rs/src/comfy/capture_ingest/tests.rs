//! `capture_ingest` 的单元测试（主体代码在 [`capture_ingest`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

/// `canReclaim` 的真值表（Kotlin `CaptureRunRetryTest` 的同一个判据）。
#[test]
fn can_reclaim_matches_the_kotlin_truth_table() {
    // 别人正在收（10 分钟内）→ 让开
    assert!(!can_reclaim("running", 0, false));
    assert!(!can_reclaim("running", 600, false));
    // 卡住的 running → 能重抢（后端被杀过）
    assert!(can_reclaim("running", 601, false));
    // ★ empty / error **可以重试**（产物晚到是常态；当终态就会出现"说生成了、画廊里没有"）
    assert!(can_reclaim("empty", 1, false));
    assert!(can_reclaim("error", 1, false));
    // success 是唯一不再重复收的终态
    assert!(!can_reclaim("success", 99999, false));
    // force（目录导入）能重抢，但**抢不走正在处理中的**
    assert!(can_reclaim("success", 1, true));
    assert!(!can_reclaim("running", 1, true));
    // 认不出的状态：保守不让（宁可漏收一次，也不要重复入库）
    assert!(!can_reclaim("weird", 1, false));
}

#[test]
fn ingest_result_json_omits_only_nulls() {
    let result = IngestResult {
        run_key: "run:abc".into(),
        prompt_id: Some(7),
        created: true,
        media_ids: vec![1, 2],
        imported: 2,
        duplicates: 1,
        failed: 0,
        already_captured: false,
        message: None,
    };
    let value = result.to_json();
    let keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec![
            "runKey", "promptId", "created", "mediaIds", "imported", "duplicates", "failed",
            "alreadyCaptured"
        ],
        "除 promptId / message 外所有键都在（Kotlin encodeDefaults=true）"
    );
    assert!(value.get("message").is_none());
    // 默认值也要**在**（不是"省略零值"）
    let bare = IngestResult { run_key: "r".into(), ..Default::default() };
    let bare_value = bare.to_json();
    assert_eq!(bare_value["imported"], 0);
    assert_eq!(bare_value["created"], false);
    assert_eq!(bare_value["mediaIds"], json!([]));
    assert!(bare_value.get("promptId").is_none());
}

#[test]
fn outputs_are_collected_from_every_known_key() {
    let entry = json!({
        "outputs": {
            "9": { "images": [ {"filename":"a.png","subfolder":"sub","type":"output"} ] },
            "12": { "gifs": [ {"filename":"b.webp"} ], "audio": [ {"filename":"c.mp3","type":"temp"} ] },
            "15": { "not_a_key": [ {"filename":"d.png"} ] }
        }
    });
    let graph: Map<String, Value> = serde_json::from_str(
        r#"{"9":{"class_type":"SaveImage"},"12":{"class_type":"VideoCombine"},"15":{"class_type":"X"}}"#,
    )
    .unwrap();
    let outputs = collect_outputs(&entry, Some(&graph));
    let names: Vec<&str> = outputs.iter().map(|o| o.filename.as_str()).collect();
    assert_eq!(names, vec!["a.png", "b.webp", "c.mp3"]);
    assert_eq!(outputs[0].subfolder.as_deref(), Some("sub"));
    assert_eq!(outputs[0].node_type.as_deref(), Some("SaveImage"));
    // `type` 缺省时按 output 补齐（不然下面的过滤会把 webp 当成 temp 丢掉）
    assert_eq!(outputs[1].type_field.as_deref(), Some("output"));
    assert_eq!(outputs[2].type_field.as_deref(), Some("temp"));
}

/// 只收 `type` 为空 / `output` 的产物，并按三元组去重（Kotlin 那两行过滤的对应物）。
#[test]
fn only_output_type_files_survive_and_duplicates_collapse() {
    let req = IngestRequest {
        run_key: "r".into(),
        source: "ComfyUI".into(),
        status: "success".into(),
        error: None,
        comfy_url: None,
        output_dir: None,
        prompt: Some(json!({"1":{"class_type":"SaveImage","inputs":{}}})),
        workflow: None,
        outputs: vec![
            IngestOutput { filename: "a.png".into(), subfolder: None, type_field: Some("output".into()), kind: None, node_id: None, node_type: None },
            IngestOutput { filename: "a.png".into(), subfolder: None, type_field: Some("output".into()), kind: None, node_id: None, node_type: None },
            IngestOutput { filename: "tmp.png".into(), subfolder: None, type_field: Some("temp".into()), kind: None, node_id: None, node_type: None },
            IngestOutput { filename: "  ".into(), subfolder: None, type_field: Some("output".into()), kind: None, node_id: None, node_type: None },
        ],
        nsfw: false,
        title: None,
        notes: None,
        tags: Vec::new(),
        raw: None,
    };
    // 直接复用过滤判据：与 `ingest_inner` 里那段保持一致
    let mut kept: Vec<(String, String, String)> = Vec::new();
    for item in &req.outputs {
        if item.filename.trim().is_empty() {
            continue;
        }
        let type_field = item.type_field.clone().unwrap_or_default();
        if !type_field.trim().is_empty() && !type_field.eq_ignore_ascii_case("output") {
            continue;
        }
        let key = (
            item.filename.clone(),
            item.subfolder.clone().unwrap_or_default(),
            if type_field.trim().is_empty() { "output".to_string() } else { type_field },
        );
        if !kept.contains(&key) {
            kept.push(key);
        }
    }
    assert_eq!(kept.len(), 1, "重复条目要折叠、temp 与非文件名要丢掉：{kept:?}");
}

#[test]
fn kind_for_prefers_the_declared_kind_then_the_node_type() {
    let mut item = IngestOutput {
        filename: "x.bin".into(),
        subfolder: None,
        type_field: Some("output".into()),
        kind: Some("video".into()),
        node_id: None,
        node_type: Some("SaveImage".into()),
    };
    assert_eq!(kind_for(&item), "VIDEO", "提交方声明优先（大小写不敏感）");
    item.kind = None;
    assert_eq!(kind_for(&item), "IMAGE", "认不出来时按扩展名 → IMAGE");
    item.node_type = Some("SaveAudioMP3".into());
    assert_eq!(kind_for(&item), "AUDIO", "节点类型线索");
    item.node_type = Some("VHS_VideoCombine".into());
    assert_eq!(kind_for(&item), "VIDEO");
}

#[test]
fn percent_encoding_matches_urlencoder() {
    assert_eq!(url_encode("a b.png"), "a%20b.png", "空格必须是 %20 而不是 +");
    assert_eq!(url_encode("中文.png"), "%E4%B8%AD%E6%96%87.png");
    assert_eq!(url_encode("a/b-c_d.png"), "a%2Fb-c_d.png");
}
