//! 产物导出的用例。
//!
//! 这一组的定位：**布局与"不越界"是用户看得见、出问题最难查的两件事**。
//!
//! 钉住四条：
//! 1. 类型目录 + 集合文件夹 + 工作流去重（同内容只落一份）；
//! 2. 单条元信息（提示词 / 集合 / 工作流引用）真的落到了 `.json` 里；
//! 3. 原始文件名带 `..` / 非法字符 / 重名 / Windows 保留名时，**一条都不许写出目标目录**；
//! 4. 文件缺失只报那一条，其余照常导出（不整批失败）。

use super::*;
use crate::infra::db::open_or_create;
use rusqlite::params;

struct Fixture {
    state: AppState,
    root: PathBuf,
    out: PathBuf,
}

fn fixture(label: &str) -> Fixture {
    let root = crate::infra::testpaths::scratch(&format!("export-{label}"));
    let media = root.join("storage").join("media");
    std::fs::create_dir_all(&media).expect("建 media 目录");
    let db = root.join("comfyhub.db");
    let conn = open_or_create(&db).expect("建测试库");
    let state = AppState::new(conn, db.display().to_string(), media.clone());
    let out = root.join("out");
    std::fs::create_dir_all(&out).expect("建导出目录");
    Fixture { state, root, out }
}

fn write_media_file(f: &Fixture, stored_name: &str, content: &[u8]) {
    std::fs::write(f.state.media_dir().join(stored_name), content).expect("写源文件");
}

fn exec(f: &Fixture, sql: &str, values: impl rusqlite::Params) {
    f.state
        .with(|conn| {
            conn.execute(sql, values)?;
            Ok(())
        })
        .expect("灌测试数据");
}

fn run_export(f: &Fixture, out: &Path, req: ExportRequest) -> Result<ExportReport, ApiError> {
    let mut req = req;
    req.dir = out.display().to_string();
    export(&f.state, req)
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} 不是合法 JSON：{e}", path.display()))
}

fn subdir(f: &Fixture, name: &str) -> PathBuf {
    let dir = f.out.join(name);
    std::fs::create_dir_all(&dir).expect("建子目录");
    dir
}

#[test]
fn 按类型分目录_集合单独成夹_工作流去重并带上元信息() {
    let f = fixture("layout");
    exec(
        &f,
        "INSERT INTO prompts (id, title, kind, positive_prompt, negative_prompt, workflow_json)
         VALUES (10, '赛博朋克少女', 'IMAGE', '1girl, cyberpunk', 'lowres', '{\"nodes\":[1]}')",
        [],
    );
    exec(
        &f,
        "INSERT INTO media_collections (id, group_key, kind, origin, title, rule_version)
         VALUES (7, 'k-layout', 'IMAGE', 'manual', '夜景三连', 1)",
        [],
    );
    // 散图：工作流挂在提示词上（老数据只有这一份）→ 必须回落到 prompt.workflow_json
    exec(
        &f,
        "INSERT INTO media_assets (id, prompt_id, kind, title, original_name, stored_name, size_bytes)
         VALUES (100, 10, 'IMAGE', '单图', 'solo.png', 'solo.png', 3)",
        [],
    );
    // 集合成员：工作流挂在产物上，两条一样 → 只该落一份
    for (id, name) in [(101, "a.png"), (102, "b.png")] {
        exec(
            &f,
            "INSERT INTO media_assets
               (id, prompt_id, collection_id, kind, title, original_name, stored_name,
                size_bytes, source_ref, workflow_json)
             VALUES (?1, 10, 7, 'IMAGE', ?2, ?2, ?2, 3, 'run:layout', '{\"nodes\":[2]}')",
            params![id, name],
        );
        write_media_file(&f, name, b"img");
    }
    exec(
        &f,
        "INSERT INTO media_assets (id, kind, title, original_name, stored_name, size_bytes)
         VALUES (103, 'VIDEO', '视频', 'clip.mp4', 'clip.mp4', 3)",
        [],
    );
    exec(
        &f,
        "INSERT INTO media_assets (id, kind, title, original_name, stored_name, size_bytes)
         VALUES (104, 'AUDIO', '音频', 'track.mp3', 'track.mp3', 3)",
        [],
    );
    exec(
        &f,
        "INSERT INTO capture_runs (run_key, status, media_count, raw)
         VALUES ('run:layout', 'success', 2,
                 '{\"prompt\":{\"1\":{\"class_type\":\"KSampler\",\"inputs\":{}}}}')",
        [],
    );
    write_media_file(&f, "solo.png", b"img");
    write_media_file(&f, "clip.mp4", b"vid");
    write_media_file(&f, "track.mp3", b"aud");

    let report = run_export(&f, &f.out, ExportRequest {
        dir: String::new(),
        media_ids: None,
        collection_id: None,
        favorites: false,
        include_workflow: true,
        include_api_workflow: true,
    })
    .expect("导出");

    assert_eq!(report.exported, 5, "3 张图 + 1 视频 + 1 音频");
    assert_eq!(report.collections, 1, "只有 7 号集合");
    assert_eq!(report.workflows, 3, "两份 UI 工作流 + 一份 API 图（同内容去重）");
    assert_eq!(report.failed.len(), 0, "不该有失败：{report:?}");
    assert_eq!(report.by_kind.get("IMAGE"), Some(&3));
    assert_eq!(report.by_kind.get("VIDEO"), Some(&1));
    assert_eq!(report.by_kind.get("AUDIO"), Some(&1));

    // 布局
    assert!(f.out.join("图片/solo.png").is_file(), "散图直接放在类型目录下");
    assert!(f.out.join("图片/夜景三连/a.png").is_file(), "集合成员进集合文件夹");
    assert!(f.out.join("图片/夜景三连/b.png").is_file());
    assert!(f.out.join("视频/clip.mp4").is_file());
    assert!(f.out.join("音频/track.mp3").is_file());

    // 散图侧车：提示词 / 工作流都指向正确来源
    let solo_meta = read_json(&f.out.join("图片/solo.png.json"));
    assert_eq!(solo_meta["media"]["id"], 100);
    assert_eq!(solo_meta["media"]["file"], "图片/solo.png");
    assert_eq!(solo_meta["prompt"]["positivePrompt"], "1girl, cyberpunk");
    assert_eq!(solo_meta["prompt"]["negativePrompt"], "lowres");
    assert_eq!(solo_meta["workflow"]["source"], "prompt");
    let solo_workflow = solo_meta["workflow"]["file"].as_str().expect("工作流路径");
    assert!(
        f.out.join(solo_workflow).is_file(),
        "侧车引用的工作流必须真的落盘：{solo_workflow}"
    );

    // 集合成员：工作流来自产物本身；集合文件夹里有一份 collection.json
    let member_meta = read_json(&f.out.join("图片/夜景三连/a.png.json"));
    assert_eq!(member_meta["media"]["id"], 101);
    assert_eq!(member_meta["collection"]["id"], 7);
    assert_eq!(member_meta["collection"]["title"], "夜景三连");
    assert_eq!(member_meta["workflow"]["source"], "media");
    assert!(member_meta["apiWorkflow"]["file"].as_str().is_some(), "有捕获记录就该带 API 图");
    let collection = read_json(&f.out.join("图片/夜景三连/collection.json"));
    assert_eq!(collection["id"], 7);
    assert_eq!(collection["title"], "夜景三连");
    assert_eq!(collection["members"].as_array().unwrap().len(), 2);
    assert_eq!(collection["members"][0]["mediaId"], 101, "成员顺序按 created_at / id");
    assert_eq!(collection["members"][1]["mediaId"], 102);

    // 工作流目录：UI 两份 + API 一份
    let workflow_files = std::fs::read_dir(f.out.join("工作流"))
        .expect("工作流目录")
        .filter_map(|e| e.ok())
        .count();
    assert_eq!(workflow_files, 3, "同内容只落一份");

    // 总清单
    let manifest = read_json(&f.out.join("导出清单.json"));
    assert_eq!(manifest["schemaVersion"], EXPORT_SCHEMA_VERSION);
    assert_eq!(manifest["counts"]["exported"], 5);
    assert_eq!(manifest["counts"]["collections"], 1);
    assert_eq!(manifest["counts"]["workflows"], 3);
    assert_eq!(manifest["items"].as_array().unwrap().len(), 5);
    assert_eq!(manifest["collections"][0]["folder"], "图片/夜景三连");
    assert_eq!(manifest["collections"][0]["members"], 2);
    assert!(manifest["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["file"].as_str().unwrap().contains('/')));
}

#[test]
fn 原始文件名里的非法字符与重名一律净化且不出目标目录() {
    let f = fixture("names");
    let rows = [
        (1, "../evil:name?.png", "e1.png"),
        (2, "same.png", "s1.png"),
        (3, "same.png", "s2.png"),
        (4, "CON.png", "c1.png"),
    ];
    for (id, original, stored) in rows {
        exec(
            &f,
            "INSERT INTO media_assets (id, kind, title, original_name, stored_name, size_bytes)
             VALUES (?1, 'IMAGE', ?2, ?2, ?3, 3)",
            params![id, original, stored],
        );
        write_media_file(&f, stored, b"img");
    }

    let report = run_export(&f, &f.out, ExportRequest {
        dir: String::new(),
        media_ids: None,
        collection_id: None,
        favorites: false,
        include_workflow: true,
        include_api_workflow: false,
    })
    .expect("导出");
    assert_eq!(report.exported, 4);

    let names: Vec<String> = std::fs::read_dir(f.out.join("图片"))
        .expect("图片目录")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names.len(), 8, "4 份原件 + 4 份侧车：{names:?}");
    assert!(
        names.iter().all(|n| !n.contains('/') && !n.contains('\\') && !n.starts_with("..")),
        "净化后的文件名不许带路径分隔符、也不许以 `..` 开头：{names:?}"
    );
    assert!(names.iter().any(|n| n == "_evil_name_.png"), "{names:?}");
    assert!(names.iter().any(|n| n.starts_with("same_")), "重名必须加后缀：{names:?}");
    assert!(names.iter().any(|n| n == "_CON.png"), "Windows 保留名要被避开：{names:?}");

    // 真正的验收：目标目录之外一个文件都没多出来。
    assert!(!f.root.join("evil_name_.png").exists());
    assert!(!f.root.join("图片/evil_name_.png").exists());
}

#[test]
fn 老数据里的_API_图不许被标成界面格式() {
    let f = fixture("api-format");
    exec(
        &f,
        "INSERT INTO media_assets
           (id, kind, title, original_name, stored_name, size_bytes, workflow_json)
         VALUES (1, 'IMAGE', 'x', 'x.png', 'x.png', 3,
                 '{\"1\":{\"class_type\":\"KSampler\",\"inputs\":{}}}')",
        [],
    );
    write_media_file(&f, "x.png", b"img");
    run_export(
        &f,
        &f.out,
        ExportRequest {
            dir: String::new(),
            media_ids: None,
            collection_id: None,
            favorites: false,
            include_workflow: true,
            include_api_workflow: false,
        },
    )
    .expect("导出");
    let meta = read_json(&f.out.join("图片/x.png.json"));
    assert_eq!(
        meta["workflow"]["format"], "comfyui-api",
        "API 图拖不进 ComfyUI 的界面，标错格式会让人白试一次"
    );
    let file = meta["workflow"]["file"].as_str().unwrap();
    assert!(file.ends_with(".api.json"), "{file}");
}

#[test]
fn 文件缺失只报那一条_其余照常导出() {
    let f = fixture("missing");
    exec(
        &f,
        "INSERT INTO media_assets (id, kind, title, original_name, stored_name, size_bytes)
         VALUES (1, 'IMAGE', '在', 'ok.png', 'ok.png', 3),
                (2, 'IMAGE', '不在', 'gone.png', 'gone.png', 3)",
        [],
    );
    write_media_file(&f, "ok.png", b"img");

    let report = run_export(&f, &f.out, ExportRequest {
        dir: String::new(),
        media_ids: None,
        collection_id: None,
        favorites: false,
        include_workflow: false,
        include_api_workflow: false,
    })
    .expect("导出");
    assert_eq!(report.exported, 1);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].media_id, Some(2));
    assert!(report.failed[0].reason.contains("文件不在盘上"), "{report:?}");
    assert!(f.out.join("图片/ok.png").is_file());

    let manifest = read_json(&f.out.join("导出清单.json"));
    assert_eq!(manifest["counts"]["failed"], 1);
    assert_eq!(manifest["failed"][0]["mediaId"], 2);
}

#[test]
fn 范围_收藏_选中_集合与空选中() {
    let f = fixture("scopes");
    exec(
        &f,
        "INSERT INTO media_collections (id, group_key, kind, origin, rule_version)
         VALUES (7, 'k-scope', 'IMAGE', 'auto', 1)",
        [],
    );
    for (id, favorite, collection) in [(1, 1, None), (2, 0, Some(7)), (3, 0, Some(7))] {
        exec(
            &f,
            "INSERT INTO media_assets
               (id, collection_id, kind, title, original_name, stored_name, size_bytes, favorite)
             VALUES (?1, ?2, 'IMAGE', ?3, ?3, ?3, 3, ?4)",
            params![id, collection, format!("m{id}.png"), favorite],
        );
        write_media_file(&f, &format!("m{id}.png"), b"img");
    }

    let by_favorite = run_export(&f, &subdir(&f, "favorite"), ExportRequest {
        dir: String::new(),
        media_ids: None,
        collection_id: None,
        favorites: true,
        include_workflow: false,
        include_api_workflow: false,
    })
    .expect("仅收藏");
    assert_eq!(by_favorite.exported, 1);

    let by_ids = run_export(&f, &subdir(&f, "ids"), ExportRequest {
        dir: String::new(),
        media_ids: Some(vec![2]),
        collection_id: None,
        favorites: false,
        include_workflow: false,
        include_api_workflow: false,
    })
    .expect("选中");
    assert_eq!(by_ids.exported, 1);

    let by_collection = run_export(&f, &subdir(&f, "collection"), ExportRequest {
        dir: String::new(),
        media_ids: None,
        collection_id: Some(7),
        favorites: false,
        include_workflow: false,
        include_api_workflow: false,
    })
    .expect("集合");
    assert_eq!(by_collection.exported, 2);
    assert_eq!(by_collection.collections, 1);

    // `Some([])` = 用户确实选了 0 条，**不许**退化成"全部"
    let empty = run_export(&f, &subdir(&f, "empty"), ExportRequest {
        dir: String::new(),
        media_ids: Some(Vec::new()),
        collection_id: None,
        favorites: false,
        include_workflow: false,
        include_api_workflow: false,
    })
    .expect("空选中");
    assert_eq!(empty.exported, 0);
    assert!(f.out.join("empty/导出清单.json").is_file(), "空导出也要有清单");
}

#[test]
fn 不许导出到自己的存储目录() {
    let f = fixture("guard");
    let err = run_export(&f, f.state.media_dir(), ExportRequest {
        dir: String::new(),
        media_ids: None,
        collection_id: None,
        favorites: false,
        include_workflow: false,
        include_api_workflow: false,
    })
    .expect_err("必须拒绝");
    assert!(err.human().contains("存储目录"), "{}", err.human());

    let missing = f.root.join("does-not-exist");
    let err = run_export(&f, &missing, ExportRequest {
        dir: String::new(),
        media_ids: None,
        collection_id: None,
        favorites: false,
        include_workflow: false,
        include_api_workflow: false,
    })
    .expect_err("目标不存在必须拒绝");
    assert!(err.human().contains("不存在") || err.human().contains("不可用"), "{}", err.human());
}
