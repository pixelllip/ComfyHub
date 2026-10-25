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

// ---------------------------------------------------------------------------
//  LoRA 回填（2026-10-23）
// ---------------------------------------------------------------------------

fn memory_db() -> rusqlite::Connection {
    let conn = crate::infra::db::open_memory().unwrap();
    conn.execute_batch(crate::infra::db::SCHEMA_SQL).expect("建 schema");
    crate::infra::db::assert_ready(&conn).unwrap();
    conn
}

/// 造一条捕获来的提示词；`edited` 模拟"用户改过"（`updated_at != created_at`）。
fn captured_prompt(conn: &rusqlite::Connection, loras: &str, edited: bool) -> i64 {
    conn.execute(
        "INSERT INTO prompts (title, kind, positive_prompt, loras, source) \
         VALUES ('t', 'IMAGE', 'p', ?1, 'ComfyUI')",
        [loras],
    )
    .unwrap();
    let id = conn.last_insert_rowid();
    if edited {
        // 显式写成另一个时间：`trg_prompts_touch` 的 WHEN 是 `NEW IS OLD`，不会把它顶回去
        conn.execute(
            "UPDATE prompts SET updated_at = '2000-01-01T00:00:00.000Z' WHERE id = ?1",
            [id],
        )
        .unwrap();
    }
    id
}

fn attach_capture(conn: &rusqlite::Connection, prompt_id: i64, raw: Value) {
    conn.execute(
        "INSERT INTO capture_runs (run_key, prompt_id, raw) VALUES (?1, ?2, ?3)",
        params![format!("run:{prompt_id}"), prompt_id, raw.to_string()],
    )
    .unwrap();
}

/// 真机形状：`Krea2ControlLoRALoader` + rgthree 的 `Lora Loader Stack`。
fn stack_graph(with_lora: bool) -> Value {
    if with_lora {
        json!({"prompt": [1, "uuid", {
          "1": {"class_type":"KSampler","inputs":{"steps":8,"cfg":1,"seed":7,"model":["91",0]}},
          "91": {"class_type":"Lora Loader Stack (rgthree)","inputs":{
                  "model":["4",0],
                  "lora_01":"KNP.safetensors","strength_01":1.25,
                  "lora_02":"hiten.safetensors","strength_02":0.3}},
          "4": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"m.safetensors"}}
        }]})
    } else {
        json!({"prompt": [1, "uuid", {
          "1": {"class_type":"KSampler","inputs":{"steps":8,"cfg":1,"seed":7,"model":["4",0]}},
          "4": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"m.safetensors"}}
        }]})
    }
}

fn loras_of(conn: &rusqlite::Connection, id: i64) -> String {
    conn.query_row("SELECT loras FROM prompts WHERE id = ?1", [id], |row| row.get(0)).unwrap()
}

/// 只追加缺的 LoRA；用户编辑过的、解析为空的、本来就没有 LoRA 的都不动。
#[test]
fn lora_backfill_appends_missing_and_never_rewrites_user_edits() {
    let conn = memory_db();

    // ① 老记录：只有 Krea2 控制 LoRA，栈里那两个没认出来
    let partial = captured_prompt(
        &conn,
        r#"[{"name":"depth-control-lora.safetensors","weight":1.0}]"#,
        false,
    );
    attach_capture(&conn, partial, stack_graph(true));

    // ② 空列表：应该补全
    let empty = captured_prompt(&conn, "[]", false);
    attach_capture(&conn, empty, stack_graph(true));

    // ③ 用户编辑过：一个字都不许动
    let edited = captured_prompt(&conn, "[]", true);
    attach_capture(&conn, edited, stack_graph(true));

    // ④ 图里没有 LoRA：不许把库里已有的清空
    let no_lora = captured_prompt(
        &conn,
        r#"[{"name":"keep.safetensors","weight":0.5}]"#,
        false,
    );
    attach_capture(&conn, no_lora, stack_graph(false));

    assert_eq!(backfill_loras(&conn, true).unwrap(), 2, "只改 ①② 两条");

    let partial_loras = loras_of(&conn, partial);
    assert!(partial_loras.contains("depth-control-lora.safetensors"), "{partial_loras}");
    assert!(partial_loras.contains("KNP.safetensors"), "{partial_loras}");
    assert!(partial_loras.contains("hiten.safetensors"), "{partial_loras}");
    // 原有的那一条不被覆盖（名字在前、权重原样）
    assert!(partial_loras.starts_with(r#"[{"name":"depth-control-lora.safetensors","weight":1.0}"#));

    assert!(loras_of(&conn, empty).contains("KNP.safetensors"));
    assert_eq!(loras_of(&conn, edited), "[]", "用户编辑过的行不许动");
    assert_eq!(
        loras_of(&conn, no_lora),
        r#"[{"name":"keep.safetensors","weight":0.5}]"#,
        "图里没有 LoRA 时不许清空"
    );

    // 回填过的行 `updated_at` 已被顶上去 → 第二遍是空操作
    assert_eq!(backfill_loras(&conn, true).unwrap(), 0);
}

/// 版本升级那趟（`only_untouched = false`）要能补到"已经被我们顶过 `updated_at`"的行，
/// 但仍然只追加：用户自己改过的名字 / 权重保持原样。
#[test]
fn lora_backfill_version_upgrade_appends_to_already_touched_rows() {
    let conn = memory_db();
    let id = captured_prompt(
        &conn,
        r#"[{"name":"depth-control-lora.safetensors","weight":0.55}]"#,
        false,
    );
    attach_capture(&conn, id, stack_graph(true));

    // 首次回填：补上两个栈内 LoRA，`updated_at` 也被顶掉
    assert_eq!(backfill_loras(&conn, true).unwrap(), 1);
    let first = loras_of(&conn, id);
    assert!(first.contains("KNP.safetensors"), "{first}");

    // 模拟"又改了一版识别规则"：把原列表换成一个用户手改过的版本，再跑升级那趟
    conn.execute(
        "UPDATE prompts SET loras = ?1 WHERE id = ?2",
        params![
            r#"[{"name":"depth-control-lora.safetensors","weight":0.9},{"name":"user-pick.safetensors","weight":0.4}]"#,
            id
        ],
    )
    .unwrap();
    assert_eq!(backfill_loras(&conn, false).unwrap(), 1, "升级那趟要补回来");
    let upgraded = loras_of(&conn, id);
    assert!(upgraded.contains(r#""name":"depth-control-lora.safetensors","weight":0.9"#), "已有权重不许改：{upgraded}");
    assert!(upgraded.contains("user-pick.safetensors"), "用户自己加的不许删：{upgraded}");
    assert!(upgraded.contains("hiten.safetensors"), "新版多认出来的要补上：{upgraded}");
}

/// 回填标记：第一次要跑，写完不再跑；版本被写低（或将来 bump）时要再跑一遍。
#[test]
fn lora_backfill_marker_tracks_the_version() {
    let conn = memory_db();
    assert!(loras_backfill_needed(&conn).unwrap(), "没有标记时必须跑");
    mark_loras_backfill_done(&conn).unwrap();
    assert!(!loras_backfill_needed(&conn).unwrap(), "同一版本不再重复扫");
    conn.execute(
        "UPDATE app_settings SET v = '0' WHERE k = ?1",
        [LORA_BACKFILL_KEY],
    )
    .unwrap();
    assert!(loras_backfill_needed(&conn).unwrap(), "旧版本标记必须再跑");
}

// ---------------------------------------------------------------------------
//  运行时正向提示词回填（2026-10-25）
// ---------------------------------------------------------------------------

fn positive_of(conn: &rusqlite::Connection, id: i64) -> String {
    conn.query_row("SELECT positive_prompt FROM prompts WHERE id = ?1", [id], |row| row.get(0))
        .unwrap()
}

/// 真机形状（`test_00901_.png`）：`TextGenerate → ComfySwitchNode → PreviewAny → CLIPTextEncode`。
fn runtime_prompt_raw(text: &str) -> Value {
    json!({
        "prompt": [48, "uuid", {
          "128": {"class_type":"KSampler","inputs":{"steps":8,"cfg":1,"seed":7,
                  "positive":["108",0],"negative":["133",0]}},
          "108": {"class_type":"CLIPTextEncode","inputs":{"text":["55",0],"clip":["4",1]}},
          "55":  {"class_type":"PreviewAny","inputs":{"source":["162",0]}},
          "162": {"class_type":"ComfySwitchNode","inputs":{"on_true":["43",0],"on_false":["182",0],"switch":true}},
          "43":  {"class_type":"TextGenerate","inputs":{"system_prompt":["45",0],"prompt":["182",0]}},
          "45":  {"class_type":"PrimitiveStringMultiline","inputs":{"value":"You are an expert prompt engineer."}},
          "182": {"class_type":"StringConcatenate","inputs":{"string_a":["147",0],"string_b":["180",0]}},
          "147": {"class_type":"PrimitiveStringMultiline","inputs":{"value":""}},
          "180": {"class_type":"PrimitiveStringMultiline","inputs":{"value":""}},
          "133": {"class_type":"CLIPTextEncode","inputs":{"text":""}}
        }],
        "outputs": {
          "55": {"text": [text]},
          "2": {"images": [{"filename":"test_00901_.png"}]}
        }
    })
}

/// 只补空正向的行；用户已经写过的正文字一个字都不动。
#[test]
fn prompt_backfill_fills_only_empty_positive_prompts() {
    let conn = memory_db();

    insert_empty_prompt(&conn);
    let empty = conn.last_insert_rowid();
    attach_capture(&conn, empty, runtime_prompt_raw("最终提示词：绿发少女站在雨夜街头"));

    insert_empty_prompt(&conn);
    let hand_written = conn.last_insert_rowid();
    conn.execute(
        "UPDATE prompts SET positive_prompt = '我自己写的' WHERE id = ?1",
        [hand_written],
    )
    .unwrap();
    attach_capture(&conn, hand_written, runtime_prompt_raw("运行时生成的"));

    assert_eq!(backfill_runtime_prompts(&conn).unwrap(), 1, "只补空的那一条");
    assert_eq!(positive_of(&conn, empty), "最终提示词：绿发少女站在雨夜街头");
    assert_eq!(positive_of(&conn, hand_written), "我自己写的", "手写的正向不许被覆盖");

    // 回填过的行已被顶过 `updated_at`，但判据是"正向空不空"，所以直接再跑也不会重复改
    assert_eq!(backfill_runtime_prompts(&conn).unwrap(), 0);
}

fn insert_empty_prompt(conn: &rusqlite::Connection) {
    conn.execute(
        "INSERT INTO prompts (title, kind, positive_prompt, source) \
         VALUES ('test · seed 1', 'IMAGE', '', 'ComfyUI')",
        [],
    )
    .unwrap();
}

#[test]
fn prompt_backfill_marker_tracks_the_version() {
    let conn = memory_db();
    assert!(prompt_backfill_needed(&conn).unwrap(), "没有标记时必须跑");
    mark_prompt_backfill_done(&conn).unwrap();
    assert!(!prompt_backfill_needed(&conn).unwrap(), "同一版本不再重复扫");
    conn.execute(
        "UPDATE app_settings SET v = '0' WHERE k = ?1",
        [PROMPT_BACKFILL_KEY],
    )
    .unwrap();
    assert!(prompt_backfill_needed(&conn).unwrap(), "旧版本标记必须再跑");
}