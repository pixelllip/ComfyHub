//! Rust 端点的形状与纪律用例（P4 灰度第一批）。
//!
//! # 这一组用例在防什么
//!
//! P4 的验收前提是"**Flutter 一行不改**"，而 Flutter 读的是 JSON 字段名。
//! 字段写错**不会报错**：界面那个字段就是空的（比如把 `positivePrompt` 写成 `positive_prompt`，
//! 提示词详情页的正文会整段消失）。所以：
//!
//! 1. 每个 DTO 都断言**精确的键集合**（不是"包含某几个键"）——
//!    这样"少一个键"和"多一个键"都会红；
//! 2. 断言 Rust 与 Kotlin 的**分页信封**一致（`items/total/page/size/pages`）；
//! 3. R1 / R2 / R14 各有一条（这些防线在 Rust 侧同样要重建，总纲 §6）。
//!
//! 数据是**测试自己灌的**（内存库 + 真 schema），不依赖机器上那份 `comfyhub.db`。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use rusqlite::Connection;
use serde_json::Value;
use tower::ServiceExt;

/// 用 `db/schema.sqlite.sql`（**唯一真源**）建库，再灌一点测试数据。
fn test_conn() -> Connection {
    let conn = comfyhub_server_rs::db::open_memory().expect("内存库");
    let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("db")
        .join("schema.sqlite.sql");
    let schema = std::fs::read_to_string(&schema_path)
        .unwrap_or_else(|e| panic!("读不到 {}：{e}", schema_path.display()));
    conn.execute_batch(&schema).expect("建 schema");
    conn.execute_batch(
        r#"
        INSERT INTO tags (id, name, normalized, category, color, use_count, created_at)
        VALUES (1, '赛博朋克', '赛博朋克', '风格', '#ff00ff', 5, '2026-09-23T00:00:00.000Z'),
               (2, '国风', '国风', '风格', NULL, 9, '2026-09-23T00:00:00.000Z');

        INSERT INTO prompts (id, title, kind, positive_prompt, negative_prompt, checkpoint,
                             loras, cfg_scale, seed, favorite, source, workflow_json,
                             created_at, updated_at)
        VALUES (10, '赛博朋克少女', 'IMAGE', '1girl, cyberpunk', 'lowres', 'krea2.safetensors',
                '[{"name":"anima","weight":0.8}]', 3.5, 42, 1, 'Manual', '{"nodes":[]}',
                '2026-09-23T01:00:00.000Z', '2026-09-23T01:00:00.000Z'),
               (11, '国风山水', 'IMAGE', 'chinese ink painting', NULL, NULL, NULL, NULL, NULL, 0,
                'Manual', NULL, '2026-09-23T02:00:00.000Z', '2026-09-23T02:00:00.000Z');

        INSERT INTO prompt_tags (prompt_id, tag_id) VALUES (10, 1), (11, 2);

        INSERT INTO media_assets (id, prompt_id, kind, original_name, stored_name, size_bytes)
        VALUES (100, 10, 'IMAGE', 'a.png', 'a.png', 1234);
        "#,
    )
    .expect("灌测试数据");
    conn
}

fn app() -> axum::Router {
    comfyhub_server_rs::routes::router(comfyhub_server_rs::routes::AppState::new(
        test_conn(),
        ":memory:",
    ))
}

async fn get_json(app: axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .expect("请求");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

fn keys(value: &Value) -> Vec<String> {
    let mut out: Vec<String> = value
        .as_object()
        .expect("对象")
        .keys()
        .cloned()
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn health_reports_engine_and_schema_version() {
    let (status, body) = get_json(app(), "/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["impl"], "rust");
    assert_eq!(body["schemaVersion"], 1, "P2 建库时写进 user_version 的数");
    assert_eq!(body["engine"]["fts5"], true, "R14：bundled 引擎必须带 FTS5");
    assert!(
        body["engine"]["sqliteVersion"].as_str().unwrap().starts_with("3."),
        "engine={}",
        body["engine"]
    );
}

#[tokio::test]
async fn tags_use_the_same_keys_as_the_kotlin_server() {
    let (status, body) = get_json(app(), "/api/tags").await;
    assert_eq!(status, StatusCode::OK);
    let items = body.as_array().unwrap();
    assert_eq!(items.len(), 2);
    // popular：使用次数降序
    assert_eq!(items[0]["name"], "国风");
    assert_eq!(
        keys(&items[0]),
        vec![
            "category", "color", "createdAt", "description", "id", "name", "normalized", "useCount"
        ],
        "Tag 的键必须与 Kotlin / Dart 一致（Dart 读 useCount / createdAt）"
    );
    assert_eq!(items[0]["useCount"], 9);
}

#[tokio::test]
async fn tag_filters_and_categories() {
    let (status, body) = get_json(app(), "/api/tags?q=%E8%B5%9B%E5%8D%9A").await; // q=赛博
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], 1);

    let (status, body) = get_json(app(), "/api/tags?category=%E9%A3%8E%E6%A0%BC").await; // category=风格
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 2);

    let (status, body) = get_json(app(), "/api/tags?sort=name").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["name"], "国风", "name 排序：按名字");

    let (status, body) = get_json(app(), "/api/tags/categories").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!(["风格"]));
}

#[tokio::test]
async fn prompts_paged_envelope_matches_dart_paged() {
    let (status, body) = get_json(app(), "/api/prompts?page=1&size=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&body), vec!["items", "page", "pages", "size", "total"]);
    assert_eq!(body["total"], 2);
    assert_eq!(body["page"], 1);
    assert_eq!(body["size"], 1);
    assert_eq!(body["pages"], 2);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    // newest：created_at 降序
    assert_eq!(body["items"][0]["title"], "国风山水");
}

#[tokio::test]
async fn prompt_object_has_every_key_dart_reads() {
    let (status, body) = get_json(app(), "/api/prompts/10").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&body),
        vec![
            "batchSize", "cfgScale", "checkpoint", "createdAt", "extraParams", "favorite",
            "hasWorkflow", "height", "id", "kind", "loras", "mediaCount", "negativePrompt",
            "notes", "positivePrompt", "sampler", "scheduler", "seed", "source", "sourceRef",
            "steps", "tags", "title", "updatedAt", "width",
        ],
        "少一个键 = 界面上少一块内容"
    );
    assert_eq!(body["positivePrompt"], "1girl, cyberpunk");
    assert_eq!(body["cfgScale"], 3.5);
    assert_eq!(body["favorite"], true, "favorite 是布尔（库里是 0/1）");
    assert_eq!(body["hasWorkflow"], true, "接口只给'有没有'，不给整份 workflow_json");
    assert_eq!(body["loras"][0]["name"], "anima");
    assert_eq!(body["loras"][0]["weight"], 0.8);
    assert_eq!(body["extraParams"], serde_json::json!({}), "NULL 当空对象");
    assert_eq!(body["tags"][0]["name"], "赛博朋克");
    assert_eq!(body["mediaCount"], 1);
}

#[tokio::test]
async fn prompt_search_and_favorite_and_missing() {
    let (status, body) = get_json(app(), "/api/prompts?q=cyberpunk").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["id"], 10);

    let (status, body) = get_json(app(), "/api/prompts?favorite=true").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["id"], 10);

    let (status, body) = get_json(app(), "/api/prompts?kind=VIDEO").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 0);
    assert_eq!(body["items"], serde_json::json!([]));

    let (status, body) = get_json(app(), "/api/prompts/999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(keys(&body), vec!["error", "message"]);
    assert!(body["message"].as_str().unwrap().contains("999"));
}

#[tokio::test]
async fn foreign_keys_cascade_is_really_on() {
    // R1：删标签要清掉关联行（`ON DELETE CASCADE`）—— 这条只有在 foreign_keys=ON 时才成立
    let conn = test_conn();
    conn.execute("DELETE FROM tags WHERE id = 1", []).unwrap();
    let left: i64 = conn
        .query_row("SELECT COUNT(*) FROM prompt_tags WHERE tag_id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0, "R1 没生效：删标签留下了孤儿关联行");
}
