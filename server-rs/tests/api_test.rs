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
    router_for(state())
}

/// 共享状态的库（写端点用例要在**同一条连接**上断言写完之后的样子）。
///
/// 顺带把产物目录准备好：夹具里 `media_assets.id=100` 的 `stored_name` 是 `a.png`，
/// 文件流端点要真的能在磁盘上找到它（内容已知，便于断言 Range 的字节）。
fn state() -> comfyhub_server_rs::routes::AppState {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("comfyhub-test-{}-{unique}", std::process::id()));
    let media = root.join("media");
    std::fs::create_dir_all(&media).unwrap();
    std::fs::write(media.join("a.png"), b"0123456789abcdef").unwrap();
    comfyhub_server_rs::routes::AppState::new(test_conn(), ":memory:", media)
}

fn router_for(state: comfyhub_server_rs::routes::AppState) -> axum::Router {
    comfyhub_server_rs::routes::router(state)
}

/// 发一个带 JSON body 的请求（写端点用）。
async fn send_json(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.expect("请求");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn send_empty(app: axum::Router, method: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.expect("请求");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
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

// ===========================================================================
//  写端点（P4 第 2 步）
// ===========================================================================

/// 建标签 → 201 + TagDto；同名（忽略大小写）**不重复建**，返回同一个 id。
#[tokio::test]
async fn post_tags_is_idempotent_by_normalized_name() {
    let state = state();
    let first = send_json(
        router_for(state.clone()),
        "POST",
        "/api/tags",
        serde_json::json!({"name": "赛博朋克", "category": "风格", "color": "#ff00ff"}),
    )
    .await;
    assert_eq!(first.0, StatusCode::CREATED, "body={}", first.1);
    assert_eq!(first.1["name"], "赛博朋克");
    let id = first.1["id"].as_i64().unwrap();

    // 第二次用不同大小写/空格：normalized 相同 → 还是那一条
    let again = send_json(
        router_for(state.clone()),
        "POST",
        "/api/tags",
        serde_json::json!({"name": " 赛博朋克 "}),
    )
    .await;
    assert_eq!(again.0, StatusCode::CREATED);
    assert_eq!(again.1["id"].as_i64().unwrap(), id, "同名标签不许建出第二条");

    let count: i64 = state
        .with(|conn| conn.query_row("SELECT COUNT(*) FROM tags WHERE normalized = '赛博朋克'", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn post_tags_with_blank_name_is_400() {
    let (status, body) = send_json(
        router_for(state()),
        "POST",
        "/api/tags",
        serde_json::json!({"name": "   "}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "BAD_REQUEST");
}

/// 改名撞上另一个标签 → 400（**不是**静默合并成一条）。
#[tokio::test]
async fn put_tag_conflict_is_400_and_missing_is_404() {
    let state = state();
    let a = send_json(router_for(state.clone()), "POST", "/api/tags", serde_json::json!({"name": "国风"})).await;
    let b = send_json(router_for(state.clone()), "POST", "/api/tags", serde_json::json!({"name": "赛博"})).await;
    let (id_a, id_b) = (a.1["id"].as_i64().unwrap(), b.1["id"].as_i64().unwrap());

    let clash = send_json(
        router_for(state.clone()),
        "PUT",
        &format!("/api/tags/{id_b}"),
        serde_json::json!({"name": "国风", "description": "画风"}),
    )
    .await;
    assert_eq!(clash.0, StatusCode::BAD_REQUEST);
    assert!(clash.1["message"].as_str().unwrap().contains("已存在"), "body={}", clash.1);

    let renamed = send_json(
        router_for(state.clone()),
        "PUT",
        &format!("/api/tags/{id_a}"),
        serde_json::json!({"name": "国风画", "category": "风格"}),
    )
    .await;
    assert_eq!(renamed.0, StatusCode::OK);
    assert_eq!(renamed.1["name"], "国风画");

    let missing = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/tags/9999",
        serde_json::json!({"name": "随便"}),
    )
    .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_tag_returns_deleted_and_cascades() {
    let state = state();
    let (status, body) = send_empty(router_for(state.clone()), "DELETE", "/api/tags/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"deleted": 1}));

    // R1：关联行被级联删掉
    let left: i64 = state
        .with(|conn| conn.query_row("SELECT COUNT(*) FROM prompt_tags WHERE tag_id = 1", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(left, 0);

    let again = send_empty(router_for(state.clone()), "DELETE", "/api/tags/1").await;
    assert_eq!(again.0, StatusCode::NOT_FOUND, "删第二次要 404，不许假装成功");
}

/// 建提示词：自动起名 + 标签自动创建 + `use_count` 重算 + **同步账本被点亮**。
#[tokio::test]
async fn post_prompts_creates_with_tags_and_writes_sync_log() {
    let state = state();
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/prompts",
        serde_json::json!({
            "positivePrompt": "1girl, cyberpunk city",
            "titles": "前端多带一个无用键不该炸",
            "cfgScale": 4.5,
            "tags": ["新标签A", "新标签B", "新标签A"],
            "extraParams": {"steps": "30"}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body={body}");
    assert_eq!(body["title"], "1girl", "标题为空时按正文第一段自动起名");
    assert_eq!(body["cfgScale"], 4.5);
    assert_eq!(body["extraParams"], serde_json::json!({"steps": "30"}));
    let new_id = body["id"].as_i64().unwrap();

    // 标签自动建出来并去重（"新标签A" 说两遍只关联一次）
    let tag_names: Vec<String> = body["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(tag_names, vec!["新标签A", "新标签B"]);
    let use_count: i64 = state
        .with(|conn| conn.query_row("SELECT use_count FROM tags WHERE name = '新标签A'", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(use_count, 1, "use_count 必须重算");

    // 同步账本：这条写操作要能被手机看到（触发器自动写的，不需要额外代码）
    let logged: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM _sync_log WHERE tbl = 'prompts' AND pk = ?1 AND op = 'upsert'",
                rusqlite::params![new_id.to_string()],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(logged, 1, "写提示词必须点亮 _sync_log（同步契约 §2）");
}

#[tokio::test]
async fn post_prompts_blank_is_400() {
    let (status, body) = send_json(
        router_for(state()),
        "POST",
        "/api/prompts",
        serde_json::json!({"title": "  ", "positivePrompt": ""}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("不能为空"));
}

#[tokio::test]
async fn put_prompt_replaces_tags_and_missing_is_404() {
    let state = state();
    let updated = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/prompts/10",
        serde_json::json!({
            "title": "改过的标题",
            "kind": "VIDEO",
            "positivePrompt": "1girl, cyberpunk",
            "tags": ["只留这一个"]
        }),
    )
    .await;
    assert_eq!(updated.0, StatusCode::OK, "body={}", updated.1);
    assert_eq!(updated.1["title"], "改过的标题");
    assert_eq!(updated.1["kind"], "VIDEO");
    let tags: Vec<String> = updated.1["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(tags, vec!["只留这一个"], "PUT 是整体替换标签");

    let missing = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/prompts/9999",
        serde_json::json!({"positivePrompt": "x"}),
    )
    .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);

    // 非法 kind 回落到 IMAGE（与 Kotlin 一致），而不是 400
    let fallback = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/prompts/11",
        serde_json::json!({"positivePrompt": "x", "kind": "NOPE"}),
    )
    .await;
    assert_eq!(fallback.1["kind"], "IMAGE");
}

/// 收藏回执是 `{id, favorite}` 两个键（Kotlin 侧踩过"混类型 map → 500"的坑）。
#[tokio::test]
async fn favorite_returns_id_and_flag() {
    let (status, body) = send_json(
        router_for(state()),
        "POST",
        "/api/prompts/11/favorite",
        serde_json::json!({"favorite": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&body), vec!["favorite", "id"]);
    assert_eq!(body["id"], 11);
    assert_eq!(body["favorite"], true);

    let missing = send_json(
        router_for(state()),
        "POST",
        "/api/prompts/9999/favorite",
        serde_json::json!({"favorite": true}),
    )
    .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn duplicate_creates_a_copy_and_keeps_tags() {
    let state = state();
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/prompts/10/duplicate",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body={body}");
    assert_ne!(body["id"].as_i64().unwrap(), 10);
    assert_eq!(body["title"], "赛博朋克少女 副本");
    assert_eq!(body["favorite"], false, "副本不继承收藏状态（与 Kotlin 一致）");
    assert_eq!(body["positivePrompt"], "1girl, cyberpunk");
    assert_eq!(body["loras"][0]["name"], "anima");
    assert_eq!(body["tags"][0]["name"], "赛博朋克");

    let missing = send_json(
        router_for(state.clone()),
        "POST",
        "/api/prompts/9999/duplicate",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
}

/// 追加 / 移除标签（两个端点都回整条 PromptDto，界面据此刷新）。
#[tokio::test]
async fn add_and_remove_prompt_tags() {
    let state = state();
    let added = send_json(
        router_for(state.clone()),
        "POST",
        "/api/prompts/11/tags",
        serde_json::json!({"tags": ["追加的标签"]}),
    )
    .await;
    assert_eq!(added.0, StatusCode::OK);
    let names: Vec<String> = added.1["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"追加的标签".to_string()), "body={}", added.1);

    let tag_id = state
        .with(|conn| {
            conn.query_row("SELECT id FROM tags WHERE name = '追加的标签'", [], |r| r.get::<_, i64>(0))
        })
        .unwrap();
    let removed = send_empty(
        router_for(state.clone()),
        "DELETE",
        &format!("/api/prompts/11/tags/{tag_id}"),
    )
    .await;
    assert_eq!(removed.0, StatusCode::OK);
    // 11 上原本还有"国风"（夹具），所以断言"被移除的那个不在了"
    let names: Vec<String> = removed.1["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(!names.contains(&"追加的标签".to_string()));
}

/// 删提示词：产物**保留但解除关联**（FK `ON DELETE SET NULL`）+ 账本记 delete。
#[tokio::test]
async fn delete_prompt_keeps_media_and_logs_delete() {
    let state = state();
    let (status, body) = send_empty(router_for(state.clone()), "DELETE", "/api/prompts/10").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"deleted": 10}));

    let prompt_id: Option<i64> = state
        .with(|conn| {
            conn.query_row("SELECT prompt_id FROM media_assets WHERE id = 100", [], |r| r.get(0))
        })
        .unwrap();
    assert_eq!(prompt_id, None, "产物必须保留、只解除关联（R1 生效才有 ON DELETE SET NULL）");

    let deletes: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM _sync_log WHERE tbl = 'prompts' AND pk = '10' AND op = 'delete'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(deletes, 1);
}

/// 还没搬的端点要**明确 501**，不许拿空数组假装成功。
#[tokio::test]
async fn unmigrated_endpoints_answer_501() {
    let (status, body) = send_empty(router_for(state()), "GET", "/api/prompts/10/media").await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body["error"], "NOT_IMPLEMENTED");
}
// ===========================================================================
//  产物（P4 第 3 步）：列表 / 详情 / 原件文件流
// ===========================================================================

#[tokio::test]
async fn media_list_uses_the_same_keys_and_filters() {
    let state = state();
    let (status, body) = get_json(router_for(state.clone()), "/api/media?size=10").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(keys(&body), vec!["items", "page", "pages", "size", "total"]);
    assert_eq!(body["total"], 1);
    let item = &body["items"][0];
    assert_eq!(
        keys(item),
        vec![
            "checkpoint", "createdAt", "durationMs", "favorite", "fileUrl", "hasWorkflow", "height",
            "id", "kind", "mimeType", "notes", "nsfw", "originalName", "promptId", "promptNegative",
            "promptPositive", "promptTags", "promptTitle", "seed", "sha256", "sizeBytes", "source",
            "storedName", "thumbUrl", "title", "updatedAt", "width",
        ],
        "少一个键 = 画廊里少一块内容（缩略图 URL 也在这里）"
    );
    // 关联提示词摘要是直出的（列表不必二次请求）
    assert_eq!(item["promptTitle"], "赛博朋克少女");
    assert_eq!(item["promptTags"], serde_json::json!(["赛博朋克"]), "产物自身标签 ∪ 提示词标签");
    assert_eq!(item["fileUrl"], "/api/media/100/file");
    assert_eq!(item["thumbUrl"], "/api/media/100/thumb");

    // 过滤：kind / promptId / untagged / favorite
    let (_, by_kind) = get_json(router_for(state.clone()), "/api/media?kind=VIDEO").await;
    assert_eq!(by_kind["total"], 0);
    let (_, by_prompt) = get_json(router_for(state.clone()), "/api/media?promptId=10").await;
    assert_eq!(by_prompt["total"], 1);
    let (_, untagged) = get_json(router_for(state.clone()), "/api/media?untagged=true").await;
    assert_eq!(untagged["total"], 0);
    // 标签过滤是**精确匹配 normalized**（与 Kotlin `TagRepo.promptTagFilter` 一致，
    // 不是 LIKE）：所以要么给全名，要么筛不到 —— 这里给全名「赛博朋克」
    let (_, by_tag) =
        get_json(router_for(state.clone()), "/api/media?tags=%E8%B5%9B%E5%8D%9A%E6%9C%8B%E5%85%8B").await;
    assert_eq!(by_tag["total"], 1, "按提示词的标签也能筛到产物");
    // 给半截名字筛不到（这条防的是"某天有人顺手把 IN 改成 LIKE"）
    let (_, partial) = get_json(router_for(state.clone()), "/api/media?tags=%E8%B5%9B%E5%8D%9A").await;
    assert_eq!(partial["total"], 0, "标签是精确匹配，不是前缀/模糊匹配");

    let (status, body) = get_json(router_for(state.clone()), "/api/media/100").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["storedName"], "a.png");
    let (status, _) = get_json(router_for(state.clone()), "/api/media/999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn media_file_serves_bytes_with_etag_and_range() {
    let state = state();
    // 给夹具补一个内容哈希：ETag 直接用 sha256（内容寻址）
    state
        .with(|conn| {
            conn.execute("UPDATE media_assets SET sha256 = 'deadbeef' WHERE id = 100", [])
                .map(|_| ())
        })
        .unwrap();
    let app = router_for(state.clone());

    // 200 全量
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/api/media/100/file").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("etag").unwrap(), "\"deadbeef\"");
    assert_eq!(response.headers().get("accept-ranges").unwrap(), "bytes");
    assert_eq!(response.headers().get("content-length").unwrap(), "16");
    assert!(response
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("inline;"));
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"0123456789abcdef");

    // If-None-Match 命中 → 304（省掉整段视频的流量）
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/media/100/file")
                .header("if-none-match", "\"deadbeef\"")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);

    // Range → 206 + Content-Range（视频拖动进度条靠它）
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/media/100/file")
                .header("range", "bytes=4-7")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers().get("content-range").unwrap(), "bytes 4-7/16");
    assert_eq!(response.headers().get("content-length").unwrap(), "4");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"4567", "Range 要给的是那一段字节，不是从头开始");

    // 多段 Range 不支持 → 回 200 全量（**不是**格式不对的 206）
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/media/100/file")
                .header("range", "bytes=0-1,4-5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn media_file_rejects_path_traversal_and_missing_files() {
    let state = state();
    // 把库里的 stored_name 改成一条越界路径（库被人手改过就是这个样子）
    state
        .with(|conn| {
            conn.execute("UPDATE media_assets SET stored_name = '../secret.txt' WHERE id = 100", [])
                .map(|_| ())
        })
        .unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/media/100/file").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "目录穿越必须被挡住");

    // 文件不在磁盘上（比如被用户删了）
    state
        .with(|conn| {
            conn.execute("UPDATE media_assets SET stored_name = 'gone.png' WHERE id = 100", [])
                .map(|_| ())
        })
        .unwrap();
    let (status, body) = get_json(router_for(state.clone()), "/api/media/100/file").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["message"].as_str().unwrap().contains("文件已丢失"));
}

#[tokio::test]
async fn media_thumb_poster_upload_are_explicitly_501() {
    let state = state();
    for uri in ["/api/media/100/thumb", "/api/media/100/poster", "/api/media/100/prompt"] {
        let (status, body) = get_json(router_for(state.clone()), uri).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "uri={uri}");
        assert_eq!(body["error"], "NOT_IMPLEMENTED");
    }
    let (status, _) = send_json(router_for(state.clone()), "POST", "/api/media/upload", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}