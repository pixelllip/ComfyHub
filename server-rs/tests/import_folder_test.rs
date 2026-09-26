//! `POST /api/capture/import`（**A3 的另一半**）的回归防线 —— 把"盘上已经生成好的产物"
//! 导进画廊，并**用 PNG 内嵌的 prompt 元数据还原出提示词**。
//!
//! # 这一组在防什么
//!
//! 这条路是用户"以前跑的一堆图"进库的唯一入口。三件事错了都不会报错、只会**少东西**：
//!  1. **PNG 内嵌元数据没读出来** → 图进来了，但只是一张没有来历的图（没有提示词、没有参数）；
//!  2. **判重失效** → 同一个目录导两次，画廊里多一份；
//!  3. **把自己 storage 里的文件又导一遍** → 每导一次多几份副本（指数增长）。
//!
//! 夹具用 `png` crate 写**带真 tEXt 文本块**的 PNG（ComfyUI 就是这么写的），
//! 所以"读内嵌图 → 还原参数"这条路是真的被走了一遍。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use rusqlite::Connection;
use serde_json::{json, Value};
use tower::ServiceExt;

use comfyhub_server_rs::routes::{router, AppState};

fn temp_dir(label: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("comfyhub-import-{label}-{nanos}"));
    std::fs::create_dir_all(dir.join("storage").join("media")).unwrap();
    dir
}

fn state(dir: &std::path::Path) -> AppState {
    let conn = Connection::open_in_memory().unwrap();
    let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("db")
        .join("schema.sqlite.sql");
    let schema = std::fs::read_to_string(&schema_path).expect("读 schema");
    conn.execute_batch(&schema).expect("建 schema");
    AppState::new(conn, ":memory:", dir.join("storage").join("media"))
}

/// 写一张真 PNG（可选带 `prompt` / `workflow` 文本块 —— ComfyUI 就是这么写的）。
///
/// ⚠ `variant` 必须每张图不同：入库按**内容 sha256** 判重，像素一样就会被当成"同一份文件"。
fn write_png(path: &std::path::Path, variant: u8, texts: &[(&str, &str)]) {
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 8, 6);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    for (key, value) in texts {
        encoder
            .add_text_chunk(key.to_string(), value.to_string())
            .unwrap();
    }
    let mut writer = encoder.write_header().unwrap();
    let data: Vec<u8> = (0..(8 * 6 * 3))
        .map(|i| ((i + variant as usize * 7) % 251) as u8)
        .collect();
    writer.write_image_data(&data).unwrap();
    writer.finish().unwrap();
}

/// 一份**最小的 API 格式节点图**（`graph_parse` 能解析出提示词 / seed / 宽度）。
fn api_graph() -> String {
    json!({
        "3": {
            "class_type": "KSampler",
            "inputs": { "seed": 20260926, "steps": 8, "cfg": 1.0, "denoise": 1.0, "model": ["4", 0] }
        },
        "4": {
            "class_type": "CheckpointLoaderSimple",
            "inputs": { "ckpt_name": "krea2.safetensors" }
        },
        "6": { "class_type": "CLIPTextEncode", "inputs": { "text": "imported cat, masterpiece", "clip": ["4", 1] } },
        "9": { "class_type": "SaveImage", "inputs": { "filename_prefix": "imported_probe", "images": ["3", 0] } }
    })
    .to_string()
}

async fn post_json(state: &AppState, path: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn 导入目录_用_png_内嵌的_prompt_还原出提示词() {
    let dir = temp_dir("happy");
    let source = dir.join("in");
    std::fs::create_dir_all(&source).unwrap();
    // ① 带 prompt + workflow 元数据的那张（ComfyUI 的常态）
    write_png(
        &source.join("with_meta.png"),
        1,
        &[
            ("prompt", &api_graph()),
            ("workflow", "{\"nodes\":[],\"links\":[]}"),
        ],
    );
    // ② 没有元数据的普通图（照样要能进画廊，只是不会建提示词）
    write_png(&source.join("plain.png"), 2, &[]);
    // ③ 非媒体文件：不该被扫到
    std::fs::write(source.join("notes.txt"), b"hello").unwrap();
    // ④ 子目录里的图（recursive=true 时也要收）
    let nested = source.join("sub");
    std::fs::create_dir_all(&nested).unwrap();
    write_png(&nested.join("nested.png"), 3, &[]);

    let state = state(&dir);
    let (status, body) = post_json(&state, "/api/capture/import", json!({ "dir": source.display().to_string() })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["scanned"], json!(3), "三张图都要被扫到（txt 不算）：{body}");
    assert_eq!(body["imported"], json!(3), "{body}");
    assert_eq!(body["duplicates"], json!(0), "{body}");
    assert_eq!(body["failed"], json!(0), "{body}");
    assert_eq!(body["promptsCreated"], json!(1), "只有带元数据的那张建提示词：{body}");
    let prompt_ids = body["promptIds"].as_array().unwrap();
    assert_eq!(prompt_ids.len(), 1, "{body}");

    // 提示词里的参数真的来自内嵌的图（不是猜的）
    let (title, seed, positive): (String, Option<i64>, String) = state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT title, seed, positive_prompt FROM prompts WHERE id = ?1",
                rusqlite::params![prompt_ids[0].as_i64().unwrap()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        })
        .unwrap();
    assert_eq!(positive, "imported cat, masterpiece");
    assert_eq!(seed, Some(20260926));
    assert!(title.contains("seed 20260926"), "标题要带 seed：{title}");

    // 产物挂在那条提示词上（画廊里点进去能看到参数）
    let linked: i64 = state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM media_assets WHERE prompt_id = ?1",
                rusqlite::params![prompt_ids[0].as_i64().unwrap()],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(linked, 1, "带元数据那张要关联到提示词");

    // ⑤ 再导一次：同一份内容不许重复入库
    let (_, again) = post_json(&state, "/api/capture/import", json!({ "dir": source.display().to_string() })).await;
    assert_eq!(again["imported"], json!(0), "{again}");
    assert_eq!(again["duplicates"], json!(3), "{again}");
    let media_count: i64 = state
        .with(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM media_assets", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(media_count, 3, "导两次仍然只有三份产物");

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn 不递归_与_自己的_storage_都不许被导入() {
    let dir = temp_dir("scope");
    let source = dir.join("in");
    let nested = source.join("sub");
    std::fs::create_dir_all(&nested).unwrap();
    write_png(&source.join("top.png"), 4, &[]);
    write_png(&nested.join("deep.png"), 5, &[]);
    // 自己的 storage 里也放一张：绝不能被"再导一遍"
    write_png(&dir.join("storage").join("media").join("mine.png"), 6, &[]);

    let state = state(&dir);
    let (status, body) = post_json(
        &state,
        "/api/capture/import",
        json!({ "dir": source.display().to_string(), "recursive": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["scanned"], json!(1), "不递归时只看顶层：{body}");

    // 直接对着 storage 目录导：里面的文件要全被跳过（scanned=0）
    let (status, body) = post_json(
        &state,
        "/api/capture/import",
        json!({ "dir": dir.join("storage").display().to_string() }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["scanned"], json!(0), "自己的 storage 一个都不许导：{body}");

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn 目录不存在与缺_dir_都是人话_400() {
    let dir = temp_dir("errors");
    let state = state(&dir);
    let (status, body) = post_json(&state, "/api/capture/import", json!({ "dir": "D:\\definitely-not-here" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("目录不存在"), "{body}");

    let (status, body) = post_json(&state, "/api/capture/import", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], json!("dir 不能为空"));

    let _ = std::fs::remove_dir_all(dir);
}
