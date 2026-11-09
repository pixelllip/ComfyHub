//! `auto_select` 的单元测试（内存库 + **真 DDL**：判据就是真表上的行）。

use super::*;
use rusqlite::Connection;
use serde_json::json;

fn conn() -> Connection {
    let conn = crate::infra::db::open_memory().unwrap();
    conn.execute_batch(crate::infra::db::schema_sql())
        .expect("建 schema");
    crate::infra::db::assert_ready(&conn).unwrap();
    conn
}

/// 一份最小 t2i API 图（一个底模槽位 + 一个 SaveImage）。
fn t2i(model: &str) -> String {
    json!({
        "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": model}},
        "2": {"class_type": "CLIPTextEncode", "inputs": {"text": "old", "clip": ["1", 1]}},
        "3": {"class_type": "KSampler", "inputs": {
            "seed": 1, "steps": 8, "cfg": 1.0, "sampler_name": "euler", "scheduler": "simple",
            "denoise": 1.0, "model": ["1", 0], "positive": ["2", 0], "latent_image": ["4", 0]}},
        "4": {"class_type": "EmptyLatentImage", "inputs": {"width": 1368, "height": 960, "batch_size": 1}},
        "5": {"class_type": "VAEDecode", "inputs": {"samples": ["3", 0], "vae": ["1", 2]}},
        "6": {"class_type": "SaveImage", "inputs": {"images": ["5", 0], "filename_prefix": "t"}}
    })
    .to_string()
}

fn seed_prompt(conn: &Connection, title: &str) -> i64 {
    conn.execute(
        "INSERT INTO prompts (title, positive_prompt, source) VALUES (?1, 'x', 'ComfyUI')",
        rusqlite::params![title],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn seed_run(conn: &Connection, prompt_id: i64, model: &str, at: &str) {
    conn.execute(
        "INSERT INTO capture_runs (run_key, status, title, raw, created_at, prompt_id) \
         VALUES (?1, 'success', ?2, ?3, ?4, ?5)",
        rusqlite::params![
            format!("run-{prompt_id}-{at}"),
            format!("{model} 那次"),
            t2i(model),
            at,
            prompt_id
        ],
    )
    .unwrap();
}

fn seed_recipe(conn: &Connection, name: &str, prompt_id: i64, verified: serde_json::Value) -> i64 {
    conn.execute(
        "INSERT INTO workflow_recipes (name, task, skeleton_hash, prompt_id, verified_json) \
         VALUES (?1, 't2i', 'abc', ?2, ?3)",
        rusqlite::params![name, prompt_id, verified.to_string()],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn anima() -> Wanted {
    Wanted {
        task: Some(Task::T2i),
        family: Some("anima".to_string()),
    }
}

#[test]
fn 族名归一化_口语与文件名都能对上() {
    assert_eq!(normalize_family("Moss Image"), "mossimage");
    assert_eq!(normalize_family("krea-2"), "krea2");
    assert!(family_matches(
        "silvermoonmixAnimaEvolved_v2329B_int8.safetensors",
        "anima"
    ));
    assert!(family_matches("moss_image_t2i", "moss image"));
    assert!(!family_matches("krea2Turbo.safetensors", "anima"));
    // 族为空 = 不限
    assert!(family_matches("任意.safetensors", ""));
}

#[test]
fn 同任务同族的最近成功优先于已验证配方() {
    let conn = conn();
    let p1 = seed_prompt(&conn, "anima 9点");
    let p2 = seed_prompt(&conn, "anima 10点");
    seed_run(&conn, p1, "silvermoonmixAnimaEvolved_v2329B_int8.safetensors", "2026-11-08T09:00:00.000Z");
    seed_run(&conn, p2, "silvermoonmixAnimaEvolved_v2329B_int8.safetensors", "2026-11-08T10:00:00.000Z");
    // 配方里也有这一族（更"正式"的凭据），但①档优先
    seed_recipe(
        &conn,
        "anima 的配方",
        p1,
        json!([{"model": "silvermoonmixAnimaEvolved_v2329B_int8.safetensors", "role": "checkpoints",
                "overrideKey": "1.ckpt_name", "runKey": "r1", "at": "2026-11-08T09:00:00Z"}]),
    );

    let out = pick(&conn, &anima()).unwrap();
    let picked = out.picked.expect("应当挑到一份");
    assert_eq!(picked.source_kind, "recent");
    assert_eq!(picked.prompt_id, p2, "①档要最新的那次成功");
    assert!(picked.recipe_id.is_none());
    assert!(picked.note.contains("最近一次成功"), "{}", picked.note);
}

#[test]
fn 没有成功记录时落到已验证配方_并带出那组槽位覆盖() {
    let conn = conn();
    let p1 = seed_prompt(&conn, "krea2 的工作流");
    seed_recipe(
        &conn,
        "krea2 t2i",
        p1,
        json!([
            {"model": "krea2TurboInt8.safetensors", "role": "diffusion_models",
             "overrideKey": "55.unet_name", "runKey": "r9", "at": "2026-11-08T09:00:00Z"},
            {"model": "qwen3vl_4b.safetensors", "role": "text_encoders",
             "overrideKey": "56.clip_name", "runKey": "r9", "at": "2026-11-08T09:00:00Z"},
            // 另一次跑的记录：**不许混进来**（同一次跑的那几条共用 runKey）
            {"model": "krea2TurboInt8.safetensors", "role": "diffusion_models",
             "overrideKey": "55.unet_name", "runKey": "r1", "at": "2026-11-07T09:00:00Z"}
        ]),
    );

    let wanted = Wanted {
        task: Some(Task::T2i),
        family: Some("krea 2".to_string()),
    };
    let out = pick(&conn, &wanted).unwrap();
    let picked = out.picked.expect("应当从配方里挑到");
    assert_eq!(picked.source_kind, "recipe");
    assert_eq!(picked.prompt_id, p1);
    let mut keys: Vec<&str> = picked.overrides.iter().map(|(k, _)| k.as_str()).collect();
    keys.sort();
    assert_eq!(keys, vec!["55.unet_name", "56.clip_name"], "只带最近那次跑的覆盖");
    assert!(
        picked.note.contains("已经跑通过"),
        "结论要说清是经验不是保证：{}",
        picked.note
    );
}

#[test]
fn 没有命中时如实列出试过哪几档_绝不猜一份() {
    let conn = conn();
    let p1 = seed_prompt(&conn, "moss 的工作流");
    seed_run(&conn, p1, "moss_image_v1.safetensors", "2026-11-08T09:00:00.000Z");

    let out = pick(&conn, &anima()).unwrap();
    assert!(out.picked.is_none(), "别的族不能算命中");
    assert_eq!(out.tried.len(), 2, "两档都要留下痕迹：{:?}", out.tried);
    assert!(out.tried[0].contains("最近成功"), "{:?}", out.tried);
    assert!(out.tried[1].contains("配方"), "{:?}", out.tried);
}

#[test]
fn 任务不同不命中_族不同也不命中() {
    let conn = conn();
    let p1 = seed_prompt(&conn, "moss 的工作流");
    seed_run(&conn, p1, "moss_image_v1.safetensors", "2026-11-08T09:00:00.000Z");

    // 同族但任务不对
    let out = pick(
        &conn,
        &Wanted {
            task: Some(Task::Edit),
            family: Some("moss".into()),
        },
    )
    .unwrap();
    assert!(out.picked.is_none());

    // 同任务同族（口语写法带空格/下划线都行）
    let out = pick(
        &conn,
        &Wanted {
            task: Some(Task::T2i),
            family: Some("moss image".into()),
        },
    )
    .unwrap();
    assert_eq!(out.picked.unwrap().prompt_id, p1);
}

#[test]
fn 宽高默认一个都不改_给了才写进工作流自己的键() {
    let graph: Value = serde_json::from_str(&t2i("a.safetensors")).unwrap();
    // 手机没提尺寸：**不许**动构图（族与族的分辨率本来就不一样）
    let (overrides, note) = size_overrides(&graph, None, None);
    assert!(overrides.is_empty());
    assert!(note.is_none());

    // 明确要 832×1216：写进这份工作流自己的宽高键
    let (overrides, note) = size_overrides(&graph, Some(832), Some(1216));
    assert_eq!(
        overrides,
        vec![
            ("4.width".to_string(), "832".to_string()),
            ("4.height".to_string(), "1216".to_string())
        ]
    );
    assert!(note.is_none());

    // 这份工作流没有可覆盖的宽高：如实说，不乱写
    let no_size = json!({"1": {"class_type": "SaveImage", "inputs": {"images": []}}});
    let (overrides, note) = size_overrides(&no_size, Some(512), Some(512));
    assert!(overrides.is_empty());
    assert!(note.unwrap().contains("没有可覆盖的 width / height"));
}
