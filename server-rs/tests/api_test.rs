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

        -- prompt 11 的来源指向一次真实捕获：它的 **API 图**从 capture_runs.raw 里取。
        -- prompt 10 的 source_ref 是 NULL → 那条走"没有 API 图"的 204 分支。
        UPDATE prompts SET source_ref = 'run:abc' WHERE id = 11;
        INSERT INTO capture_runs (run_key, status, media_count, raw, created_at)
        VALUES ('run:abc', 'success', 0,
                '{"prompt":{"3":{"class_type":"KSampler","inputs":{"seed":1}}}}',
                '2026-09-23T02:00:00.000Z');

        INSERT INTO media_assets (id, prompt_id, kind, original_name, stored_name, size_bytes)
        VALUES (100, 10, 'IMAGE', 'a.png', 'a.png', 1234);

        -- ★ 每张**被同步的表**都放一行。
        -- 为什么非要这样：真库上 /api/sync/changes 曾经 500（`no such column: id`），
        -- 根因是 ai_message_parts 的主键是 (message_id, ordinal) 而不是 id ——
        -- 而当时夹具里**这张表一行都没有**，所以测试全绿也抓不到。
        -- 少放一张表 = 那条链路在夹具里根本没被走到。
        INSERT INTO ai_conversations (id, title, archived, created_at, updated_at)
        VALUES ('conv-0', '夹具会话', 0, '2026-09-23T03:00:00.000Z', '2026-09-23T03:00:00.000Z');
        INSERT INTO ai_messages (id, conversation_id, seq, role, status, text, created_at, updated_at)
        VALUES ('msg-0', 'conv-0', 1, 'user', 'complete', '夹具消息',
                '2026-09-23T03:00:00.000Z', '2026-09-23T03:00:00.000Z');
        INSERT INTO ai_message_parts (message_id, ordinal, type, text, created_at)
        VALUES ('msg-0', 0, 'text', '夹具块', '2026-09-23T03:00:00.000Z');
        INSERT INTO ai_attachments (id, original_name, stored_name, kind, mime_type, created_at)
        VALUES ('att-0', 'a.png', 'att-0.png', 'IMAGE', 'image/png', '2026-09-23T03:00:00.000Z');
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

/// 还没搬的端点要**明确 501**，不许拿空数组假装成功；搬过来的则必须真的能用。
#[tokio::test]
async fn unmigrated_endpoints_answer_501_and_migrated_ones_work() {
    // 搬过来了：提示词下的产物列表（200 + 数组，提示词不存在也是空数组——与 Kotlin 同）
    let (status, body) = get_json(router_for(state()), "/api/prompts/10/media").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    let (status, body) = get_json(router_for(state()), "/api/prompts/9999/media").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!([]));

    // 还没搬：视频封面要 ffmpeg 抽帧
    let (status, body) = get_json(router_for(state()), "/api/media/100/poster").await;
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

/// 缩略图：真图能生成 JPEG 并缓存；非图片 204；坏图回退原图；poster/upload 仍是 501。
#[tokio::test]
async fn media_thumb_generates_a_cached_jpeg_and_falls_back_honestly() {
    let state = state();
    let media_dir = state.media_dir().to_path_buf();

    // 夹具换成一张**真图**（800×400），不然测的是"坏图回退"那条分支
    let mut img = image::RgbImage::new(800, 400);
    for (_, _, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([10, 200, 30]);
    }
    img.save(media_dir.join("a.png")).unwrap();

    let response = router_for(state.clone())
        .oneshot(Request::builder().uri("/api/media/100/thumb").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("content-type").unwrap(), "image/jpeg");
    assert!(response.headers().get("x-thumbnail").is_some(), "命中缓存的标记要带上");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..2], &[0xFF, 0xD8], "JPEG 魔数（说明真的编码过，不是把原图透传）");

    // 落盘缓存 + 最长边压到 512
    let thumb_path = media_dir.parent().unwrap().join("thumbs").join("100.jpg");
    assert!(thumb_path.is_file(), "缩略图要落盘（否则每次都重算）");
    let out = image::open(&thumb_path).unwrap();
    assert_eq!((out.width(), out.height()), (512, 256));

    // 非图片 → 204（前端据此不放缩略图）
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO media_assets (id, kind, original_name, stored_name, mime_type)
                 VALUES (101, 'VIDEO', 'v.mp4', 'v.mp4', 'video/mp4')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    std::fs::write(media_dir.join("v.mp4"), b"not really a video").unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/media/101/thumb").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 坏图（后缀是 png、内容不是）→ **回退原图**（200 + 原 MIME + 原字节），不是 500
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO media_assets (id, kind, original_name, stored_name, mime_type)
                 VALUES (102, 'IMAGE', 'broken.png', 'broken.png', 'image/png')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    std::fs::write(media_dir.join("broken.png"), b"definitely not a png").unwrap();
    let response = router_for(state.clone())
        .oneshot(Request::builder().uri("/api/media/102/thumb").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("content-type").unwrap(), "image/png");
    assert!(response.headers().get("x-thumbnail").is_none());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"definitely not a png");

    // 文件丢了 → 404（不是 500，也不是空白图）
    state
        .with(|conn| {
            conn.execute("UPDATE media_assets SET stored_name = 'gone.png' WHERE id = 100", [])
                .map(|_| ())
        })
        .unwrap();
    std::fs::remove_file(&thumb_path).unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/media/100/thumb").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 还没搬的仍然是 501（别让"缩略图能用了"顺手把 poster/upload 也变成假成功）
    for uri in ["/api/media/100/poster"] {
        let (status, body) = get_json(router_for(state.clone()), uri).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "uri={uri}");
        assert_eq!(body["error"], "NOT_IMPLEMENTED");
    }
    // 产物关联的完整提示词也搬过来了（204 = 没关联）
    let (status, _) = get_json(router_for(state.clone()), "/api/media/101/prompt").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "视频产物没关联提示词 → 204");

    // 上传已经搬过来了：空 multipart 是 400（不再是 501）
    let empty = multipart_body("----comfyhubtest", &[], &[]);
    let (status, _) = post_multipart(router_for(state.clone()), "/api/media/upload", empty).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
// ===========================================================================
//  产物写侧 + /api/stats（P4 第 4 步）
// ===========================================================================

#[tokio::test]
async fn patch_media_updates_fields_and_rejects_empty_body() {
    let state = state();
    // 一个字段都不给 → 404（Kotlin 的 `update` 返回 false，路由翻成"不存在或无字段需要更新"）
    let empty = send_json(router_for(state.clone()), "PATCH", "/api/media/100", serde_json::json!({})).await;
    assert_eq!(empty.0, StatusCode::NOT_FOUND, "空 body 不许静默成功");

    let patched = send_json(
        router_for(state.clone()),
        "PATCH",
        "/api/media/100",
        serde_json::json!({"title": "改过的标题", "favorite": true, "nsfw": true, "notes": "备注"}),
    )
    .await;
    assert_eq!(patched.0, StatusCode::OK, "body={}", patched.1);
    assert_eq!(patched.1["title"], "改过的标题");
    assert_eq!(patched.1["favorite"], true);
    assert_eq!(patched.1["nsfw"], true, "手动标记 NSFW（只影响预览打码）");
    assert_eq!(patched.1["notes"], "备注");
    assert_eq!(patched.1["promptId"], 10, "没提这一列就不该动它");

    // clearPrompt：明确解除关联（与 promptId=null 不是一回事）
    let cleared = send_json(
        router_for(state.clone()),
        "PATCH",
        "/api/media/100",
        serde_json::json!({"clearPrompt": true}),
    )
    .await;
    assert_eq!(cleared.0, StatusCode::OK);
    assert_eq!(cleared.1["promptId"], serde_json::Value::Null, "clearPrompt 必须真的置空");
    assert_eq!(cleared.1["promptTags"], serde_json::json!([]), "解绑后标签也不该再带上提示词的");

    // 指向不存在的提示词 → 400（不是静默忽略）
    let bogus = send_json(
        router_for(state.clone()),
        "PATCH",
        "/api/media/100",
        serde_json::json!({"promptId": 9999}),
    )
    .await;
    assert_eq!(bogus.0, StatusCode::BAD_REQUEST);
    assert!(bogus.1["message"].as_str().unwrap().contains("9999"));

    let missing = send_json(
        router_for(state.clone()),
        "PATCH",
        "/api/media/9999",
        serde_json::json!({"title": "x"}),
    )
    .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
}

/// 删产物：库、原件、缩略图一起清；标签计数跟着重算。
#[tokio::test]
async fn delete_media_removes_row_file_and_thumb() {
    let state = state();
    let media_dir = state.media_dir().to_path_buf();
    let thumb_dir = media_dir.parent().unwrap().join("thumbs");
    std::fs::create_dir_all(&thumb_dir).unwrap();
    std::fs::write(thumb_dir.join("100.jpg"), b"thumb").unwrap();
    std::fs::write(thumb_dir.join("100.poster.png"), b"poster").unwrap();
    assert!(media_dir.join("a.png").exists());

    let (status, body) = send_empty(router_for(state.clone()), "DELETE", "/api/media/100").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body, serde_json::json!({"deleted": 100}));

    let rows: i64 = state
        .with(|conn| conn.query_row("SELECT COUNT(*) FROM media_assets WHERE id = 100", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(rows, 0);
    assert!(!media_dir.join("a.png").exists(), "原件必须一起删（只删库会留下磁盘孤儿）");
    assert!(!thumb_dir.join("100.jpg").exists());
    assert!(!thumb_dir.join("100.poster.png").exists());

    let again = send_empty(router_for(state.clone()), "DELETE", "/api/media/100").await;
    assert_eq!(again.0, StatusCode::NOT_FOUND, "删第二次要 404");
}

#[tokio::test]
async fn stats_counts_match_the_fixture() {
    let (status, body) = get_json(router_for(state()), "/api/stats").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&body),
        vec!["byKind", "byMediaKind", "favoriteMedia", "favoritePrompts", "media", "prompts", "tags"]
    );
    assert_eq!(body["prompts"], 2);
    assert_eq!(body["media"], 1);
    assert_eq!(body["tags"], 2);
    assert_eq!(body["favoritePrompts"], 1, "夹具里 id=10 是收藏的");
    assert_eq!(body["favoriteMedia"], 0);
    assert_eq!(body["byKind"], serde_json::json!({"IMAGE": 2}));
    assert_eq!(body["byMediaKind"], serde_json::json!({"IMAGE": 1}));
}
// ===========================================================================
//  上传（P4 第 6 步）：multipart 多文件 + sha256 判重
// ===========================================================================

/// 造一个 multipart 体（省得引第三方 crate：这一小段手工拼就够，且**结构与 curl 一致**）。
fn multipart_body(boundary: &str, fields: &[(&str, &str)], files: &[(&str, &str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, value) in fields {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        out.extend_from_slice(format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes());
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    for (name, filename, bytes) in files {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        out.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n").as_bytes(),
        );
        out.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

async fn post_multipart(app: axum::Router, uri: &str, body: Vec<u8>) -> (StatusCode, Value) {
    let boundary = "----comfyhubtest";
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();
    let response = app.oneshot(request).await.expect("请求");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn upload_stores_files_with_tags_and_dedupes_by_sha256() {
    let state = state();
    let media_dir = state.media_dir().to_path_buf();

    // 一张真 PNG + 一个重复（同样内容、不同文件名）+ 一段"音频"
    let mut img = image::RgbImage::new(64, 32);
    for (_, _, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([1, 2, 3]);
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let png_bytes = png.into_inner();

    let boundary = "----comfyhubtest";
    let body = multipart_body(
        boundary,
        &[("promptId", "10"), ("tags", "上传标签, 第二个"), ("title", "上传的图")],
        &[
            ("files", "shot.png", &png_bytes),
            ("files", "copy.png", &png_bytes),
            ("files", "voice.mp3", b"ID3fakeaudio"),
        ],
    );
    let (status, body) = post_multipart(router_for(state.clone()), "/api/media/upload", body).await;
    assert_eq!(status, StatusCode::CREATED, "body={body}");
    assert_eq!(keys(&body), vec!["duplicates", "failed", "items", "promptId"]);
    assert_eq!(body["promptId"], 10);
    assert_eq!(body["failed"], serde_json::json!([]));

    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "两个**不同内容**的文件入库；重复那份只记 duplicates");
    let kinds: Vec<&str> = items.iter().map(|i| i["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"IMAGE") && kinds.contains(&"AUDIO"), "kind={kinds:?}");
    let image_item = items.iter().find(|i| i["kind"] == "IMAGE").unwrap();
    assert_eq!(image_item["title"], "上传的图", "给了 title 就用它");
    assert_eq!(image_item["width"], 64, "图片要探出尺寸");
    assert_eq!(image_item["height"], 32);
    assert_eq!(image_item["promptId"], 10);
    // promptTags = 产物自身标签 ∪ 关联提示词(10)的标签「赛博朋克」，合并排序去重
    assert_eq!(image_item["promptTags"], serde_json::json!(["上传标签", "第二个", "赛博朋克"]));
    assert!(image_item["sha256"].as_str().unwrap().len() == 64, "sha256 要落库（判重键）");

    assert_eq!(body["duplicates"].as_array().unwrap().len(), 1);
    assert_eq!(body["duplicates"][0]["fileName"], "copy.png");

    // 缩略图跟着生成（图片入库时就写）
    let thumb = media_dir.parent().unwrap().join("thumbs").join(format!("{}.jpg", image_item["id"]));
    assert!(thumb.is_file(), "图片入库要顺手写缩略图：{}", thumb.display());

    // 落盘名是 32 位 hex + 扩展名（与 Kotlin 同一套命名）
    let stored = image_item["storedName"].as_str().unwrap();
    assert!(stored.ends_with(".png") && stored.len() == 36, "storedName={stored}");

    // 判重是**内容**判重：换个名字再传一次仍然是 duplicate
    let again = multipart_body(boundary, &[], &[("files", "another.png", &png_bytes)]);
    let (status, body2) = post_multipart(router_for(state.clone()), "/api/media/upload", again).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(body2["items"].as_array().unwrap().is_empty());
    assert_eq!(body2["duplicates"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn upload_rejects_empty_and_bad_prompt_id() {
    let state = state();
    // 一个文件都没有 → 400（不静默成功）
    // ⚠ boundary 必须与 `post_multipart` 里的那一个一致（不一致的话解析就失败了，
    // 测到的不是"没有文件"那条分支）
    let empty = multipart_body("----comfyhubtest", &[("title", "只有字段")], &[]);
    let (status, body) = post_multipart(router_for(state.clone()), "/api/media/upload", empty).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("files"));

    // promptId 不存在 → 400
    let body = multipart_body("----comfyhubtest", &[("promptId", "9999")], &[("files", "a.png", b"x")]);
    let (status, body) = post_multipart(router_for(state.clone()), "/api/media/upload", body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("9999"));

    // 校验失败时**临时文件不许留在盘上**（tmp 目录要干净）
    let tmp = state.media_dir().parent().unwrap().join("tmp");
    let leftovers = std::fs::read_dir(&tmp).map(|d| d.count()).unwrap_or(0);
    assert_eq!(leftovers, 0, "校验失败要清掉临时文件");
}
// ===========================================================================
//  捕获配置与工作流原文（P4 第 7 步）
// ===========================================================================

#[tokio::test]
async fn capture_config_round_trips_and_normalizes() {
    let state = state();
    let (status, body) = get_json(router_for(state.clone()), "/api/capture/config").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&body),
        vec!["autoTag", "comfyUrl", "downloadFallback", "enabled", "maxPerPoll", "outputDir", "pollSeconds"]
    );
    assert_eq!(body["comfyUrl"], "http://127.0.0.1:8188", "库里没配过就是环境默认值");
    assert_eq!(body["autoTag"], "ComfyUI");
    assert_eq!(body["pollSeconds"], 4);

    // PUT 是**整体覆盖 + 归一化**：尾斜杠去掉、空白目录当 NULL、越界数值夹住
    let (status, saved) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/capture/config",
        serde_json::json!({
            "enabled": false,
            "comfyUrl": "  http://127.0.0.1:8188/  ",
            "outputDir": "   ",
            "pollSeconds": 99999,
            "autoTag": "  ComfyUI  ",
            "maxPerPoll": 0,
            "downloadFallback": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={saved}");
    assert_eq!(saved["comfyUrl"], "http://127.0.0.1:8188");
    assert_eq!(saved["outputDir"], serde_json::Value::Null);
    assert_eq!(saved["pollSeconds"], 600, "夹到上限：轮询太快会把 ComfyUI 问崩");
    assert_eq!(saved["maxPerPoll"], 1);
    assert_eq!(saved["comfyUrl"].as_str().unwrap().ends_with('/'), false);

    // 再读一次：落库的确实是归一化后的那份（GET 与 PUT 的响应必须一致）
    let (_, again) = get_json(router_for(state.clone()), "/api/capture/config").await;
    assert_eq!(again, saved);

    // 缺字段的 body 也要能收（每个字段都有默认值：前端版本不一致时不该整条 400）
    let (status, partial) = send_json(
        router_for(state.clone()),
        "PUT",
        "/api/capture/config",
        serde_json::json!({"enabled": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(partial["enabled"], true);
    assert_eq!(partial["autoTag"], "ComfyUI", "缺的字段走默认值");
}

#[tokio::test]
async fn workflow_endpoints_distinguish_404_from_204() {
    let state = state();
    // 夹具里 id=10 有 workflow_json、id=11 没有
    let response = router_for(state.clone())
        .oneshot(Request::builder().uri("/api/prompts/10/workflow").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("content-type").unwrap(), "application/json");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], br#"{"nodes":[]}"#, "原文直出，不重新序列化");

    let (status, _) = get_json(router_for(state.clone()), "/api/prompts/11/workflow").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "有这条提示词但没有工作流 → 204（不是 404）");

    let (status, body) = get_json(router_for(state.clone()), "/api/prompts/9999/workflow").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "提示词不存在才是 404");
    assert!(body["message"].as_str().unwrap().contains("9999"));

    // 产物侧同一套语义
    state
        .with(|conn| {
            conn.execute("UPDATE media_assets SET workflow_json = '{\"a\":1}' WHERE id = 100", [])
                .map(|_| ())
        })
        .unwrap();
    let response = router_for(state.clone())
        .oneshot(Request::builder().uri("/api/media/100/workflow").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (status, _) = get_json(router_for(state.clone()), "/api/media/9999/workflow").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// `GET /api/prompts/{id}/api-graph` —— **API 格式**节点图。
///
/// 这条是 R10 的前提：`comfy_submit` 只认 API 图，而它的唯一来源就是这里。
/// 所以三种结局必须分得清清楚楚 —— **把 204 说成 404，界面就会以为"提示词没了"**。
#[tokio::test]
async fn api_graph_distinguishes_404_from_204_from_200() {
    let state = state();

    // ① 有来源、有 raw、raw 里是节点图 → 200 + 图本体
    let (status, body) = get_json(router_for(state.clone()), "/api/prompts/11/api-graph").await;
    assert_eq!(status, StatusCode::OK, "夹具 id=11 的来源指向 run:abc");
    assert_eq!(body["3"]["class_type"], "KSampler");
    assert_eq!(body["3"]["inputs"]["seed"], 1);

    // ② 有这条提示词但没来源 → 204（不是 404、也不是空对象）
    let (status, _) = get_json(router_for(state.clone()), "/api/prompts/10/api-graph").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "source_ref 为空 → 204");

    // ③ 来源指向一次**没有 raw** 的捕获 → 还是 204（"这条跑不了"，而不是"没这条提示词"）
    state
        .with(|conn| {
            conn.execute("UPDATE prompts SET source_ref = 'run:gone' WHERE id = 11", []).map(|_| ())
        })
        .unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/prompts/11/api-graph").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "来源悬空 → 204");

    // ④ raw 是脏数据（解析不了）也只回 204 —— **绝不猜着转换**
    state
        .with(|conn| {
            conn.execute("UPDATE capture_runs SET raw = '{not json' WHERE run_key = 'run:abc'", [])
                .map(|_| ())?;
            conn.execute("UPDATE prompts SET source_ref = 'run:abc' WHERE id = 11", []).map(|_| ())
        })
        .unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/prompts/11/api-graph").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "脏 raw 不猜，回 204");

    // ⑤ raw 里没有节点图（只有 outputs 之类的壳）→ 204
    state
        .with(|conn| {
            conn.execute(
                "UPDATE capture_runs SET raw = '{\"outputs\":{}}' WHERE run_key = 'run:abc'",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/prompts/11/api-graph").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "没有图就是没有图");

    // ⑥ 提示词根本不存在才是 404
    let (status, body) = get_json(router_for(state.clone()), "/api/prompts/9999/api-graph").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["message"].as_str().unwrap().contains("9999"));
}

/// `GET /api/sync/manifest` —— 手机连上来的第一件事（🔒 契约 §1）。
///
/// 最要紧的一条：**带凭据/令牌的表绝不能出现在 `synced: true` 里**。
/// `_sync_log` 的触发器其实覆盖了 `app_settings`，所以"从触发器派生同步范围"这个
/// 看起来很自然的写法会把**配对令牌明文**推给手机。这条用例盯着那份显式白名单。
#[tokio::test]
async fn sync_manifest_lists_scope_and_never_offers_credentials() {
    let state = state();
    let (status, body) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    assert_eq!(status, StatusCode::OK);

    // 形状：契约 §1 的五个键一个都不能少/改名
    assert_eq!(
        keys(&body),
        vec!["cursor", "kitVersion", "schemaVersion", "serverTime", "tables"],
        "manifest 的键集合就是契约",
    );
    assert_eq!(body["schemaVersion"].as_i64(), Some(1), "= PRAGMA user_version");
    // 夹具是**直接 SQL 插的行** —— 触发器照常点亮 `_sync_log`，所以游标已经往前走了。
    // （这正是"游标 = MAX(seq)、不是自己数的行数"的一个活证据）
    let fixture_cursor = body["cursor"].as_i64().unwrap();
    assert!(fixture_cursor > 0, "夹具插过行，_sync_log 就不该是空的");
    let max_seq: i64 = state
        .with(|conn| conn.query_row("SELECT MAX(seq) FROM _sync_log", [], |r| r.get(0)).map_err(Into::into))
        .unwrap();
    assert_eq!(fixture_cursor, max_seq, "游标 = MAX(_sync_log.seq)");
    assert!(
        body["serverTime"].as_str().unwrap().ends_with('Z'),
        "时间一律 ISO-8601 UTC：{}",
        body["serverTime"],
    );

    // 同步表：带 rowCount
    assert_eq!(body["tables"]["prompts"]["synced"], true);
    assert_eq!(body["tables"]["prompts"]["rowCount"].as_i64(), Some(2));
    assert_eq!(body["tables"]["tags"]["rowCount"].as_i64(), Some(2));

    // ★ 安全：令牌/凭据/模型目录一律 synced:false，而且**连行数都不给**
    for dangerous in ["app_settings", "ai_providers", "ai_models", "ai_run_events"] {
        let entry = &body["tables"][dangerous];
        assert_eq!(entry["synced"], false, "{dangerous} 绝不能被当成可同步的表");
        assert!(entry["reason"].as_str().is_some_and(|r| !r.is_empty()), "{dangerous} 要给理由");
        assert!(entry["rowCount"].is_null(), "{dangerous} 连行数都不该外发（行数本身也是信息）");
    }

    // kitVersion：Kotlin 侧没写过就是 null —— **不编一个**
    assert!(body["kitVersion"].is_null(), "没写过版本号就得如实回 null");

    // Kotlin 侧写过之后就取得到（键名 `ai.runkit.version` 与 RunKit.KEY_VERSION 同值）
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO app_settings (k, v, updated_at) VALUES ('ai.runkit.version','2026-09-24.1','x')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    let (_, body) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    assert_eq!(body["kitVersion"].as_str(), Some("2026-09-24.1"));
}

/// `cursor` 必须跟着 `_sync_log` 走（手机靠它判断"我的游标还有效吗"）。
#[tokio::test]
async fn sync_manifest_cursor_follows_the_sync_log() {
    let state = state();
    let (_, before_body) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let before = before_body["cursor"].as_i64().unwrap();

    // 写一行被同步的表 → 触发器点亮 `_sync_log` → 游标必须跟着走
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO tags (name, normalized, category, use_count, created_at)
                 VALUES ('游标用例','游标用例','风格',0,'2026-09-24T00:00:00.000Z')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    let (_, body) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let cursor = body["cursor"].as_i64().unwrap();
    assert!(cursor > before, "写过之后游标必须往前走：{before} -> {cursor}");

    // 与账本里的最大值**同源**（不能是"自己数的行数"）
    let max_seq: i64 = state
        .with(|conn| conn.query_row("SELECT MAX(seq) FROM _sync_log", [], |r| r.get(0)).map_err(Into::into))
        .unwrap();
    assert_eq!(cursor, max_seq, "游标 = MAX(_sync_log.seq)");
}

/// `GET /api/sync/changes` —— 增量。契约 §2 的硬约束逐条验。
#[tokio::test]
async fn sync_changes_returns_full_rows_and_scalar_or_array_keys() {
    let state = state();
    let (_, before) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let since = before["cursor"].as_i64().unwrap();

    // 一张整数主键表 + 一张**TEXT 主键**表 + 一张**复合主键**表
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO tags (name, normalized, category, use_count, created_at)
                 VALUES ('增量A','增量a','风格',0,'2026-09-24T00:00:00.000Z')",
                [],
            )
            .map(|_| ())?;
            conn.execute(
                "INSERT INTO ai_conversations (id,title,archived,created_at,updated_at)
                 VALUES ('conv-uuid-1','增量会话',0,'2026-09-24T00:00:00.000Z','2026-09-24T00:00:00.000Z')",
                [],
            )
            .map(|_| ())?;
            conn.execute("INSERT INTO prompt_tags (prompt_id, tag_id) VALUES (10, 2)", [])
                .map(|_| ())
        })
        .unwrap();

    let (status, body) = get_json(router_for(state.clone()), &format!("/api/sync/changes?since={since}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&body),
        vec!["cursor", "deletes", "hasMore", "upserts"],
        "changes 的键集合就是契约",
    );
    assert_eq!(body["hasMore"], false);
    assert!(body["cursor"].as_i64().unwrap() >= since);

    // upserts 给的是**完整行**（不是 diff）：标签那行连 use_count / created_at 都在
    let tag = &body["upserts"]["tags"][0];
    assert_eq!(tag["name"], "增量A");
    assert!(tag["use_count"].is_number(), "完整行要带齐所有列：{tag}");
    assert!(tag["created_at"].is_string(), "完整行要带齐所有列：{tag}");
    // TEXT 主键（UUID）原样是字符串，没有被数字化
    assert_eq!(body["upserts"]["ai_conversations"][0]["id"], "conv-uuid-1");

    // 复合主键的 insert 也会进 upserts，且取得到整行
    assert_eq!(body["upserts"]["prompt_tags"][0]["prompt_id"], 10);
    assert_eq!(body["upserts"]["prompt_tags"][0]["tag_id"], 2);
}

/// **`deletes` 必须独立成组**，且复合主键要发成数组（契约 §2「主键表示」）。
#[tokio::test]
async fn sync_changes_groups_deletes_separately_with_array_keys() {
    let state = state();
    let (_, before) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let since = before["cursor"].as_i64().unwrap();

    // 删一条单列主键的 + 一条复合主键的
    state
        .with(|conn| {
            conn.execute("DELETE FROM tags WHERE id = 2", []).map(|_| ())?;
            conn.execute("DELETE FROM prompt_tags WHERE prompt_id = 10 AND tag_id = 1", [])
                .map(|_| ())
        })
        .unwrap();

    let (_, body) = get_json(router_for(state.clone()), &format!("/api/sync/changes?since={since}")).await;

    // 单列 → 标量数字；绝不能混进 upserts
    assert_eq!(body["deletes"]["tags"][0], 2, "单列主键发标量");
    assert!(body["upserts"]["tags"].is_null(), "删掉的行不该出现在 upserts 里");
    // 复合 → 数组（不是 '10:1' 字符串）
    //
    // ⚠ 这里**不断言顺序**：删 tag 2 会经外键级联掉 prompt_tags(11,2)，
    // 那条级联删除的账本条目排在显式删 (10,1) 的前面。两条都要在，顺序由 seq 决定。
    let deleted = body["deletes"]["prompt_tags"].as_array().unwrap();
    assert!(
        deleted.contains(&serde_json::json!([10, 1])),
        "显式删的复合主键要发数组：{deleted:?}",
    );
    assert!(
        deleted.contains(&serde_json::json!([11, 2])),
        "★ 外键级联删掉的行也要进 deletes（否则手机永远留着幽灵行）：{deleted:?}",
    );
}

/// ★ 凭据/令牌表**一条都不许出现在同步响应里**。
///
/// 分两层验，两层都是真的：
///
/// ① **schema 现状**：`app_settings` / `ai_providers` 只有 `updated_at` 的 `touch` 触发器，
///    **没有** `_sync_log` 触发器 —— 所以写它们根本不会进账本。
///    （⚠ 别把 `trg_x_touch` 看成 `trg_x_sync_*`：当初就是这么误判过一次。）
/// ② **实现在被"将来扩宽触发器"时的表现**：手工往账本里插一条 `tbl='app_settings'` 的条目
///    （模拟"哪天有人给设置表补了同步触发器"），`changes` **必须**把它挡在门外，
///    而且游标要照常跨过去（否则手机永远推不动）。
#[tokio::test]
async fn sync_changes_never_leaks_settings_or_providers() {
    let state = state();
    let (_, before) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let since = before["cursor"].as_i64().unwrap();

    // ① 写一行令牌表 + 一行凭据表，再看账本 —— **不该有任何条目**
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO app_settings (k, v, updated_at) VALUES ('auth.pairToken','SECRET-TOKEN','x')",
                [],
            )
            .map(|_| ())?;
            conn.execute(
                "INSERT INTO ai_providers (id,display_name,api,base_url,created_at,updated_at)
                 VALUES ('p-secret','秘密供应商','openai','https://x','2026-09-24T00:00:00.000Z','2026-09-24T00:00:00.000Z')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    let touched: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM _sync_log WHERE seq > ?1 AND tbl IN ('app_settings','ai_providers')",
                [since],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(
        touched, 0,
        "这两张表**不该有** _sync_log 触发器（touch 触发器不是 sync 触发器）",
    );

    // ② 手工注入两条账本条目（模拟将来触发器被扩宽）
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO _sync_log (tbl, pk, op, at) VALUES ('app_settings','auth.pairToken','upsert','x')",
                [],
            )
            .map(|_| ())?;
            conn.execute(
                "INSERT INTO _sync_log (tbl, pk, op, at) VALUES ('ai_providers','p-secret','upsert','x')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    let (status, body) = get_json(router_for(state.clone()), &format!("/api/sync/changes?since={since}")).await;
    assert_eq!(status, StatusCode::OK);

    // ★ 一条都不能漏出去（响应体里连那个令牌字符串都不该出现）
    let raw = body.to_string();
    assert!(!raw.contains("SECRET-TOKEN"), "配对令牌明文绝不能出现在同步响应里：{raw}");
    assert!(body["upserts"]["app_settings"].is_null(), "app_settings 不同步");
    assert!(body["upserts"]["ai_providers"].is_null(), "ai_providers 不同步");
    assert!(body["deletes"]["app_settings"].is_null());
    assert!(body["deletes"]["ai_providers"].is_null());

    // 但游标要能跨过这些**不属于同步范围**的条目（否则手机永远推不动）
    let cursor = body["cursor"].as_i64().unwrap();
    let max_seq: i64 = state
        .with(|conn| conn.query_row("SELECT MAX(seq) FROM _sync_log", [], |r| r.get(0)).map_err(Into::into))
        .unwrap();
    assert_eq!(cursor, max_seq, "扫完了就跳到账本末尾，跨过不同步的条目");
}

/// ★ **游标落在保留窗口之外 → `410 Gone`**（契约 §2：这个分支不是可选项）。
///
/// 缺了它的表现是"手机少了几百条，而且**永远补不回来**" ——
/// 增量接口照着旧游标往后扫，永远扫不到被裁掉的那些洞。
#[tokio::test]
async fn sync_changes_returns_410_when_the_cursor_fell_out_of_the_window() {
    let state = state();

    // 模拟日志裁剪：把账本清空，再写一行 → 此时 MIN(seq) 已经是个很大的数
    state
        .with(|conn| {
            conn.execute("DELETE FROM _sync_log", []).map(|_| ())?;
            conn.execute(
                "INSERT INTO tags (name, normalized, category, use_count, created_at)
                 VALUES ('裁剪后','裁剪后','风格',0,'2026-09-24T00:00:00.000Z')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    // 老游标（1）早于窗口 → 410
    let (status, body) = get_json(router_for(state.clone()), "/api/sync/changes?since=1").await;
    assert_eq!(status, StatusCode::GONE, "窗口外的游标必须 410，实际 {status} {body}");
    assert_eq!(body["error"], "GONE");
    assert!(
        body["message"].as_str().unwrap().contains("bootstrap"),
        "错误信息要告诉手机改走 bootstrap：{body}",
    );

    // 贴着窗口左沿（min - 1）是**安全**的，不该 410
    let min: i64 = state
        .with(|conn| conn.query_row("SELECT MIN(seq) FROM _sync_log", [], |r| r.get(0)).map_err(Into::into))
        .unwrap();
    let (status, _) = get_json(router_for(state.clone()), &format!("/api/sync/changes?since={}", min - 1)).await;
    assert_eq!(status, StatusCode::OK, "刚好贴着左沿应当正常返回（不多报一次全量）");

    // 空账本不该 410（新手机本来就没有游标）
    state.with(|conn| conn.execute("DELETE FROM _sync_log", []).map(|_| ())).unwrap();
    let (status, body) = get_json(router_for(state.clone()), "/api/sync/changes?since=99999").await;
    assert_eq!(status, StatusCode::OK, "空账本不 410：{body}");
    assert_eq!(body["hasMore"], false);
}

/// 分页：`hasMore: true` 时游标只能停在**扫到的最后一条**（手机必须接着拉）。
#[tokio::test]
async fn sync_changes_paginates_and_advances_the_cursor() {
    let state = state();
    let (_, before) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let mut cursor = before["cursor"].as_i64().unwrap();

    for i in 0..5 {
        state
            .with(|conn| {
                conn.execute(
                    "INSERT INTO tags (name, normalized, category, use_count, created_at)
                     VALUES (?1, ?1, '风格', 0, '2026-09-24T00:00:00.000Z')",
                    [format!("分页{i}")],
                )
                .map(|_| ())
            })
            .unwrap();
    }

    // limit=2：第一页应当 hasMore=true，且游标**没有**跳到账本末尾
    let (_, page1) = get_json(router_for(state.clone()), &format!("/api/sync/changes?since={cursor}&limit=2")).await;
    assert_eq!(page1["hasMore"], true, "还有下一页");
    let c1 = page1["cursor"].as_i64().unwrap();
    assert!(c1 > cursor, "游标要往前走");
    let max_seq: i64 = state
        .with(|conn| conn.query_row("SELECT MAX(seq) FROM _sync_log", [], |r| r.get(0)).map_err(Into::into))
        .unwrap();
    assert!(c1 < max_seq, "没扫完就不能跳到末尾（否则中间那几条永远丢）：c1={c1} max={max_seq}");
    assert_eq!(page1["upserts"]["tags"].as_array().unwrap().len(), 2, "一页就是 limit 条");

    // 接着拉，最后把所有标签都收全（一页都不许漏）
    cursor = c1;
    let mut seen = page1["upserts"]["tags"].as_array().unwrap().len();
    for _ in 0..10 {
        let (_, page) = get_json(router_for(state.clone()), &format!("/api/sync/changes?since={cursor}&limit=2")).await;
        seen += page["upserts"]["tags"].as_array().map(|a| a.len()).unwrap_or(0);
        cursor = page["cursor"].as_i64().unwrap();
        if page["hasMore"] == false {
            break;
        }
    }
    assert_eq!(seen, 5, "翻完所有页要正好收到 5 条（多一条/少一条都是 bug）");
}

// ---------------------------------------------------------------------------
//  POST /api/sync/push（M4 的 DoD 就在这里）
// ---------------------------------------------------------------------------

/// ⚠ 信封是 camelCase（`kind` / `refId` / `deviceId`），但 **payload 里是数据库列名（snake_case）**
/// —— 契约 §2 的例子就是 `{ "id": 12, "text": "…", "updated_at": "…" }`。
/// payload 走的是"完整行直接覆盖本地行"，所以它必须与列名同形，不能再转一道驼峰。
fn conv_payload(id: &str, title: &str, updated_at: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "system_prompt_version": "v1",
        "archived": 0,
        "created_at": "2026-09-24T00:00:00.000Z",
        "updated_at": updated_at,
    })
}

fn push_item(kind: &str, ref_id: &str, payload: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "kind": kind, "refId": ref_id, "op": "upsert", "payload": payload })
}

async fn push(state: &comfyhub_server_rs::routes::AppState, batch: Vec<serde_json::Value>) -> (StatusCode, Value) {
    send_json(
        router_for(state.clone()),
        "POST",
        "/api/sync/push",
        serde_json::json!({ "deviceId": "phone-test", "batch": batch }),
    )
    .await
}

/// ★ **M4 的 DoD：手机产生的会话在桌面可见；重复推送不产生重复行。**
#[tokio::test]
async fn sync_push_applies_and_is_idempotent() {
    let state = state();
    let item = push_item(
        "ai_conversation",
        "conv-phone-1",
        conv_payload("conv-phone-1", "手机上起的会话", "2026-09-24T10:00:00.000Z"),
    );

    // 第一次：applied
    let (status, body) = push(&state, vec![item.clone()]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["applied"], serde_json::json!(["conv-phone-1"]));
    assert_eq!(body["rejected"], serde_json::json!([]));

    // 桌面看得见（这是 M4 DoD 的前半句）
    let (status, row) = get_json(router_for(state.clone()), "/api/sync/changes?since=0").await;
    assert_eq!(status, StatusCode::OK);
    let titles: Vec<&str> = row["upserts"]["ai_conversations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["title"].as_str())
        .collect();
    assert!(titles.contains(&"手机上起的会话"), "上行之后的会话应当在同步流里：{titles:?}");

    // ★ 第二次推**同一条**：不许产生重复行
    let (status, body) = push(&state, vec![item.clone()]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["applied"], serde_json::json!([]), "第二次不该再 applied");
    assert_eq!(body["rejected"][0]["code"], "STALE_IGNORED", "LWW：同一条再推→服务端赢");
    assert_eq!(body["rejected"][0]["refId"], "conv-phone-1");

    let n: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM ai_conversations WHERE id = 'conv-phone-1'",
                [],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(n, 1, "重复推送**只能有一行**（就地更新，绝不新增）");
    let title: String = state
        .with(|conn| {
            conn.query_row("SELECT title FROM ai_conversations WHERE id='conv-phone-1'", [], |r| r.get(0))
                .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(title, "手机上起的会话", "被拒的那次不该改动内容");
}

/// **服务端赢（LWW）**：`updated_at` 不比桌面新就不许覆盖。
#[tokio::test]
async fn sync_push_keeps_the_desktop_value_when_the_push_is_older() {
    let state = state();
    // 桌面先有一行（更新的时间）
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_conversations (id,title,archived,created_at,updated_at)
                 VALUES ('conv-lww','桌面改过的名字',0,'2026-09-24T00:00:00.000Z','2026-09-24T12:00:00.000Z')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    // 手机推一份**更旧**的 → 被拒，桌面值原封不动
    let (status, body) = push(
        &state,
        vec![push_item(
            "ai_conversation",
            "conv-lww",
            conv_payload("conv-lww", "手机上的旧名字", "2026-09-24T09:00:00.000Z"),
        )],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rejected"][0]["code"], "STALE_IGNORED");
    let title: String = state
        .with(|conn| {
            conn.query_row("SELECT title FROM ai_conversations WHERE id='conv-lww'", [], |r| r.get(0))
                .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(title, "桌面改过的名字", "旧的推送不许把桌面的行回退");

    // 手机推一份**更新**的 → 覆盖（桌面已经在别处改过，这是正常接力）
    let (_, body) = push(
        &state,
        vec![push_item(
            "ai_conversation",
            "conv-lww",
            conv_payload("conv-lww", "手机上的新名字", "2026-09-24T13:00:00.000Z"),
        )],
    )
    .await;
    assert_eq!(body["applied"], serde_json::json!(["conv-lww"]));
    let title: String = state
        .with(|conn| {
            conn.query_row("SELECT title FROM ai_conversations WHERE id='conv-lww'", [], |r| r.get(0))
                .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(title, "手机上的新名字");
}

/// `rejected` 必须**逐条给 code**，而且**永久性失败与暂时失败要分清**。
#[tokio::test]
async fn sync_push_rejects_with_per_item_codes() {
    let state = state();
    let (status, body) = push(
        &state,
        vec![
            // 手机不许创建的表（总纲 §3.3）
            push_item("prompts", "p1", serde_json::json!({"id":"p1","updated_at":"2026-09-24T10:00:00.000Z"})),
            push_item("media_assets", "m1", serde_json::json!({"id":"m1"})),
            push_item("tags", "t1", serde_json::json!({"id":"t1"})),
            // 未知类型
            push_item("whatever", "x1", serde_json::json!({"id":"x1"})),
            // 列对不上（永久性：手机死循环重试也没用）
            push_item(
                "ai_conversation",
                "conv-badcol",
                serde_json::json!({"id":"conv-badcol","不存在的列": 1, "updated_at":"2026-09-24T10:00:00.000Z"}),
            ),
            // refId 与 payload.id 不一致
            push_item("ai_conversation", "conv-ref", conv_payload("别的id", "x", "2026-09-24T10:00:00.000Z")),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let codes: Vec<&str> = body["rejected"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["code"].as_str())
        .collect();
    assert_eq!(
        codes,
        vec![
            "FORBIDDEN_KIND",
            "FORBIDDEN_KIND",
            "FORBIDDEN_KIND",
            "UNKNOWN_KIND",
            "SCHEMA_MISMATCH",
            "REF_MISMATCH",
        ],
        "每个被拒项都要有自己的 code（不能只回一个总数）：{body}",
    );
    assert_eq!(body["applied"], serde_json::json!([]), "这批一条都不该成功");

    // 不支持的操作（delete 是"手机删桌面数据"，策略还没定）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/sync/push",
        serde_json::json!({"batch":[{"kind":"ai_conversation","refId":"c-del","op":"delete","payload":{"id":"c-del"}}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rejected"][0]["code"], "UNSUPPORTED_OP");

    // 被拒的那几条**一条都不许落库**
    // （夹具里本来就有一条 conv-0，所以按 id 查而不是数总数）
    let bad: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM ai_conversations WHERE id IN ('p1','m1','t1','x1','conv-badcol','conv-ref')",
                [],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(bad, 0, "被拒的行一条都不许落库");
}

/// ★ **`changes` 要能扫过每一张被同步的表**（真库上曾经在这里 500）。
///
/// 现场：真实库副本上 `/api/sync/changes?since=0&limit=500` 回
/// `INTERNAL: no such column: id in SELECT * FROM ai_message_parts ...` ——
/// `ai_message_parts` 的主键是 `(message_id, ordinal)`，当初按"单列 id"实现。
/// 夹具里没有这张表的行，所以当时**全绿也没抓住**。
/// 这条用例的做法是：夹具里给每张同步表都塞一行，然后要求 `changes` 一次扫完都不出错，
/// 且 **10 张表一张不少地出现在 upserts 里**（少一张就是那条链路没被走到）。
#[tokio::test]
async fn sync_changes_covers_every_synced_table_without_error() {
    let state = state();
    // `media_tags` 不放公共夹具里：`patch_media_...` 那条用例依赖"产物 100 没有自己的标签"，
    // 共享夹具里塞一行会把那条用例弄红。所以**在这条用例里**补，`since=0` 照样能扫到它。
    state
        .with(|conn| {
            conn.execute("INSERT INTO media_tags (media_id, tag_id) VALUES (100, 1)", []).map(|_| ())
        })
        .unwrap();

    let (status, body) = get_json(router_for(state.clone()), "/api/sync/changes?since=0&limit=500").await;
    assert_eq!(status, StatusCode::OK, "扫过所有同步表不该出错：{body}");

    let all: Vec<&str> = [
        "prompts",
        "tags",
        "prompt_tags",
        "media_assets",
        "media_tags",
        "ai_conversations",
        "ai_messages",
        "ai_message_parts",
        "ai_attachments",
        "capture_runs",
    ]
    .to_vec();
    for table in &all {
        let rows = body["upserts"][table].as_array();
        assert!(
            rows.is_some_and(|r| !r.is_empty()),
            "夹具里每张同步表都有一行，所以 {table} 必须出现在 upserts 里：{}",
            body["upserts"],
        );
    }

    // 复合主键那张表要给出**完整行**（含 message_id / ordinal / type）
    let part = &body["upserts"]["ai_message_parts"][0];
    assert_eq!(part["message_id"], "msg-0");
    assert_eq!(part["ordinal"], 0);
    assert_eq!(part["type"], "text");
}

/// 复合主键的 `deletes` 要按**各自的编码**发数组 ——
/// `ai_message_parts` 用的是 `message_id#ordinal`（`#`，且第一段是 UUID 文本），
/// 与 `prompt_tags` 的 `prompt_id:tag_id`（`:`，两段都是数字）**不是一回事**。
#[tokio::test]
async fn sync_changes_deletes_use_each_tables_own_pk_encoding() {
    let state = state();
    let (_, before) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let since = before["cursor"].as_i64().unwrap();

    state
        .with(|conn| {
            // 删一条消息块（复合主键，`#` 分隔，第一段是 UUID）
            conn.execute("DELETE FROM ai_message_parts WHERE message_id='msg-0' AND ordinal=0", [])
                .map(|_| ())?;
            // 删一条附件（单列 UUID 主键）
            conn.execute("DELETE FROM ai_attachments WHERE id='att-0'", []).map(|_| ())
        })
        .unwrap();

    let (status, body) = get_json(router_for(state.clone()), &format!("/api/sync/changes?since={since}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["deletes"]["ai_message_parts"][0],
        serde_json::json!(["msg-0", 0]),
        "复合主键发数组，第一段是**字符串**（UUID）不是数字",
    );
    assert_eq!(
        body["deletes"]["ai_attachments"][0],
        serde_json::json!("att-0"),
        "单列文本主键发字符串",
    );
}

/// ★ 手机推**消息块**上来（复合主键、没有 `id` 列）必须能成功。
///
/// 真库上曾经因为"假定表有 `id` 列"而把这类上行**全判 SCHEMA_MISMATCH** ——
/// 表现是"手机上能聊、桌面看不到内容"，而且错误码还写着"永远不可能成功"，
/// 手机照做就会把 outbox 那行丢掉（**永久丢数据**）。
#[tokio::test]
async fn sync_push_accepts_composite_key_tables_without_an_id_column() {
    let state = state();
    let (status, body) = push(
        &state,
        vec![push_item(
            "ai_message_part",
            "msg-0#1",
            serde_json::json!({
                "message_id": "msg-0",
                "ordinal": 1,
                "type": "text",
                "text": "手机推上来的块",
                "created_at": "2026-09-24T10:00:00.000Z",
            }),
        )],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["applied"], serde_json::json!(["msg-0#1"]), "被拒了：{body}");

    let text: String = state
        .with(|conn| {
            conn.query_row(
                "SELECT text FROM ai_message_parts WHERE message_id='msg-0' AND ordinal=1",
                [],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(text, "手机推上来的块");

    // 再推同一条（内容一样）→ **幂等**：不管 applied 里有没有它，结果必须还是同一行。
    //
    // ⚠ 注意 `ai_message_parts` **没有 `updated_at` 列**，所以走不到 LWW 分支 ——
    //   它会老老实实再做一次 `ON CONFLICT DO UPDATE`（于是 `applied` 里又有它）。
    //   这不是 bug：契约 §4 要的是"同一条推两次**结果相同**"，而结果确实相同（一行、内容不变）。
    //   有 `updated_at` 的表（会话 / 消息）才会在第二次落到 `STALE_IGNORED`。
    let (_, body) = push(
        &state,
        vec![push_item(
            "ai_message_part",
            "msg-0#1",
            serde_json::json!({
                "message_id": "msg-0",
                "ordinal": 1,
                "type": "text",
                "text": "手机推上来的块",
                "created_at": "2026-09-24T10:00:00.000Z",
            }),
        )],
    )
    .await;
    assert!(
        body["rejected"].as_array().unwrap().is_empty(),
        "重复推不该被拒（它只是幂等地再写一次）：{body}",
    );
    let n: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM ai_message_parts WHERE message_id='msg-0' AND ordinal=1",
                [],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(n, 1, "幂等：同一块推两次**只有一行**");
}

/// 单批上限 500（契约 §4：别让一次请求扛几万行）。
#[tokio::test]
async fn sync_push_rejects_oversized_batches() {
    let state = state();
    let batch: Vec<serde_json::Value> = (0..501)
        .map(|i| push_item("ai_conversation", &format!("c{i}"), serde_json::json!({"id": format!("c{i}")})))
        .collect();
    let (status, body) = push(&state, batch).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("500"), "{body}");
}

/// 上行点亮 `_sync_log`（否则手机上产生的会话**永远同步不回其它设备**）。
#[tokio::test]
async fn sync_push_lights_up_the_sync_log_without_fake_deletes() {
    let state = state();
    let (_, before) = get_json(router_for(state.clone()), "/api/sync/manifest").await;
    let since = before["cursor"].as_i64().unwrap();

    push(
        &state,
        vec![push_item(
            "ai_conversation",
            "conv-log",
            conv_payload("conv-log", "会点亮账本", "2026-09-24T10:00:00.000Z"),
        )],
    )
    .await;
    // 再推一次更新的（走 UPDATE 分支）
    push(
        &state,
        vec![push_item(
            "ai_conversation",
            "conv-log",
            conv_payload("conv-log", "改名了", "2026-09-24T11:00:00.000Z"),
        )],
    )
    .await;

    // ★ R3：upsert 绝不能产生 **delete** 条目
    //   （`INSERT OR REPLACE` 内部是 DELETE+INSERT，会点亮 DELETE 触发器写假 delete）
    let fake_deletes: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM _sync_log WHERE seq > ?1 AND tbl='ai_conversations' AND op='delete'",
                [since],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(fake_deletes, 0, "upsert 不许写假 delete（R3）");

    let upserts: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM _sync_log WHERE seq > ?1 AND tbl='ai_conversations' AND op='upsert'",
                [since],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert!(upserts >= 2, "插入与更新都要进账本，实际 {upserts}");
}

// ---------------------------------------------------------------------------
//  GET /api/sync/bootstrap
// ---------------------------------------------------------------------------

/// 全量分页：形状自洽 + **翻完所有页不重不漏** + 只发同步范围内的表。
#[tokio::test]
async fn sync_bootstrap_pages_without_gaps_or_duplicates() {
    let state = state();
    // 夹具里 tags 有 2 行；再补 3 行凑成 5 行，pageSize=2 → 3 页
    for i in 0..3 {
        state
            .with(|conn| {
                conn.execute(
                    "INSERT INTO tags (name, normalized, category, use_count, created_at)
                     VALUES (?1, ?1, '风格', 0, '2026-09-24T00:00:00.000Z')",
                    [format!("全量{i}")],
                )
                .map(|_| ())
            })
            .unwrap();
    }

    let (status, page1) = get_json(
        router_for(state.clone()),
        "/api/sync/bootstrap?table=tags&page=1&pageSize=2",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&page1),
        vec!["page", "pageSize", "rows", "table", "totalPages"],
        "bootstrap 的键集合就是契约 §3",
    );
    assert_eq!(page1["table"], "tags");
    assert_eq!(page1["page"], 1);
    assert_eq!(page1["pageSize"], 2);
    assert_eq!(page1["totalPages"], 3, "5 行 / 每页 2 = 3 页");
    assert_eq!(page1["rows"].as_array().unwrap().len(), 2);

    // ★ 翻完所有页：id 一个不重、一个不漏
    let mut ids: Vec<i64> = Vec::new();
    for page in 1..=3 {
        let (_, body) = get_json(
            router_for(state.clone()),
            &format!("/api/sync/bootstrap?table=tags&page={page}&pageSize=2"),
        )
        .await;
        for row in body["rows"].as_array().unwrap() {
            ids.push(row["id"].as_i64().unwrap());
        }
    }
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 5, "翻完所有页要正好 5 条（重复/漏掉都是 bug）：{ids:?}");
}

/// **绝不发 `.db` 文件**：`app_settings` 里有配对令牌明文，bootstrap 一张表都不许多发。
#[tokio::test]
async fn sync_bootstrap_refuses_tables_outside_the_scope() {
    let state = state();
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO app_settings (k, v, updated_at) VALUES ('auth.pairToken','SECRET-TOKEN','x')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    // 凭据/令牌表一律 400，而且错误信息里**列出合法表名**（自解释）
    for forbidden in ["app_settings", "ai_providers", "ai_models", "ai_run_events"] {
        let (status, body) = get_json(
            router_for(state.clone()),
            &format!("/api/sync/bootstrap?table={forbidden}&page=1"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{forbidden} 不该能全量导出");
        let msg = body["message"].as_str().unwrap();
        assert!(msg.contains("prompts"), "错误信息要列出合法表名：{msg}");
        assert!(!body.to_string().contains("SECRET-TOKEN"), "令牌绝不能出现：{body}");
    }

    // 缺 table 也是 400（必填，理由见 sync::bootstrap 的注释）
    let (status, body) = get_json(router_for(state.clone()), "/api/sync/bootstrap?page=1").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("table"));
}
#[tokio::test]
async fn media_prompt_has_three_outcomes() {
    let state = state();
    // 有关联 → 200 + 完整提示词（含标签与产物计数）
    let (status, body) = get_json(router_for(state.clone()), "/api/media/100/prompt").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["id"], 10);
    assert_eq!(body["title"], "赛博朋克少女");
    assert_eq!(body["tags"][0]["name"], "赛博朋克");

    // 没关联 → 204（Dart 的 `mediaPrompt` 把 204 当 null，界面退化成"没有提示词"）
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO media_assets (id, kind, original_name, stored_name) VALUES (103, 'IMAGE', 'z.png', 'z.png')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/media/103/prompt").await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 正常路径下**根本不会出现悬挂引用**：R1 生效时 `prompt_id` 是 ON DELETE SET NULL，
    // 删提示词只会把它解绑 → 下一条查询变成 204（产物照样留着，这正是"删提示词保产物"）
    state
        .with(|conn| conn.execute("DELETE FROM prompts WHERE id = 10", []).map(|_| ()))
        .unwrap();
    let (status, _) = get_json(router_for(state.clone()), "/api/media/100/prompt").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "删提示词后产物被解绑（不是悬挂）");

    // 悬挂引用只在"外键没生效时写进去的库"里才会出现 —— 把 FK 关掉造一个，验证 404 分支
    // （Kotlin 那条路由这里回的是 200 + {"error":…}，前端会把它解析成一条空提示词；
    //  我们回 404，Dart 的 mediaPrompt 对非 2xx 是抛异常 —— 这是有意的行为差异）
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO media_assets (id, prompt_id, kind, original_name, stored_name)
                 VALUES (104, 11, 'IMAGE', 'd.png', 'd.png')",
                [],
            )?;
            conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
            conn.execute("DELETE FROM prompts WHERE id = 11", [])?;
            conn.execute_batch("PRAGMA foreign_keys = ON;")?;
            Ok(())
        })
        .unwrap();
    let (status, body) = get_json(router_for(state.clone()), "/api/media/104/prompt").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["message"].as_str().unwrap().contains("已被删除"));

    // 产物本身不存在 → 404
    let (status, _) = get_json(router_for(state.clone()), "/api/media/9999/prompt").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}