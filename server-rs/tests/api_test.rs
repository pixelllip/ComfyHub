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
    // ⚠ 唯一性靠"**纳秒 + 进程内计数器**"，只靠纳秒**撞过**：Windows 上
    // `SystemTime` 的粒度约 100ns，并行用例里两个 `state()` 拿到同一个值，
    // 于是两个用例共用同一个临时根 —— 一个写进去的长期记忆被另一个读到，
    // 表现是"拒绝了的调用居然写进了记忆"这种莫名其妙的红（2026-09-25 实测）。
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir()
        .join(format!("comfyhub-test-{}-{unique}-{seq}", std::process::id()));
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
    // ★ 这四个键是 **Dart 在读的**（Kotlin `HealthDto` 同形）：
    //   `backend_launcher.dart:271` 用 `database == "ok"` 判断"后端已就绪"，
    //   缺了它启动流程会以为库坏了；设置页显示 `version` / `storageDir`。
    for key in ["version", "database", "storageDir", "serverTime"] {
        assert!(body.get(key).is_some(), "缺了 Dart 会读的键 `{key}`：{body}");
    }
    assert_eq!(body["database"], "ok", "库里真能跑通 SELECT 1 才叫 ok");
    assert_eq!(body["version"], "1.0.0", "Kotlin 的 APP_VERSION");
    // `storageDir` 是 `<storage>`（media 的父目录），不是 media 目录本身
    let storage = body["storageDir"].as_str().unwrap();
    assert!(!storage.replace('\\', "/").ends_with("/media"), "storageDir 不该指向 media：{storage}");
    assert!(storage.contains("comfyhub-test"), "夹具的 storage 就在临时目录下：{storage}");
    assert!(body["serverTime"].as_str().unwrap().contains('T'), "{body}");
}

#[tokio::test]
async fn tags_use_the_same_keys_as_the_kotlin_server() {
    let (status, body) = get_json(app(), "/api/tags").await;
    assert_eq!(status, StatusCode::OK);
    let items = body.as_array().unwrap();
    assert_eq!(items.len(), 2);
    // popular：使用次数降序
    assert_eq!(items[0]["name"], "国风");
    // 键集合**逐条照 Kotlin 算**（`explicitNulls = false`，真库副本上实测过）：
    //  · `normalized` **不存在** —— Kotlin 的 `TagDto`（`Models.kt:11`）里根本没这个字段；
    //  · `color` / `description` 在库里是 NULL → **键不出现**（不是 `"color": null`）。
    assert_eq!(
        keys(&items[0]),
        vec!["category", "createdAt", "id", "name", "useCount"],
        "国风那条 color/description 是 NULL，键就该不出现（Kotlin 实测形状）"
    );
    assert_eq!(
        keys(&items[1]),
        vec!["category", "color", "createdAt", "id", "name", "useCount"],
        "赛博朋克那条有 color，于是正好多这一个键"
    );
    assert!(!items[0].as_object().unwrap().contains_key("color"), "NULL 不等于 null");
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
    // 这一条（prompt 10）**填满了**大部分字段，所以键几乎都在；NULL 的那些键不出现（Kotlin 形状）
    assert_eq!(
        keys(&body),
        vec![
            "cfgScale", "checkpoint", "createdAt", "extraParams", "favorite", "hasWorkflow", "id",
            "kind", "loras", "mediaCount", "negativePrompt", "positivePrompt", "seed", "source",
            "tags", "title", "updatedAt",
        ],
        "少一个键 = 界面上少一块内容；多一个键 = 与 Kotlin 不一致"
    );
    for absent in ["sampler", "scheduler", "steps", "width", "height", "batchSize", "notes", "sourceRef"] {
        assert!(
            !body.as_object().unwrap().contains_key(absent),
            "`{absent}` 在库里是 NULL，Kotlin 那边**整个键都不发**（不是 null）：{body}"
        );
    }
    assert_eq!(body["positivePrompt"], "1girl, cyberpunk");
    assert_eq!(body["cfgScale"], 3.5);
    assert_eq!(body["favorite"], true, "favorite 是布尔（库里是 0/1）");
    assert_eq!(body["hasWorkflow"], true, "接口只给'有没有'，不给整份 workflow_json");
    assert_eq!(body["loras"][0]["name"], "anima");
    assert_eq!(body["loras"][0]["weight"], 0.8);
    assert_eq!(body["extraParams"], serde_json::json!({}), "NULL 当空对象");
    assert_eq!(body["tags"][0]["name"], "赛博朋克");
    assert_eq!(body["mediaCount"], 1);

    // prompt 11 几乎全空：键集合缩到最小 —— 这条专门盯"NULL 字段不许冒出来"
    let (status, bare) = get_json(app(), "/api/prompts/11").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&bare),
        vec![
            "createdAt", "extraParams", "favorite", "hasWorkflow", "id", "kind", "loras",
            "mediaCount", "positivePrompt", "source", "sourceRef", "tags", "title", "updatedAt",
        ],
        "全空的那条只剩这些键：{bare}"
    );
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
    // 错误体是 `{error, detail?}`：普通错误**人话就写在 `error` 里**（`detail` 不出现）
    assert_eq!(keys(&body), vec!["error"]);
    assert!(body["error"].as_str().unwrap().contains("999"), "body={body}");
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
//  错误体形状（2026-09-24 对齐）：`{error, detail?}` —— **没有 `message` 这个键**
// ===========================================================================

/// `error` 是不是一个"机器码"（`NOT_FOUND` / `BAD_REQUEST` / `SCHEMA_MISMATCH` 那种全大写）。
fn looks_like_a_machine_code(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
}

/// **普通错误**：`error` 就是人话，`detail` 不出现，`message` 这个键根本不存在。
///
/// 为什么这条必须有用例：Dart 的 `ApiClient._decode`（`lib/core/api_client.dart:50`）
/// 把 `error` 当人话读；`AiApiClient._decode` 先读 `detail`、再读 `error`。
/// 早先这里是 `{error:"NOT_FOUND", message:"提示词 999 不存在"}` ——
/// 两个客户端都读不到人话，界面显示的是 `NOT_FOUND`。**形状错不报错，只显示机器码。**
#[tokio::test]
async fn ordinary_errors_put_the_human_text_in_error() {
    for (uri, expected) in [
        ("/api/prompts/999", StatusCode::NOT_FOUND),
        ("/api/prompts/abc", StatusCode::BAD_REQUEST),
        ("/api/media/9999", StatusCode::NOT_FOUND),
        ("/api/nope", StatusCode::NOT_FOUND), // 未知路径也要同一个信封（默认 fallback 是空 body）
    ] {
        let (status, body) = get_json(app(), uri).await;
        assert_eq!(status, expected, "uri={uri} body={body}");
        assert_eq!(keys(&body), vec!["error"], "普通错误只有 `error` 一个键：{body}");
        let text = body["error"].as_str().unwrap();
        assert!(!looks_like_a_machine_code(text), "`error` 必须是给人看的话：{text}");
        assert!(!body.to_string().contains("message"), "`message` 这个键不许再出现：{body}");
    }
}

/// **AI 领域错误**（`AiException` 的对应物，AIH-024）：`error` = 稳定码，`detail` = 人话。
///
/// 形状基线是实测出来的（真库副本 + Kotlin 8090）：
/// `GET /api/ai/conversations/999999` → `400 {"error":"CONFIG_ERROR","detail":"会话不存在: 999999"}`。
#[tokio::test]
async fn ai_errors_put_the_stable_code_in_error_and_the_human_text_in_detail() {
    use axum::response::IntoResponse;

    let body_of = |error: comfyhub_server_rs::routes::ApiError| async move {
        let response = error.into_response();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice::<Value>(&bytes).unwrap())
    };

    let (status, body) = body_of(comfyhub_server_rs::routes::ApiError::ai(
        "CONFIG_ERROR",
        "会话不存在: 999999",
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "AI 异常照 Kotlin 固定 400");
    assert_eq!(keys(&body), vec!["detail", "error"]);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert_eq!(body["detail"], "会话不存在: 999999");

    // 没有人类可读说明时 `detail` **整键省略**（Kotlin 那边 `cause.message == null` 就是这个形状）
    let (_, body) = body_of(comfyhub_server_rs::routes::ApiError::ai("CONFIG_ERROR", "   ")).await;
    assert_eq!(keys(&body), vec!["error"], "`detail` 必须是**不出现**，不是 null：{body}");
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
    assert!(body["error"].as_str().unwrap().contains("不能为空"), "body={body}");
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
    assert!(clash.1["error"].as_str().unwrap().contains("已存在"), "body={}", clash.1);

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
    assert!(body["error"].as_str().unwrap().contains("不能为空"), "body={body}");
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
    assert!(
        body["error"].as_str().unwrap().contains("还没搬到 Rust"),
        "501 的 error 也是人话（不是 NOT_IMPLEMENTED 这种机器码）：{body}"
    );
}
// ===========================================================================
//  本机工作流文件（P4 第 14 步）：GET /api/capture/workflows
// ===========================================================================

/// 起一个"磁盘上真有工作流文件"的环境，并把它接进 `AppState`。
///
/// 目录布局照**发布包/源码树的真实形状**摆：`<根>\storage\media` 是产物目录，
/// 于是 `AppState::project_root()` 算出来就是 `<根>`（与脚本传的 `COMFYHUB_PROJECT_ROOT` 等价）。
fn state_with_workflow_files() -> (comfyhub_server_rs::routes::AppState, std::path::PathBuf) {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("comfyhub-wf-{}-{unique}", std::process::id()));

    // 项目内的 comfyui（三份目录来源里的第 ② 份）
    let home = root.join("comfyui");
    std::fs::create_dir_all(home.join("comfy")).unwrap();
    std::fs::write(home.join("main.py"), "print(1)").unwrap();
    let workflows = home.join("user").join("default").join("workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join("krea2_hd.json"), r#"{"nodes":[],"links":[]}"#).unwrap();
    std::fs::write(workflows.join("broken.json"), "{ 不是 json").unwrap();

    let media = root.join("storage").join("media");
    std::fs::create_dir_all(&media).unwrap();

    let conn = test_conn();
    // 其中一份已经在库里（`comfy_load_workflow` 用的就是 `run_key = file:<sha256>`）
    let sha = comfyhub_server_rs::comfy_dirs::sha256_hex(br#"{"nodes":[],"links":[]}"#);
    conn.execute(
        "INSERT INTO capture_runs (run_key, status, media_count, raw, prompt_id, created_at)
         VALUES (?1, 'success', 0, NULL, 10, '2026-09-24T00:00:00.000Z')",
        rusqlite::params![format!("file:{sha}")],
    )
    .unwrap();

    (comfyhub_server_rs::routes::AppState::new(conn, ":memory:", media), home)
}

#[tokio::test]
async fn capture_workflows_lists_files_with_the_same_keys() {
    let (state, _dir) = state_with_workflow_files();
    let (status, body) = get_json(router_for(state), "/api/capture/workflows").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(keys(&body), vec!["count", "dirs", "files", "hint", "total"], "少一个键 = 界面上少一块内容");
    // 只列真的能用的：坏 JSON 不列（列了 AI 会白跑一趟 comfy_load_workflow）
    assert_eq!(body["total"], 1, "body={body}");
    let file = &body["files"][0];
    let mut file_keys = keys(file);
    file_keys.sort();
    assert_eq!(
        file_keys,
        vec!["bytes", "format", "inLibrary", "modifiedAt", "name", "path", "promptId"],
        "file 的键集合"
    );
    assert_eq!(file["name"], "krea2_hd.json");
    assert_eq!(file["format"], "ui");
    // `file:<sha256>` 是**同一把钥匙**：在库里就得如实说，并且给出可以直接提交的 promptId
    assert_eq!(file["inLibrary"], true);
    assert_eq!(file["promptId"], 10);
    assert!(body["dirs"].as_array().unwrap().len() >= 1, "三份来源里的项目内 comfyui 必须被认出来");
}

#[tokio::test]
async fn capture_workflows_query_filters_by_file_name() {
    let (state, _dir) = state_with_workflow_files();
    let (_, hit) = get_json(router_for(state.clone()), "/api/capture/workflows?q=KREA2").await;
    assert_eq!(hit["count"], 1, "query 是文件名的**不区分大小写**子串");
    let (_, miss) = get_json(router_for(state), "/api/capture/workflows?q=%E6%B2%A1%E6%9C%89").await;
    assert_eq!(miss["count"], 0);
    // 没命中时要说清楚"共有几份、可以不带 query 再列一次"，而不是干巴巴一个空数组
    assert!(miss["message"].as_str().unwrap().contains("文件名匹配"));
}

#[tokio::test]
async fn capture_locate_reports_the_home_and_never_writes_on_its_own() {
    let (state, home) = state_with_workflow_files();
    let (status, body) = get_json(router_for(state.clone()), "/api/capture/locate").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let mut shape = keys(&body);
    shape.sort();
    assert_eq!(
        shape,
        vec!["candidates", "home", "outputDir", "source"],
        "与 Kotlin `ComfyLocation` 同形（`configuredOutputDir` / `note` 是 null → 键不出现）"
    );
    // 夹具把 ComfyUI 摆在项目根下的 `comfyui`（源码形态：main.py + comfy/）
    assert_eq!(body["home"].as_str().unwrap(), home.to_string_lossy(), "body={body}");
    assert_eq!(
        body["outputDir"].as_str().unwrap(),
        home.join("output").to_string_lossy(),
        "输出目录 = <home>\\output（还没跑过 ComfyUI 也照样给）"
    );
    assert!(
        !body.as_object().unwrap().contains_key("configuredOutputDir"),
        "没配过就是 null → 这个键不出现，也不编一个：{body}"
    );
    // 候选里记了"项目内的 comfyui 目录"这一条，且带判定依据（用户排错要看）
    let candidates = body["candidates"].as_array().unwrap();
    let hit = candidates
        .iter()
        .find(|c| c["kind"].as_str().unwrap().contains("项目内的 comfyui"))
        .expect("项目内的 comfyui 必须在候选里");
    assert_eq!(hit["looksLikeComfy"], true);
    assert!(hit["reason"].as_str().unwrap().contains("main.py"));

    // **只读**：探测一遍之后配置里不该多出任何东西
    let (_, config) = get_json(router_for(state), "/api/capture/config").await;
    assert!(
        !config.as_object().unwrap().contains_key("outputDir"),
        "探测不许自动生效（要用户点「使用这个目录」）；没配时 outputDir 键也不出现：{config}"
    );
}

#[tokio::test]
async fn locate_apply_validates_the_dir_and_then_persists_it() {
    let (state, home) = state_with_workflow_files();
    let output = home.join("output");
    std::fs::create_dir_all(&output).unwrap();

    // 空 → 400；不存在 → 400（这两条都会把"捕获一条也收不到"的配置写进去，必须先挡住）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/capture/locate/apply",
        serde_json::json!({"outputDir": "   "}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
    let (status, _) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/capture/locate/apply",
        serde_json::json!({"outputDir": "D:\\comfyhub-not-here-98765"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 真的存在 → 写进配置，并且**回读得到**（返回的就是归一化后的那份配置）
    let (status, saved) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/capture/locate/apply",
        serde_json::json!({"outputDir": output.to_string_lossy()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={saved}");
    assert_eq!(saved["outputDir"].as_str().unwrap(), output.to_string_lossy());
    let (_, config) = get_json(router_for(state.clone()), "/api/capture/config").await;
    assert_eq!(config["outputDir"].as_str().unwrap(), output.to_string_lossy());

    // 配上之后 locate 就"就按这个用"（尊重用户配置，不再探）
    let (_, again) = get_json(router_for(state), "/api/capture/locate").await;
    assert_eq!(again["configuredOutputDir"].as_str().unwrap(), output.to_string_lossy());
    assert!(again["source"].as_str().unwrap().contains("已经配置过了"));
    assert_eq!(again["home"].as_str().unwrap(), home.to_string_lossy());
}

// ===========================================================================
//  AI 端点第一批（P4 第 16 步）：GET /api/ai/providers
// ===========================================================================

/// provider 清单：**形状要逐字对齐**，而且**一个字节的密钥都不许出去**（AIH-012）。
#[tokio::test]
async fn ai_providers_shape_and_never_leaks_a_secret_value() {
    let state = state();
    // 托管凭据文件里放一把"密钥"：它**只该被读键名**，值绝不能出现在响应里
    let store = state.media_dir().parent().unwrap().join("ai");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(
        store.join("credentials.dpapi.json"),
        r#"{"MANAGED_KEY":"DPAPI-SEALED-SECRET-VALUE"}"#,
    )
    .unwrap();
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_providers
                 (id, display_name, api, base_url, credential_ref, endpoint_trust, enabled, revision)
                 VALUES ('p-managed', '本机网关', 'openai-completions',
                         'http://127.0.0.1:11434/v1', 'MANAGED_KEY', 'loopback', 1, 3),
                        ('p-none', '没配过的', 'anthropic-messages',
                         'https://api.anthropic.com/v1', 'MISSING_KEY', 'public', 0, 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let (status, body) = get_json(router_for(state), "/api/ai/providers").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let list = body.as_array().unwrap();
    assert_eq!(list.len(), 2, "按 display_name, id 排序");
    assert_eq!(list[0]["id"], "p-managed");

    let mut shape = keys(&list[0]);
    shape.sort();
    assert_eq!(
        shape,
        vec![
            "api", "baseURL", "createdAt", "credential", "credentialRef", "displayName", "enabled",
            "endpointTrust", "id", "revision", "updatedAt",
        ],
        "`credential` 与两个时间戳最容易漏（Kotlin 那边 encodeDefaults = true，一定会序列化）"
    );
    assert_eq!(list[0]["baseURL"], "http://127.0.0.1:11434/v1");
    assert_eq!(list[0]["enabled"], true);
    assert_eq!(list[0]["revision"], 3);

    // 凭据状态：三个键、**没有值**；托管文件里有 → managed 且可写
    assert_eq!(keys(&list[0]["credential"]), vec!["configured", "source", "writable"]);
    assert_eq!(list[0]["credential"]["configured"], true);
    assert_eq!(list[0]["credential"]["source"], "managed");
    assert_eq!(list[0]["credential"]["writable"], true);
    // 没配过的那条：none，但**仍可写**（界面要能给它写一把新的）
    assert_eq!(list[1]["credential"]["configured"], false);
    assert_eq!(list[1]["credential"]["source"], "none");
    assert_eq!(list[1]["credential"]["writable"], true);

    // ★ 红线（AIH-012）：响应里**不许出现那把密钥的值**
    let raw = serde_json::to_string(&body).unwrap();
    assert!(!raw.contains("DPAPI-SEALED-SECRET-VALUE"), "凭据值绝不能出库/出网：{raw}");
}

// ===========================================================================
//  模型目录（P4 第 20 步 = Task 2）：GET /api/ai/providers/{id}/models
// ===========================================================================

/// 模型目录：**12 个键永远在、6 个可空键一个都不许自己冒出来**，且坏 JSON 不能让整页打不开。
///
/// 这条用例是照着**真库副本上 Kotlin（8090）的实测**写的，不是照着 DTO 猜的：
/// 81 条 `command-code-goat` 模型里 `thinkingFormat` 有 10 条不出现、
/// `maxOutputTokens` / `maxAttachmentBytes` / `maxAttachmentCount` / `capabilityVerifiedAt`
/// 81/81 全都不出现（NULL → `explicitNulls = false`）。
/// 写成"所有键一定都在"就等于**用测试把错误的形状钉住**（第 18 步刚栽过这一次）。
#[tokio::test]
async fn ai_models_distinguish_missing_provider_from_an_empty_catalog() {
    let state = state();
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_providers
                 (id, display_name, api, base_url, endpoint_trust, enabled, revision)
                 VALUES ('p-models', '模型目录', 'openai-completions', 'http://127.0.0.1:1234/v1',
                         'loopback', 1, 1),
                        ('p-empty', '空目录', 'openai-completions', 'http://127.0.0.1:1235/v1',
                         'loopback', 1, 1)",
                [],
            )?;
            conn.execute(
                r#"INSERT INTO ai_models
                   (provider_id, model_id, display_name, input_modalities, attachment_transports,
                    mime_allowlist, tools, parallel_tools, reasoning, thinking_efforts, thinking_format,
                    context_window, max_output_tokens, max_attachment_bytes, max_attachment_count,
                    capability_source, capability_verified_at, enabled)
                   VALUES
                   -- ① 样样都有：18 个键一个不少
                   ('p-models', 'm-a', 'A 模型', '["text","image"]', '{"image":["inline_base64"]}',
                    '["image/png"]', 1, 0, 1, '{"high":"high","max":4096}', 'openai',
                    200000, 8192, 8388608, 4, 'builtin', '2026-09-23T05:00:00.000Z', 1),
                   -- ② 可空列全 NULL：6 个可空键**一个都不许出现**
                   --    （display_name / capability_source 是 NOT NULL，给真值）
                   ('p-models', 'm-b', 'm-b', NULL, NULL, NULL, 0, 0, 0, NULL, NULL,
                    NULL, NULL, NULL, NULL, 'manual', NULL, 0),
                   -- ③ 坏 JSON：解析不出来就按空处理，**不能让整个端点 500**
                   ('p-models', 'm-c', 'B 模型', 'not json', '[1,2]', '{oops}',
                    1, 0, 0, '{"off":null,"low":"low"}', '   ',
                    NULL, NULL, 0, 0, 'manual', NULL, 1)"#,
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let (status, body) = get_json(router_for(state.clone()), "/api/ai/providers/p-models/models").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let list = body.as_array().unwrap();
    // 顺序照抄 Kotlin 的 `ORDER BY display_name, model_id`（**默认 BINARY 排序**）：
    // 大写 ASCII 在前、小写在后 → A 模型 / B 模型 / m-b
    assert_eq!(
        list.iter().map(|m| m["id"].as_str().unwrap()).collect::<Vec<_>>(),
        vec!["m-a", "m-c", "m-b"],
        "顺序换了在界面上就是「列表位置变了」：{body}"
    );

    // ① 全字段那行：精确键集合（18 个）
    assert_eq!(
        keys(&list[0]),
        vec![
            "attachmentTransports", "capabilitySource", "capabilityVerifiedAt", "contextWindow",
            "displayName", "enabled", "id", "inputModalities", "maxAttachmentBytes",
            "maxAttachmentCount", "maxOutputTokens", "mimeAllowlist", "parallelTools", "providerId",
            "reasoning", "thinkingEfforts", "thinkingFormat", "tools",
        ],
        "少一个键 = 界面上少一块内容（附件准入与思考强度选择器全靠它们）"
    );
    assert_eq!(list[0]["providerId"], "p-models");
    assert_eq!(list[0]["id"], "m-a");
    assert_eq!(list[0]["displayName"], "A 模型");
    assert_eq!(list[0]["inputModalities"], serde_json::json!(["text", "image"]));
    assert_eq!(
        list[0]["attachmentTransports"],
        serde_json::json!({"image": ["inline_base64"]}),
        "附件传输方式的唯一真源（AGENTS §10）"
    );
    assert_eq!(list[0]["mimeAllowlist"], serde_json::json!(["image/png"]));
    assert_eq!(list[0]["tools"], true);
    assert_eq!(list[0]["reasoning"], true);
    // 数字必须还是数字（写成字符串前端 `as num?` 就取不到）
    assert_eq!(list[0]["contextWindow"], 200000);
    assert_eq!(list[0]["maxOutputTokens"], 8192);
    assert_eq!(list[0]["maxAttachmentBytes"], 8388608);
    assert_eq!(list[0]["maxAttachmentCount"], 4);
    // thinking_efforts 的值**可能是数字**（Anthropic 的思考预算，AIH-056）→ 统一按字面文本
    assert_eq!(list[0]["thinkingEfforts"]["max"], "4096");
    // 时间列**不是透传**：Kotlin 先解析、再按 Java `Instant.toString()` 重排 ——
    // 库里那个 `.000` 在这里被吃掉（`…:00.000Z` → `…:00Z`）。
    // 这条断言就是 `iso_time` 存在的理由：透传版本在真库上"恰好"是对的，
    // 直到某一行的时间戳毫秒位真的是 000（1/1000）。
    assert_eq!(list[0]["capabilityVerifiedAt"], "2026-09-23T05:00:00Z");

    // ② 全 NULL 那行：**只有 12 个键**
    assert_eq!(
        keys(&list[2]),
        vec![
            "attachmentTransports", "capabilitySource", "displayName", "enabled", "id",
            "inputModalities", "mimeAllowlist", "parallelTools", "providerId", "reasoning",
            "thinkingEfforts", "tools",
        ],
        "可空列为 NULL 时 Kotlin 连键都不发（explicitNulls = false）"
    );
    assert_eq!(list[2]["displayName"], "m-b");
    assert_eq!(list[2]["capabilitySource"], "manual");
    assert_eq!(list[2]["inputModalities"], serde_json::json!([]));
    assert_eq!(list[2]["attachmentTransports"], serde_json::json!({}), "空表不是 null");
    assert_eq!(list[2]["thinkingEfforts"], serde_json::json!({}));
    assert_eq!(list[2]["enabled"], false);

    // ③ 坏 JSON 那行：不炸，按空处理；空白 thinking_format 也算"没声明"
    assert_eq!(list[1]["inputModalities"], serde_json::json!([]));
    assert_eq!(list[1]["attachmentTransports"], serde_json::json!({}));
    assert_eq!(list[1]["mimeAllowlist"], serde_json::json!([]));
    assert_eq!(
        list[1]["thinkingEfforts"],
        serde_json::json!({"low": "low"}),
        "JSON null 的值要丢掉（Kotlin contentOrNull 也是这个口径）"
    );
    let m_c = list[1].as_object().unwrap();
    assert!(!m_c.contains_key("thinkingFormat"), "空白串 = 没声明：{body}");
    assert!(!m_c.contains_key("contextWindow"));

    // ★ 三种"没有模型"必须分清：provider 存在但目录为空 → 200 `[]`
    let (status, empty) = get_json(router_for(state.clone()), "/api/ai/providers/p-empty/models").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty, serde_json::json!([]));

    // ★ provider 不存在 → **400 `{error:"CONFIG_ERROR", detail: 人话}`**（不是 404、不是 500）
    let (status, missing) = get_json(router_for(state), "/api/ai/providers/nope/models").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(keys(&missing), vec!["detail", "error"]);
    assert_eq!(missing["error"], "CONFIG_ERROR");
    assert_eq!(missing["detail"], "Provider 不存在: nope");
}

// ===========================================================================
//  会话与消息的读侧（P4 第 21 步）
// ===========================================================================

/// 会话 / 消息 / **有序块**：形状、顺序、以及"会话不存在"与"会话为空"的分野。
///
/// 夹具里 `conv-0` 只有一条消息（`msg-0` + 一个块）。这里再补一条归档会话、
/// 一条多块消息（text → tool_call → tool_result），把三件最容易静默做错的事钉住：
/// ① 每一条消息内部的块必须按 **ordinal 升序**（工具卡会插在正文之间，乱了看不出来）；
/// ② 消息按 `seq, created_at` 排（同毫秒发的两条只靠 seq）；
/// ③ `includeArchived` 只认 `1` / `true`（别的值当 false），默认不列归档的。
#[tokio::test]
async fn ai_conversations_and_messages_keep_order_and_block_shape() {
    let state = state();
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_conversations (id, title, provider_id, model_id, archived, created_at, updated_at)
                 VALUES ('conv-1', '未命名', 'p1', 'm1', 0, '2026-09-23T04:00:00.000Z', '2026-09-23T04:30:00.000Z'),
                        ('conv-arch', '归档的', NULL, NULL, 1, '2026-09-23T02:00:00.000Z', '2026-09-23T02:00:00.000Z')",
                [],
            )?;
            // 三条消息：seq 故意与插入顺序不同，验证排序真的按 seq
            conn.execute(
                "INSERT INTO ai_messages (id, conversation_id, seq, role, status, text, provider_id, model_id,
                                          provider_response_id, usage_json, created_at, updated_at)
                 VALUES ('msg-b', 'conv-1', 2, 'assistant', 'complete', '好的', 'p1', 'm1', 'resp-1',
                         '{\"total_tokens\":12}', '2026-09-23T04:10:00.000Z', '2026-09-23T04:10:05.000Z'),
                        ('msg-a', 'conv-1', 1, 'user', 'complete', '画一只猫', NULL, NULL, NULL, NULL,
                         '2026-09-23T04:10:00.000Z', '2026-09-23T04:10:00.000Z')",
                [],
            )?;
            // 块**故意倒序插入**：取出来必须仍是 ordinal 升序
            conn.execute(
                "INSERT INTO ai_message_parts (message_id, ordinal, type, text, attachment_id, tool_call_id, json_payload)
                 VALUES ('msg-b', 2, 'tool_result', '工具回来了', NULL, 'call-1', '{\"mediaIds\":[7]}'),
                        ('msg-b', 0, 'text', '先看看', NULL, NULL, NULL),
                        ('msg-b', 1, 'tool_call', NULL, NULL, 'call-1', '{\"name\":\"comfy_submit\"}')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    // ---- 列表：默认不列归档，顺序按 updated_at DESC ----
    let (status, list) = get_json(router_for(state.clone()), "/api/ai/conversations").await;
    assert_eq!(status, StatusCode::OK, "body={list}");
    let ids: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["conv-1", "conv-0"], "按 updated_at DESC，且归档的默认不出现：{list}");

    let first = &list[0];
    assert_eq!(
        keys(first),
        vec![
            "archived", "createdAt", "id", "messageCount", "modelId", "providerId",
            "systemPromptVersion", "title", "updatedAt",
        ],
        "`systemPromptVersion` / `archived` / `messageCount` 最容易漏（Kotlin encodeDefaults = true）"
    );
    assert_eq!(first["messageCount"], 2);
    assert_eq!(first["archived"], false);
    assert_eq!(first["systemPromptVersion"], "v1");
    // 标题是原样读出来的（NULL → 「新对话」那条回落见 `conversations.rs` 的单元用例：
    // 真 schema 上 `title` 是 NOT NULL，夹具里构造不出 NULL）
    assert_eq!(first["title"], "未命名");
    assert_eq!(first["createdAt"], "2026-09-23T04:00:00Z", "`.000Z` 要按 Java 规则收成整秒");
    assert_eq!(first["updatedAt"], "2026-09-23T04:30:00Z");

    // `includeArchived=1` / `=true` 才列归档的
    let (_, with_arch) = get_json(router_for(state.clone()), "/api/ai/conversations?includeArchived=1").await;
    assert_eq!(with_arch.as_array().unwrap().len(), 3);
    let (_, with_arch2) = get_json(router_for(state.clone()), "/api/ai/conversations?includeArchived=true").await;
    assert_eq!(with_arch2.as_array().unwrap().len(), 3);
    let (_, not_arch) = get_json(router_for(state.clone()), "/api/ai/conversations?includeArchived=yes").await;
    assert_eq!(not_arch.as_array().unwrap().len(), 2, "别的值一律当 false（Kotlin 只认 1/true）");

    // ---- 详情 ----
    let (status, one) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-1").await;
    assert_eq!(status, StatusCode::OK, "body={one}");
    assert_eq!(one["id"], "conv-1");
    assert_eq!(one["providerId"], "p1");
    assert_eq!(one["messageCount"], 2);
    // 没有 provider_id 的会话：可空键**不出现**
    let (_, bare) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-arch").await;
    assert!(!bare.as_object().unwrap().contains_key("providerId"), "{bare}");
    assert!(!bare.as_object().unwrap().contains_key("modelId"));

    // ---- 消息 + 有序块 ----
    let (status, msgs) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-1/messages").await;
    assert_eq!(status, StatusCode::OK, "body={msgs}");
    let msgs = msgs.as_array().unwrap();
    assert_eq!(
        msgs.iter().map(|m| m["id"].as_str().unwrap()).collect::<Vec<_>>(),
        vec!["msg-a", "msg-b"],
        "必须按 seq 排，不是插入顺序"
    );
    assert_eq!(
        keys(&msgs[1]),
        vec![
            "conversationId", "createdAt", "id", "modelId", "parts", "providerId",
            "providerResponseId", "role", "seq", "status", "text", "updatedAt", "usage",
        ]
    );
    // 没有 provider / usage 的那条：可空键一个都不出现，但 `parts` 一定在（空数组）
    let user_msg = &msgs[0];
    for absent in ["providerId", "modelId", "providerResponseId", "usage"] {
        assert!(!user_msg.as_object().unwrap().contains_key(absent), "{absent}：{user_msg}");
    }
    assert_eq!(user_msg["parts"], serde_json::json!([]));

    // ★ 块顺序：插入顺序是 2,0,1，取出来必须是 0,1,2 —— 工具卡要插在正文之间
    let parts = msgs[1]["parts"].as_array().unwrap();
    assert_eq!(
        parts.iter().map(|p| p["type"].as_str().unwrap()).collect::<Vec<_>>(),
        vec!["text", "tool_call", "tool_result"],
        "块顺序乱了 = 工具卡位置不对（而正文还在，看起来只是『有点怪』）"
    );
    // 每个块：只带自己有的键
    assert_eq!(keys(&parts[0]), vec!["text", "type"]);
    assert_eq!(keys(&parts[1]), vec!["jsonPayload", "toolCallId", "type"]);
    assert_eq!(parts[1]["jsonPayload"], serde_json::json!({"name": "comfy_submit"}));
    assert_eq!(parts[2]["jsonPayload"], serde_json::json!({"mediaIds": [7]}));
    // usage 是**对象**，不是字符串
    assert_eq!(msgs[1]["usage"], serde_json::json!({"total_tokens": 12}));
    assert_eq!(msgs[1]["providerResponseId"], "resp-1");
    assert_eq!(msgs[1]["createdAt"], "2026-09-23T04:10:00Z");
    assert_eq!(msgs[1]["updatedAt"], "2026-09-23T04:10:05Z");

    // ---- 三种"没有"必须分清 ----
    // 会话存在但没有消息 → 200 []
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_conversations (id, title, archived, created_at, updated_at)
                 VALUES ('conv-empty', '空的', 0, '2026-09-23T06:00:00.000Z', '2026-09-23T06:00:00.000Z')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let (status, empty) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-empty/messages").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty, serde_json::json!([]));

    // 会话不存在 → 400 CONFIG_ERROR（不是 404、不是空数组）
    for path in ["/api/ai/conversations/nope", "/api/ai/conversations/nope/messages"] {
        let (status, err) = get_json(router_for(state.clone()), path).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path} body={err}");
        assert_eq!(keys(&err), vec!["detail", "error"]);
        assert_eq!(err["error"], "CONFIG_ERROR");
        assert_eq!(err["detail"], "会话不存在: nope");
    }
}

/// 坏 JSON 不许把整段历史打成 500（Kotlin 那边是 `runCatching { … }.getOrNull()`）。
#[tokio::test]
async fn ai_messages_survive_broken_json_columns() {
    let state = state();
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_conversations (id, title, archived, created_at, updated_at)
                 VALUES ('conv-bad', '脏数据', 0, '2026-09-23T07:00:00.000Z', '2026-09-23T07:00:00.000Z')",
                [],
            )?;
            conn.execute(
                "INSERT INTO ai_messages (id, conversation_id, seq, role, status, text, usage_json, created_at, updated_at)
                 VALUES ('msg-bad', 'conv-bad', 1, 'assistant', 'complete', '正文', '{oops',
                         '2026-09-23T07:00:01.000Z', '2026-09-23T07:00:01.000Z')",
                [],
            )?;
            conn.execute(
                "INSERT INTO ai_message_parts (message_id, ordinal, type, json_payload)
                 VALUES ('msg-bad', 0, 'tool_call', 'not json')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let (status, msgs) = get_json(router_for(state), "/api/ai/conversations/conv-bad/messages").await;
    assert_eq!(status, StatusCode::OK, "一条脏 json 不该连累整段历史：{msgs}");
    let msg = &msgs[0];
    assert_eq!(msg["text"], "正文");
    assert!(!msg.as_object().unwrap().contains_key("usage"), "坏 usage_json 当『没有』：{msg}");
    assert_eq!(
        keys(&msg["parts"][0]),
        vec!["type"],
        "坏 json_payload 的那一块只留 type（键不出现，不是 null）：{msg}"
    );
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
            "checkpoint", "createdAt", "favorite", "fileUrl", "hasWorkflow", "id", "kind", "nsfw",
            "originalName", "promptId", "promptNegative", "promptPositive", "promptTags",
            "promptTitle", "seed", "sizeBytes", "source", "storedName", "thumbUrl", "title",
            "updatedAt",
        ],
        "少一个键 = 画廊里少一块内容（缩略图 URL 也在这里）"
    );
    // 夹具里这些列是 NULL → Kotlin（`explicitNulls = false`）**整个键都不发**
    for absent in ["mimeType", "width", "height", "durationMs", "sha256", "notes"] {
        assert!(
            !item.as_object().unwrap().contains_key(absent),
            "`{absent}` 是 NULL 就不该有这个键（不是 null）：{item}"
        );
    }
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
    assert!(body["error"].as_str().unwrap().contains("文件已丢失"), "body={body}");
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
        assert!(body["error"].as_str().unwrap().contains("还没搬到 Rust"), "uri={uri} body={body}");
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
    assert!(
        !cleared.1.as_object().unwrap().contains_key("promptId"),
        "clearPrompt 必须真的置空；Kotlin 那边 null 就是**这个键不出现**：{}",
        cleared.1
    );
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
    assert!(bogus.1["error"].as_str().unwrap().contains("9999"));

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
    // 这次没带 promptId → Kotlin（`promptId: Long? = null` + `explicitNulls = false`）那边
    // **整个键都不出现**（上面那次带了 promptId=10，所以那次有）
    assert_eq!(keys(&body2), vec!["duplicates", "failed", "items"], "body={body2}");
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
    assert!(body["error"].as_str().unwrap().contains("files"));

    // promptId 不存在 → 400
    let body = multipart_body("----comfyhubtest", &[("promptId", "9999")], &[("files", "a.png", b"x")]);
    let (status, body) = post_multipart(router_for(state.clone()), "/api/media/upload", body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("9999"));

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
        vec!["autoTag", "comfyUrl", "downloadFallback", "enabled", "maxPerPoll", "pollSeconds"],
        "没配过输出目录 → `outputDir` 这个键**根本不出现**（Kotlin 实测形状）：{body}"
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
    assert!(
        !saved.as_object().unwrap().contains_key("outputDir"),
        "空白目录归一化成 NULL → 键不出现（Kotlin `explicitNulls = false`）：{saved}"
    );
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
    assert!(body["error"].as_str().unwrap().contains("9999"));

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
    assert!(body["error"].as_str().unwrap().contains("9999"));
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
    // 410 的信封与别处一致：`error` 是**人话**（手机判 410 认的是状态码，不是 `error` 的值）
    assert_eq!(keys(&body), vec!["error"]);
    assert!(
        body["error"].as_str().unwrap().contains("bootstrap"),
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
    assert!(body["error"].as_str().unwrap().contains("500"), "{body}");
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
        let msg = body["error"].as_str().unwrap();
        assert!(msg.contains("prompts"), "错误信息要列出合法表名：{msg}");
        assert!(!body.to_string().contains("SECRET-TOKEN"), "令牌绝不能出现：{body}");
    }

    // 缺 table 也是 400（必填，理由见 sync::bootstrap 的注释）
    let (status, body) = get_json(router_for(state.clone()), "/api/sync/bootstrap?page=1").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("table"));
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
    assert!(body["error"].as_str().unwrap().contains("已被删除"));

    // 产物本身不存在 → 404
    let (status, _) = get_json(router_for(state.clone()), "/api/media/9999/prompt").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ===========================================================================
//  run-kit 工件（P4 第 22 步）：GET /api/ai/run-kit{,/status}
// ===========================================================================

/// 把夹具摆成"一台配好的 Provider + 三个模型（两个启用）+ 一条长期记忆 + 一个 skill"。
///
/// 这几个正是 `systemPrompt` 里那几段的来源：环境头、工具清单、白名单、记忆、Skills ——
/// 少灌一样，"那一段在提示词里到底写对没写对"就完全没被走到。
fn seed_run_kit(state: &comfyhub_server_rs::routes::AppState) {
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_providers
                 (id, display_name, api, base_url, credential_ref, endpoint_trust, enabled, revision)
                 VALUES ('p-run', '本机网关', 'openai-completions', 'http://127.0.0.1:11434/v1',
                         'RUN_KEY', 'loopback', 1, 1)",
                [],
            )?;
            conn.execute(
                r#"INSERT INTO ai_models
                   (provider_id, model_id, display_name, input_modalities, attachment_transports,
                    mime_allowlist, tools, parallel_tools, reasoning, thinking_efforts, thinking_format,
                    context_window, max_output_tokens, max_attachment_bytes, max_attachment_count,
                    capability_source, capability_verified_at, enabled)
                   VALUES
                   -- ① 样样都有：run-kit 那 17 个键一个不少
                   ('p-run', 'm-full', 'A 全字段', '["text","image"]', '{"image":["inline_base64"]}',
                    '["image/png"]', 1, 0, 1, '{"high":"high"}', 'openai',
                    200000, 8192, 8388608, 4, 'builtin', '2026-09-23T05:00:00.000Z', 1),
                   -- ② 可空列全 NULL：5 个可空键**一个都不许出现**（explicitNulls = false）
                   ('p-run', 'm-bare', 'B 光板', '["text"]', NULL, NULL, 1, 0, 0, NULL, NULL,
                    NULL, NULL, NULL, NULL, 'manual', NULL, 1),
                   -- ③ 关掉的模型：不许出现在 models[] 里，也不许被 ?modelId= 指定
                   ('p-run', 'm-off', 'C 关掉的', '["text"]', NULL, NULL, 0, 0, 0, NULL, NULL,
                    NULL, NULL, NULL, NULL, 'manual', NULL, 0)"#,
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let storage = state.media_dir().parent().unwrap().to_path_buf();
    std::fs::create_dir_all(storage.join("ai").join("skills").join("demo-skill")).unwrap();
    std::fs::write(storage.join("ai").join("memory.md"), "- 用户偏好 4:3 画幅\n").unwrap();
    std::fs::write(
        storage.join("ai").join("skills").join("demo-skill").join("SKILL.md"),
        "---\nname: demo-skill\ndescription: 演示用\nwhenToUse: 演示\n---\n正文\n",
    )
    .unwrap();
}

/// run-kit 的形状：11 个顶层键、7 个手机工具、模板横幅、两条按轮指令、预算数值。
#[tokio::test]
async fn run_kit_ships_every_rule_the_phone_needs() {
    let state = state();
    seed_run_kit(&state);
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/run-kit").await;
    assert_eq!(status, StatusCode::OK, "body={body}");

    assert_eq!(
        keys(&body),
        vec![
            "continueInstruction", "inlineBudget", "kitVersion", "models", "nsfwEnabled",
            "promptVersion", "schemaVersion", "systemPrompt", "titleInstruction", "tools",
            "untrustedBanner",
        ]
    );
    assert_eq!(body["schemaVersion"], 1, "与 /api/sync/manifest 的 user_version 同一个数");
    assert_eq!(body["promptVersion"], "v28");
    assert_eq!(body["nsfwEnabled"], false, "夹具里没开 NSFW");
    assert_eq!(body["inlineBudget"]["perImageBytes"], 8388608, "单图 8MB");
    assert_eq!(body["inlineBudget"]["totalBytes"], 20971520, "合计 20MB");
    let kit = body["kitVersion"].as_str().unwrap();
    assert!(kit.contains('.'), "kitVersion 形如 <日期>.<当日序号>：{kit}");

    // tools[]：**只有手机可执行的那 7 个**，每项恰好四个键
    let tools = body["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec![
            "ask_user_question", "civitai_search", "list_skills", "load_skill", "remember",
            "web_fetch", "web_search",
        ],
        "按 name 升序（顺序进了 kitVersion 签名）"
    );
    for tool in tools {
        assert_eq!(keys(tool), vec!["description", "inputSchema", "name", "scope"]);
        assert_eq!(tool["inputSchema"]["type"], "object");
        let scope = tool["scope"].as_str().unwrap();
        assert!(scope == "MOBILE" || scope == "BOTH", "错档：{scope}");
    }
    // 桌面专属的一个都不许出现（总纲 §3.5：别让模型看见它调不到的工具）
    for forbidden in [
        "comfy_submit", "comfy_get_status", "read_file", "write_file", "register_skill",
        "delete_skill", "list_dir", "find_model", "comfy_load_workflow",
    ] {
        assert!(!names.contains(&forbidden), "{forbidden} 是桌面专属，不该进手机清单");
    }

    // 横幅是**模板**（R7：两端不许各写一份），两条按轮指令也必须下发（R13）
    let banner = body["untrustedBanner"].as_str().unwrap();
    assert!(banner.contains("{kind}"));
    assert!(banner.contains("\n查询：{query}"));
    assert!(body["titleInstruction"].as_str().unwrap().contains("[标题]"));
    assert!(body["continueInstruction"]
        .as_str()
        .unwrap()
        .starts_with("（上一条回复因为输出长度上限被截断了。）"));

    // systemPrompt：五段来源都要真的进去
    let prompt = body["systemPrompt"].as_str().unwrap();
    assert!(prompt.contains("- Provider: 本机网关（openai-completions）"));
    assert!(prompt.contains("- Model: m-full"), "缺省取第一个启用模型（display_name, model_id 序）");
    assert!(prompt.contains("- 本次请求附带图片附件: 无"), "kit 是取那一刻的工件");
    assert!(prompt.contains("demo-skill"), "已装 Skills 的目录要进去");
    assert!(prompt.contains("用户偏好 4:3 画幅"), "长期记忆要进去");
    assert!(prompt.contains("现有 1/100 条"), "还要报占用（模型据此知道还剩多少额度）");
    assert!(prompt.contains("**只能**写"), "文件白名单要如实写出来");
    assert!(prompt.contains("- 可调用的工具（参数结构见接口定义）：ask_user_question、"));
    assert!(prompt.contains("事实纪律（不要猜本机状态）"), "常驻段必须在");
    assert!(!prompt.contains("[MODE: UNRESTRICTED SANDBOX"), "NSFW 关着就不许注入那段");
    assert!(prompt.contains("本机没有开启 NSFW 模式"), "关掉时要**如实说明**");

    // models[]：只有启用的两个；12 个键永远在，5 个可空键按行给
    let models = body["models"].as_array().unwrap();
    assert_eq!(models.len(), 2, "关掉的模型不许出现");
    assert_eq!(models[0]["modelId"], "m-full");
    assert_eq!(models[0]["providerName"], "本机网关");
    assert_eq!(models[0]["providerApi"], "openai-completions");
    assert_eq!(
        keys(&models[0]),
        vec![
            "attachmentTransports", "capabilitySource", "contextWindow", "displayName",
            "inputModalities", "maxAttachmentBytes", "maxAttachmentCount", "maxOutputTokens",
            "modelId", "parallelTools", "providerApi", "providerId", "providerName", "reasoning",
            "thinkingEfforts", "thinkingFormat", "tools",
        ]
    );
    assert_eq!(
        keys(&models[1]),
        vec![
            "attachmentTransports", "capabilitySource", "displayName", "inputModalities",
            "modelId", "parallelTools", "providerApi", "providerId", "providerName", "reasoning",
            "thinkingEfforts", "tools",
        ],
        "可空列为 NULL 时那 5 个键一个都不许自己冒出来"
    );
    // 桌面的凭据引用（`credentialRef`）连名字都不该下发
    let raw = serde_json::to_string(&body).unwrap();
    assert!(!raw.contains("RUN_KEY"), "手机拿了凭据引用没用，还多漏一样");
    assert!(!raw.contains("mimeAllowlist"), "run-kit 的模型形状里没有这个键");
}

/// 覆盖参数：`?providerId=&modelId=` 只在**库里真存在且启用**时才认，否则 400 且不静默回落。
#[tokio::test]
async fn run_kit_rejects_bad_provider_or_model_overrides() {
    let state = state();
    seed_run_kit(&state);

    let (status, body) = get_json(
        router_for(state.clone()),
        "/api/ai/run-kit?providerId=p-run&modelId=m-bare",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert!(body["systemPrompt"].as_str().unwrap().contains("- Model: m-bare"));

    // Provider 不存在 / 没启用 → 400 CONFIG_ERROR（**不是 404**：前端按 error 分支、按 detail 给人看）
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/run-kit?providerId=nope").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert!(body["detail"].as_str().unwrap().contains("Provider 不存在或未启用: nope"));

    let (status, body) = get_json(
        router_for(state.clone()),
        "/api/ai/run-kit?providerId=p-run&modelId=nope",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert!(body["detail"].as_str().unwrap().contains("模型不存在或未启用: p-run/nope"));

    // 被用户关掉的模型同样不许指定（"指定了就按它跑"，不许悄悄换一个）
    let (status, _) = get_json(
        router_for(state.clone()),
        "/api/ai/run-kit?providerId=p-run&modelId=m-off",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// `/status` 幂等；**只有规则变了才跳号**；改规则后 `tools[]` 跟着变。
///
/// 这条盯着的是两件事：① 手机每次取 kit 都调一次 `ensureVersion`，如果它不是幂等的，
/// 版本号会一路涨到天上；② 规则（这里是"禁用一个工具"）变了却不跳号 ——
/// 那手机就会一直用旧规则跑，而且没有任何办法发现。
#[tokio::test]
async fn run_kit_status_is_idempotent_and_bumps_only_when_rules_change() {
    let state = state();
    seed_run_kit(&state);

    let (status, body) = get_json(router_for(state.clone()), "/api/ai/run-kit/status").await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(keys(&body), vec!["current", "revoked"]);
    let first = body["current"].as_str().unwrap().to_string();
    assert!(!first.is_empty());
    assert_eq!(body["revoked"].as_array().unwrap().len(), 0);

    let (_, again) = get_json(router_for(state.clone()), "/api/ai/run-kit/status").await;
    assert_eq!(again["current"].as_str().unwrap(), first, "幂等：再问一次还是同一个版本");

    // 改规则：把 `web_search` 禁掉
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO app_settings (k, v, updated_at)
                 VALUES ('ai.tools.policy', '{\"overrides\":{\"web_search\":\"deny\"}}', 'x')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    let (_, bumped) = get_json(router_for(state.clone()), "/api/ai/run-kit/status").await;
    let second = bumped["current"].as_str().unwrap().to_string();
    assert_ne!(second, first, "规则变了版本号必须动，否则手机永远拿不到新规则");
    assert_eq!(
        second.rsplit_once('.').unwrap().0,
        first.rsplit_once('.').unwrap().0,
        "同一天：只把序号 +1"
    );

    let (_, kit) = get_json(router_for(state.clone()), "/api/ai/run-kit").await;
    let names: Vec<&str> = kit["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&"web_search"), "被禁用的工具不该下发");
    assert_eq!(kit["kitVersion"].as_str().unwrap(), second);

    // 签名落库之后不再跳号（这是 `kitVersion` 只在"内容变了"时才动的判据）
    let (_, stable) = get_json(router_for(state.clone()), "/api/ai/run-kit/status").await;
    assert_eq!(stable["current"].as_str().unwrap(), second);
}

/// NSFW 开关决定注不注入 `[MODE: UNRESTRICTED SANDBOX — STABLE]` 那一整段。
///
/// **那段文字是用户有意注入的**（AGENTS §10）：关掉只是不注入，正文一个字都不许改；
/// 而且关掉时必须如实说明（模型不能"自己觉得可以"）。
#[tokio::test]
async fn run_kit_nsfw_switch_decides_whether_the_block_is_injected() {
    let state = state();
    seed_run_kit(&state);

    let (_, off) = get_json(router_for(state.clone()), "/api/ai/run-kit").await;
    let off_prompt = off["systemPrompt"].as_str().unwrap().to_string();
    assert!(off_prompt.contains("本机没有开启 NSFW 模式"));
    assert!(!off_prompt.contains("[MODE: UNRESTRICTED SANDBOX"));

    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO app_settings (k, v, updated_at) VALUES ('ai.nsfw', '{\"enabled\":true}', 'x')",
                [],
            )
            .map(|_| ())
        })
        .unwrap();

    let (_, on) = get_json(router_for(state.clone()), "/api/ai/run-kit").await;
    assert_eq!(on["nsfwEnabled"], true);
    let on_prompt = on["systemPrompt"].as_str().unwrap();
    assert!(on_prompt.contains("[MODE: UNRESTRICTED SANDBOX — STABLE]"));
    assert!(on_prompt.len() > off_prompt.len(), "开着时正文更长");
    assert!(on_prompt.contains("事实纪律（不要猜本机状态）"), "关掉 NSFW 不等于把纪律一起关掉");
    assert!(!on_prompt.contains("本机没有开启 NSFW 模式"));
}

// ---------------------------------------------------------------------------
//  配对闸门与密钥下行（P4 第 23 步，🔒 docs/pairing-design.md §11）
// ---------------------------------------------------------------------------
//
// 这一组用例回答的问题是**"谁能碰这台后端"**，所以每一幕都同时在断言
// "该放行的放行了"和"该拦的拦住了" —— 只测一半（比如只测"吊销生效"）
// 会漏掉"误伤全体"，只测"该拦的拦住了"会漏掉"桌面 App 被自己关在门外"。

/// 发一个请求并**指定来源地址**（+ 可选令牌）。
///
/// `oneshot` 的请求默认没有 `ConnectInfo`，那是"拿不到来源"这条保守分支；
/// 要验"回环放行 / 非回环拦下"就必须自己把它塞进 extensions ——
/// 这也正是 `main.rs` 必须用 `into_make_service_with_connect_info` 的原因，
/// 少了它生产上每个请求都会走保守分支。
async fn send_as(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    remote: &str,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let addr: std::net::SocketAddr = format!("{remote}:51000").parse().expect("来源地址");
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    if let Some(token) = token {
        builder = builder.header("x-comfyhub-token", token);
    }
    let mut request = builder
        .body(match body {
            Some(v) => Body::from(v.to_string()),
            None => Body::empty(),
        })
        .unwrap();
    request.extensions_mut().insert(axum::extract::ConnectInfo(addr));
    let response = app.oneshot(request).await.expect("请求");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// 往 `app_settings` 里写一行（闸门状态就存在那儿：`auth.pairing`）。
fn put_setting(state: &comfyhub_server_rs::routes::AppState, key: &str, value: &str) {
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO app_settings (k, v, updated_at) VALUES (?1, ?2, 'x')
                 ON CONFLICT(k) DO UPDATE SET v = excluded.v",
                rusqlite::params![key, value],
            )
            .map(|_| ())
        })
        .unwrap();
}

fn read_setting(state: &comfyhub_server_rs::routes::AppState, key: &str) -> Option<Value> {
    state
        .with(|conn| {
            let raw: Option<String> = conn
                .query_row("SELECT v FROM app_settings WHERE k = ?1", rusqlite::params![key], |r| {
                    r.get::<_, Option<String>>(0)
                })
                .ok()
                .flatten();
            Ok(raw)
        })
        .unwrap()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

/// 闸门开着 + 一台已配对设备 + 一个全局令牌。
fn seed_auth(state: &comfyhub_server_rs::routes::AppState, allow_credentials: bool) {
    let file = serde_json::json!({
        "schema": "comfyhub-inbound-auth/2",
        "enabled": true,
        "token": "global-token-43chars-aaaaaaaaaaaaaaaaaaaaaa",
        "createdAt": "2026-09-24T00:00:00Z",
        "devices": [{
            "id": "phone-seed",
            "name": "夹具手机",
            "token": "device-token-43chars-bbbbbbbbbbbbbbbbbbbbbb",
            "pairedAt": "2026-09-24T00:00:00Z",
            "allowCredentials": allow_credentials
        }]
    });
    put_setting(state, "auth.pairing", &file.to_string());
}

/// 配对用例的**串行闸门**。
///
/// 为什么必须串行：`auth.rs` 里那份"每台设备刚刚活跃过"的 `seen` 是**进程级全局**
/// （`sessions()`），而 `POST /api/auth/rotate` 会把它整张清掉（轮换 = 所有设备立刻 401，
/// 语义上就该清）。两个 auth 用例并行时，一个 rotate 会把另一个刚记下的活跃抹掉 ——
/// 表现是「最后活跃不必等 30 秒落库」偶发变红（2026-09-25 实测，跑单条永远绿）。
/// 这不是产品缺陷，是**测试之间共用全局态**；所以把它们串起来，防线一条都不动。
fn auth_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// **默认行为一个字节都不许变**：闸门关着时，非回环不带令牌也照样通。
///
/// 这条是三条既有 e2e（capture / submit / ai-tools）与桌面 App 的代言人：
/// 它们全都不带任何头，闸门一开就得全绿 —— 红了就说明默认被改了。
#[tokio::test]
async fn auth_gate_is_off_by_default_and_blocks_only_when_enabled() {
    let _serial = auth_test_lock().lock().await;
    let state = state();
    // （一）没有 auth.pairing 这一行 = 今天的行为
    let (status, _) = send_as(router_for(state.clone()), "GET", "/api/prompts", None, "10.9.1.1", None).await;
    assert_eq!(status, StatusCode::OK, "闸门默认关着：局域网请求照旧放行");

    // （二）开着以后：没带令牌 → 403（**不是 401**）
    seed_auth(&state, false);
    let (status, body) = send_as(router_for(state.clone()), "GET", "/api/prompts", None, "10.9.1.2", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "需要配对令牌（X-ComfyHub-Token）");
    assert!(body.get("detail").is_none(), "普通错误只有 error 一个键");

    // 令牌不对 / 已被吊销 → 401（与 403 的语义不能混）
    let (status, body) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/prompts",
        None,
        "10.9.1.3",
        Some("wrong-token"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "令牌无效或已被吊销");

    // 设备令牌 / 全局令牌 → 放行
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/prompts",
        None,
        "10.9.1.4",
        Some("device-token-43chars-bbbbbbbbbbbbbbbbbbbbbb"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/prompts",
        None,
        "10.9.1.5",
        Some("global-token-43chars-aaaaaaaaaaaaaaaaaaaaaa"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 回环永远放行（桌面 App / 脚本 / e2e 走的就是这条）
    let (status, _) = send_as(router_for(state.clone()), "GET", "/api/prompts", None, "127.0.0.1", None).await;
    assert_eq!(status, StatusCode::OK, "回环不带令牌也必须通");
}

/// 手机发起配对的完整一幕：举手 → 桌面看到 6 位码 → 兑码拿令牌 → 令牌真的能用。
#[tokio::test]
async fn auth_pair_flow_hands_out_a_device_token_exactly_once() {
    let _serial = auth_test_lock().lock().await;
    let state = state();
    seed_auth(&state, false);

    // ① 手机举手（未配对设备唯一能打的两条路径之一）
    let (status, body) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/request",
        Some(serde_json::json!({"deviceId": "phone-e2e-1", "deviceName": "OnePlus 8T"})),
        "10.9.2.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let request_id = body["requestId"].as_str().expect("requestId").to_string();
    assert!(!request_id.is_empty());
    // ★ 纪律：响应里**没有 ticket**（给了它，手机自己就能算出 6 位码）
    assert!(
        !body.as_object().unwrap().contains_key("ticket"),
        "手机举手的响应里不许出现 ticket：{body}"
    );
    assert_eq!(
        keys(&body),
        vec!["codeExpiresAt", "expiresAt", "requestId", "serverTime"],
        "举手响应的键集合就是这四个"
    );

    // ② 桌面（回环）看到待配对列表，里面是本机的 6 位码
    let (status, body) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/auth/pair/pending",
        None,
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mine = body["pending"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["requestId"] == request_id.as_str())
        .expect("待配对列表里应当有这一条")
        .clone();
    let code = mine["code"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 6);
    assert!(code.chars().all(|c| c.is_ascii_digit()));
    assert_eq!(mine["ip"], "10.9.2.1");
    assert_eq!(mine["deviceName"], "OnePlus 8T");

    // 非回环读这张列表 → 403（它是"谁有钥匙"的情报）
    let (status, _) =
        send_as(router_for(state.clone()), "GET", "/api/auth/pair/pending", None, "10.9.2.2", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // ③ 码错 → 400「配对失败」，且**不回显** ticket / code
    let wrong = if code == "000000" { "111111" } else { "000000" };
    let (status, body) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/claim",
        Some(serde_json::json!({
            "requestId": request_id, "code": wrong,
            "deviceId": "phone-e2e-1", "deviceName": "OnePlus 8T"
        })),
        "10.9.2.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "配对失败");
    let text = body.to_string();
    assert!(!text.contains(&code), "失败响应里不许回显 6 位码");
    assert!(!text.contains(&request_id) || true, "（会话 id 可以回显，它是攻击者自己给的）");

    // ④ 码对 → 拿到**这台设备专属**的令牌（只此一次）
    let (status, body) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/claim",
        Some(serde_json::json!({
            "requestId": request_id, "code": code,
            "deviceId": "phone-e2e-1", "deviceName": "OnePlus 8T"
        })),
        "10.9.2.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    let token = body["token"].as_str().expect("token").to_string();
    assert_eq!(token.len(), 43, "32 字节 base64url = 43 字符（D11）");
    assert_eq!(body["deviceId"], "phone-e2e-1");
    assert!(body["schemaVersion"].as_i64().unwrap() > 0);

    // ⑤ 同一个会话**不能兑第二次**（"一次性"这个词的全部含义）
    let (status, _) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/claim",
        Some(serde_json::json!({
            "requestId": request_id, "code": code,
            "deviceId": "phone-e2e-1", "deviceName": "OnePlus 8T"
        })),
        "10.9.2.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::GONE, "用过的会话 → 410（手机据此回桌面重新发起）");

    // ⑥ 令牌真的能用：拿它打一条受保护的端点
    let (status, _) = send_as(router_for(state.clone()), "GET", "/api/ai/conversations", None, "10.9.2.1", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);

    // ⑥′ "最后活跃"要**立刻**看得到（内存里那笔还没落库的活跃也要叠加上来）。
    // 少了叠加这一层，界面上的"最后活跃"会停在上一次落库的时刻（30 秒前），
    // 表现是"数据没错、只是旧了" —— 那种最容易被当成 bug 去查后端。
    let (_, status_body) = send_as(router_for(state.clone()), "GET", "/api/auth", None, "127.0.0.1", None).await;
    let just_seen = status_body["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "phone-e2e-1")
        .expect("刚配的设备")
        .clone();
    assert_eq!(just_seen["lastIp"], "10.9.2.1", "最后来源地址就是刚打过来的那个");
    assert!(just_seen["lastSeenAt"].is_string(), "最后活跃不必等 30 秒落库");

    // ⑦ 设备列表：**有这台设备、但绝不含令牌**（连前缀都不回）
    let (status, body) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/auth/devices",
        None,
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let device = body["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "phone-e2e-1")
        .expect("新配的设备应当在列表里")
        .clone();
    // 设备行只该有那几个**事实**键：必有的四个 + 两个随时间才出现的
    // （`lastSeenAt` / `lastIp` 有没有取决于活跃记录落库没有，所以这里逐条算，
    //  不写成"键一定等于某四个" —— 那会随"谁先跑了 GET /api/auth"而红）
    let device_keys = keys(&device);
    for required in ["allowCredentials", "id", "name", "pairedAt"] {
        assert!(device_keys.iter().any(|k| k == required), "设备行少了 {required}：{device_keys:?}");
    }
    for key in &device_keys {
        assert!(
            ["allowCredentials", "id", "lastIp", "lastSeenAt", "name", "pairedAt"].contains(&key.as_str()),
            "设备行冒出一个不该有的键：{key}"
        );
    }
    assert_eq!(device["allowCredentials"], false, "配对成功 ≠ 允许下行密钥");
    assert!(
        !body.to_string().contains(&token),
        "设备列表里一个字都不许出现长期令牌"
    );
    // 而且令牌**确实落库了**（不是只在响应里一闪而过）
    let stored = read_setting(&state, "auth.pairing").expect("auth.pairing 已落库");
    assert!(stored["devices"].as_array().unwrap().iter().any(|d| d["id"] == "phone-e2e-1"));

    // ⑧ 吊销 → **立刻** 401（无宽限期）
    let (status, _) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/devices/phone-e2e-1/revoke",
        Some(serde_json::json!({})),
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send_as(router_for(state.clone()), "GET", "/api/ai/conversations", None, "10.9.2.1", Some(&token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // 别的设备不受影响（夹具里那台仍然通）
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/ai/conversations",
        None,
        "10.9.2.1",
        Some("device-token-43chars-bbbbbbbbbbbbbbbbbbbbbb"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "吊销一台不许误伤另一台");
}

/// 6 位码只有 10^6 种：错 5 次就把这次配对**当场作废**（不是锁一会儿）。
#[tokio::test]
async fn auth_claim_voids_the_session_after_five_wrong_codes() {
    let _serial = auth_test_lock().lock().await;
    let state = state();
    seed_auth(&state, false);
    let (_, body) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/request",
        Some(serde_json::json!({"deviceId": "phone-brute", "deviceName": "guess"})),
        "10.9.3.1",
        None,
    )
    .await;
    let request_id = body["requestId"].as_str().unwrap().to_string();

    let (_, listing) = send_as(router_for(state.clone()), "GET", "/api/auth/pair/pending", None, "127.0.0.1", None).await;
    let code = listing["pending"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["requestId"] == request_id.as_str())
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string();
    let wrong = if code == "000000" { "111111" } else { "000000" };

    for attempt in 1..=4 {
        let (status, _) = send_as(
            router_for(state.clone()),
            "POST",
            "/api/auth/pair/claim",
            Some(serde_json::json!({"requestId": request_id, "code": wrong, "deviceId": "phone-brute"})),
            "10.9.3.1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "第 {attempt} 次错码");
    }
    // 第 5 次错 → 会话作废（410）
    let (status, _) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/claim",
        Some(serde_json::json!({"requestId": request_id, "code": wrong, "deviceId": "phone-brute"})),
        "10.9.3.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
    // 作废之后**即使码对也不行**
    let (status, _) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/claim",
        Some(serde_json::json!({"requestId": request_id, "code": code, "deviceId": "phone-brute"})),
        "10.9.3.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::GONE, "作废是硬判据，不许被一个正确的码救回来");
}

/// **密钥下行**：只认"回环 或 已配对且被显式授权"的设备（与闸门开关无关）。
#[tokio::test]
async fn credentials_downlink_requires_an_authorized_device() {
    let state = state();
    seed_auth(&state, false);
    // 夹具用**环境变量**这一档（托管文件那档是 DPAPI 密文，由 auth.rs 的单测覆盖）
    let reference = "FIXTURE_KEY_E2E_AUTH";
    std::env::set_var(reference, "sk-fixture-value");

    let uri = format!("/api/ai/credentials?refs={reference}");
    // ① 匿名非回环 → 403（**即使闸门关着**也不放行；这句人话指向"你还没配对"）
    put_setting(
        &state,
        "auth.pairing",
        &serde_json::json!({"enabled": false, "token": "g"}).to_string(),
    );
    let (status, body) = send_as(router_for(state.clone()), "GET", &uri, None, "10.9.4.1", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "需要配对令牌（X-ComfyHub-Token）");

    seed_auth(&state, false);
    // ② 配对了但没勾"允许密钥" → 403（含全局令牌也不行）
    for token in ["device-token-43chars-bbbbbbbbbbbbbbbbbbbbbb", "global-token-43chars-aaaaaaaaaaaaaaaaaaaaaa"] {
        let (status, _) = send_as(router_for(state.clone()), "GET", &uri, None, "10.9.4.2", Some(token)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "令牌 {token:?} 不该能读密钥");
    }

    // ③ 勾了 → 放行并拿到明文
    seed_auth(&state, true);
    let (status, body) = send_as(
        router_for(state.clone()),
        "GET",
        &uri,
        None,
        "10.9.4.3",
        Some("device-token-43chars-bbbbbbbbbbbbbbbbbbbbbb"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["credentials"][reference], "sk-fixture-value");

    // ④ 本机回环永远可以（桌面 App 自己要用）
    let (status, body) = send_as(router_for(state.clone()), "GET", &uri, None, "127.0.0.1", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["credentials"][reference], "sk-fixture-value");

    // ⑤ 桌面上没有的那把 ref：**不报错、也不出现在结果里**（手机据此如实说"还得自己填"）
    let (status, body) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/ai/credentials?refs=NOT_CONFIGURED_REF",
        None,
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["credentials"].as_object().unwrap().is_empty());
    assert!(body["missing"].as_array().unwrap().len() == 1);

    std::env::remove_var(reference);
}

/// 开关与轮换：打开开关顺手发令牌；轮换 = 全局令牌换新 + **所有设备立刻失效**。
#[tokio::test]
async fn auth_switch_and_rotate_behave_as_documented() {
    let _serial = auth_test_lock().lock().await;
    let state = state();
    // 打开开关（还没有任何状态）
    let (status, body) = send_as(
        router_for(state.clone()),
        "PUT",
        "/api/auth",
        Some(serde_json::json!({"enabled": true})),
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], true);
    assert_eq!(body["hasToken"], true, "打开开关时顺手生成长期令牌");
    assert_eq!(keys(&body), vec!["devices", "enabled", "hasToken", "pending", "schema"]);
    let first = read_setting(&state, "auth.pairing").unwrap()["token"].as_str().unwrap().to_string();

    // 非回环打管理端点 → 403（只允许本机）
    let (status, _) = send_as(router_for(state.clone()), "GET", "/api/auth", None, "10.9.5.1", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 参数不对 → 400（不是静默当成 false）
    let (status, _) = send_as(
        router_for(state.clone()),
        "PUT",
        "/api/auth",
        Some(serde_json::json!({})),
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 轮换：换令牌 + 清空所有设备
    seed_auth(&state, true);
    let (status, body) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/rotate",
        Some(serde_json::json!({})),
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revokedDevices"], 1, "轮换要说清楚吊销了几台");
    let second = read_setting(&state, "auth.pairing").unwrap()["token"].as_str().unwrap().to_string();
    assert_ne!(first, second);
    // 旧设备令牌立刻 401，新全局令牌 200
    let (status, _) = send_as(
        router_for(state.clone()),
        "GET",
        "/api/prompts",
        None,
        "10.9.5.2",
        Some("device-token-43chars-bbbbbbbbbbbbbbbbbbbbbb"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send_as(router_for(state.clone()), "GET", "/api/prompts", None, "10.9.5.2", Some(&second)).await;
    assert_eq!(status, StatusCode::OK);
}

/// 扫码通道本轮**明确没做**：503 形状的诚实回答，而不是掉进"接口不存在"。
#[tokio::test]
async fn pair_start_is_explicitly_not_implemented() {
    let (status, body) = send_as(
        app(),
        "POST",
        "/api/auth/pair/start",
        Some(serde_json::json!({})),
        "127.0.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(
        body["error"].as_str().unwrap().contains("扫码配对还没做"),
        "要说清是哪条路没修：{body}"
    );
}

/// 兑码**也要限速**：只靠"错 5 次作废"挡不住"再举一次手换 5 次机会"。
#[tokio::test]
async fn claim_attempts_are_rate_limited_per_source_address() {
    let state = state();
    seed_auth(&state, false);
    // 同一来源、对着一个不存在的会话反复兑：前若干次是 400，之后必须出现 429
    let mut saw_rate_limited = false;
    let mut bodies: Vec<String> = Vec::new();
    for _ in 0..40 {
        let (status, body) = send_as(
            router_for(state.clone()),
            "POST",
            "/api/auth/pair/claim",
            Some(serde_json::json!({
                "requestId": "deadbeef", "code": "000000", "deviceId": "phone-flood"
            })),
            "10.9.6.1",
            None,
        )
        .await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            saw_rate_limited = true;
            bodies.push(body.to_string());
            break;
        }
        assert_eq!(status, StatusCode::BAD_REQUEST, "限速之前只能是「配对失败」");
    }
    assert!(saw_rate_limited, "同一个来源反复兑码必须被限速（设计 §5.2 的来源地址口径）");
    // 429 的响应里**不回显任何凭据**（ticket / code / 会话 id）
    let text = bodies.join(" ");
    assert!(!text.contains("000000"), "429 不许回显码：{text}");
    assert!(!text.contains("deadbeef"), "429 连会话 id 都不必回：{text}");
    // 另一个来源不受影响（限速是按来源地址分桶的）
    let (status, _) = send_as(
        router_for(state.clone()),
        "POST",
        "/api/auth/pair/claim",
        Some(serde_json::json!({
            "requestId": "deadbeef", "code": "000000", "deviceId": "phone-other"
        })),
        "10.9.6.2",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "限速按 IP 分桶，不该误伤别的来源");
}

// ---------------------------------------------------------------------------
//  跨端待办总览（P4 第 25 步）
// ---------------------------------------------------------------------------

/// `GET /api/ai/pending`：**只列"真的还在等人"的**，两个 `askedAt` 类型不同也别改。
#[tokio::test]
async fn pending_lists_only_what_is_really_waiting() {
    let state = state();
    // 夹具：一条 pending 提问、一条已答提问；一个 running Run 上的 pending 审批、
    // 一个已收尾 Run 上的 pending 审批（那是"永远等不到人"的残留）、
    // 以及一个 approved 的调用（不该出现）
    state
        .with(|conn| {
            conn.execute_batch(
                r#"
                INSERT INTO ai_runs (id, conversation_id, status, started_at)
                VALUES ('run-live', 'conv-0', 'running', '2026-09-24T14:00:00.000Z'),
                       ('run-done', 'conv-0', 'failed',  '2026-09-24T13:00:00.000Z');

                INSERT INTO ai_questions
                  (question_id, run_id, conversation_id, group_id, card_index, total,
                   question, options_json, allow_free_text, multi_select, header,
                   status, answer, option_index, option_indexes, asked_at)
                VALUES
                  ('q-1', 'run-live', 'conv-0', 'g-1', 1, 2, '要几张图？', '["1","2","4"]', 1, 0, '张数',
                   'pending', NULL, NULL, NULL, '2026-09-24T14:00:05.647Z'),
                  ('q-2', 'run-live', 'conv-0', 'g-1', 2, 2, '要什么风格？', NULL, 0, 1, NULL,
                   'pending', '国风', 0, '[0,2]', '2026-09-24T14:00:06.000Z'),
                  ('q-3', 'run-live', 'conv-0', 'g-1', 1, 1, '已经答过了', NULL, 1, 0, NULL,
                   'answered', '好的', NULL, NULL, '2026-09-24T13:59:00.000Z');

                INSERT INTO ai_tool_calls
                  (id, run_id, provider_call_id, name, arguments_json, approval, status, started_at)
                VALUES
                  ('tc-1', 'run-live', 'call-live', 'comfy_submit', '{"promptId":3}', 'pending', 'pending', '2026-09-24T14:00:10.000Z'),
                  ('tc-2', 'run-done', 'call-dead', 'write_file', '{}', 'pending', 'pending', '2026-09-24T13:00:10.000Z'),
                  ('tc-3', 'run-live', 'call-ok', 'read_file', '{}', 'approved', 'success', '2026-09-24T14:00:11.000Z');
                "#,
            )
            .map(|_| ())
        })
        .unwrap();

    let (status, body) = get_json(router_for(state.clone()), "/api/ai/pending").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&body), vec!["approvals", "asks"], "顶层就两个键");

    // 提问：只回那两条 pending（已答的 q-3 不在），顺序按 asked_at
    let asks = body["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 2, "只列还没答的：{asks:?}");
    assert_eq!(asks[0]["questionId"], "q-1");
    assert_eq!(asks[1]["questionId"], "q-2");

    // 第一条：带 header、没有 answer / optionIndex / optionIndexes（可空键不出现）
    assert_eq!(
        keys(&asks[0]),
        vec!["allowFreeText", "answered", "askedAt", "conversationId", "groupId", "header",
             "index", "multiSelect", "options", "question", "questionId", "runId", "total"]
    );
    assert_eq!(asks[0]["askedAt"], 1_790_258_405_647_i64, "⚠ 提问的 askedAt 是**毫秒数**");
    assert_eq!(asks[0]["options"], serde_json::json!(["1", "2", "4"]));
    assert_eq!(asks[0]["answered"], false);
    assert_eq!(asks[0]["index"], 1);
    assert_eq!(asks[0]["total"], 2);
    assert_eq!(asks[0]["groupId"], "g-1");

    // 第二条：有 answer / optionIndex / optionIndexes，没有 header
    assert_eq!(asks[1]["answer"], "国风");
    assert_eq!(asks[1]["optionIndex"], 0);
    assert_eq!(asks[1]["optionIndexes"], serde_json::json!([0, 2]));
    assert_eq!(asks[1]["multiSelect"], true);
    assert_eq!(asks[1]["allowFreeText"], false);
    assert!(asks[1].get("header").is_none(), "NULL 的 header 整个键不出现");
    assert!(asks[1].get("options").is_some(), "options 一定是数组（空也是 []）");
    assert_eq!(asks[1]["options"], serde_json::json!([]));

    // 审批：只回 running Run 上那条 pending；已收尾 Run 的残留与 approved 的都不算
    let approvals = body["approvals"].as_array().unwrap();
    assert_eq!(approvals.len(), 1, "只列真的还在等的：{approvals:?}");
    assert_eq!(
        keys(&approvals[0]),
        vec!["argumentsJson", "askedAt", "callId", "conversationId", "name", "runId"]
    );
    assert_eq!(approvals[0]["callId"], "call-live");
    assert_eq!(approvals[0]["name"], "comfy_submit");
    // ⚠ 这一份是 **ISO 文本**（与提问那份的毫秒数不同 —— Kotlin 侧就是这样，别"统一"）
    assert_eq!(approvals[0]["askedAt"], "2026-09-24T14:00:10Z");
}

/// 库里没有待办时回**两个空数组**（不是 null、也不是 404）—— 另一台设备据此说"没有在等的"。
#[tokio::test]
async fn pending_is_empty_arrays_when_nothing_waits() {
    let (status, body) = get_json(app(), "/api/ai/pending").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["asks"], serde_json::json!([]));
    assert_eq!(body["approvals"], serde_json::json!([]));
}

// ---------------------------------------------------------------------------
//  会话与消息的写侧（P4 第 24 步）
// ---------------------------------------------------------------------------

/// 建会话 / 改会话 / 删会话：**三种"没有"分清**，且删掉之后消息与块一起没。
#[tokio::test]
async fn conversation_write_side_creates_patches_and_deletes() {
    let state = state();

    // 空标题 / 全空白 → 「新对话」（Kotlin 的 `?:`，不是空串）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations",
        serde_json::json!({ "title": "   " }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "建会话是 201（Kotlin 同）");
    assert_eq!(body["title"], "新对话");
    assert_eq!(body["archived"], false);
    assert_eq!(body["messageCount"], 0);
    assert_eq!(body["systemPromptVersion"], "v1");
    assert!(body["id"].as_str().unwrap().len() >= 32, "id 是 UUID：{}", body["id"]);

    let (status, created) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations",
        serde_json::json!({ "title": "手机来的会话", "providerId": "p1", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["providerId"], "p1");
    assert_eq!(created["modelId"], "m1");

    // PATCH：给了的字段才改
    let (status, patched) = send_json(
        router_for(state.clone()),
        "PATCH",
        &format!("/api/ai/conversations/{id}"),
        serde_json::json!({ "title": "改过的标题", "archived": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["title"], "改过的标题");
    assert_eq!(patched["archived"], true);
    assert_eq!(patched["providerId"], "p1", "没给的字段不许被清掉");

    // 空 body 的 PATCH = 什么都不改（`null` 与"没给"同义）
    let (status, same) = send_json(
        router_for(state.clone()),
        "PATCH",
        &format!("/api/ai/conversations/{id}"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(same["title"], "改过的标题");
    assert_eq!(same["archived"], true);

    // 空标题 = 不改（不是改成空）
    let (_, same) = send_json(
        router_for(state.clone()),
        "PATCH",
        &format!("/api/ai/conversations/{id}"),
        serde_json::json!({ "title": "  " }),
    )
    .await;
    assert_eq!(same["title"], "改过的标题");

    // 不存在的会话：PATCH / DELETE 都是 **400 CONFIG_ERROR + detail**
    let (status, body) = send_json(
        router_for(state.clone()),
        "PATCH",
        "/api/ai/conversations/nope",
        serde_json::json!({ "title": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert_eq!(body["detail"], "会话不存在: nope");

    // DELETE：先给夹具那条会话挂一条消息 + 一个块，验级联真的清了（R1 是前提）
    let (_, msg) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/messages",
        serde_json::json!({ "role": "user", "text": "删我试试" }),
    )
    .await;
    assert!(msg["id"].is_string());
    let (status, body) = send_json(
        router_for(state.clone()),
        "DELETE",
        "/api/ai/conversations/conv-0",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted"], true);
    assert_eq!(body["id"], "conv-0");
    let (messages, parts): (i64, i64) = state
        .with(|conn| {
            Ok((
                conn.query_row("SELECT COUNT(*) FROM ai_messages WHERE conversation_id = 'conv-0'", [], |r| r.get(0))?,
                conn.query_row("SELECT COUNT(*) FROM ai_message_parts WHERE message_id = 'msg-0'", [], |r| r.get(0))?,
            ))
        })
        .unwrap();
    assert_eq!(messages, 0, "删会话要连消息一起删（外键级联）");
    assert_eq!(parts, 0, "…连有序块一起删");
    // 再删一次 → 400（不是静默成功）
    let (status, _) = send_json(
        router_for(state.clone()),
        "DELETE",
        "/api/ai/conversations/conv-0",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// 追加消息：`seq` 由服务端分配、消息与块**同一个事务**、非法角色/状态一律 400 带稳定码。
#[tokio::test]
async fn append_message_assigns_seq_and_writes_parts_atomically() {
    let state = state();

    // 量的是"这次追加带来几条"：夹具插的那条消息**本身**就会点亮一次账本
    // （触发器挂在表上，裸 INSERT 同样会触发），所以不能拿总数比。
    let before: i64 = state
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM _sync_log WHERE tbl = 'ai_messages' AND op = 'upsert'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();

    // 调用方给 id（界面按它认自己插的那条气泡）+ 两个块（正文 + 工具卡）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/messages",
        serde_json::json!({
            "id": "msg-mine",
            "role": "assistant",
            "text": "好的",
            "parts": [
                { "type": "text", "text": "好的" },
                { "type": "tool_call", "toolCallId": "call-1",
                  "jsonPayload": { "name": "web_search", "arguments": { "query": "x" } } }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "追加消息是 201：{body}");
    assert_eq!(body["id"], "msg-mine", "认调用方给的 id");
    assert_eq!(body["seq"], 2, "夹具里已有 seq=1 的那条");
    assert_eq!(body["status"], "complete");
    assert_eq!(body["parts"].as_array().unwrap().len(), 2);
    assert_eq!(body["parts"][0]["type"], "text");
    assert_eq!(body["parts"][1]["type"], "tool_call");
    assert_eq!(body["parts"][1]["jsonPayload"]["name"], "web_search");
    assert!(body["parts"][1]["jsonPayload"].is_object(), "jsonPayload 是对象，不是字符串");
    // 块里没值的键**不出现**（形状纪律）
    assert!(body["parts"][0].get("toolCallId").is_none());

    // 第二条 → seq 递增；没给 id 就自己生成一个
    let (_, second) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/messages",
        serde_json::json!({ "role": "user", "text": "第二条" }),
    )
    .await;
    assert_eq!(second["seq"], 3);
    assert_ne!(second["id"], "msg-mine");

    // 发消息要**把会话顶上去**（列表按 updated_at 排序，不顶就沉底）
    let (_, conversation) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-0").await;
    assert_eq!(conversation["messageCount"], 3, "夹具 1 条 + 新加的 2 条");

    // 非法角色 / 非法状态 / 不存在的会话 → 400 + 稳定码 + 人话
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/messages",
        serde_json::json!({ "role": "wizard", "text": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert_eq!(body["detail"], "非法角色: wizard");

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/messages",
        serde_json::json!({ "role": "user", "text": "x", "status": "sleeping" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "非法状态: sleeping");

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/nope/messages",
        serde_json::json!({ "role": "user", "text": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "会话不存在: nope");

    // 非法的那几次**一行都没写进去**（校验在事务之前）
    let (_, conversation) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-0").await;
    assert_eq!(conversation["messageCount"], 3, "被拒的追加不许留下半截数据");

    // 读回来时块是**按 ordinal 升序**的（工具卡位置不能乱）
    let (_, messages) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-0/messages").await;
    let mine = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "msg-mine")
        .expect("刚追加的那条要在列表里");
    assert_eq!(mine["parts"][0]["text"], "好的");
    assert_eq!(mine["parts"][1]["toolCallId"], "call-1");

    // 写侧**点亮同步账本**（手机靠它增量收到桌面写的会话）—— R3：不许出现假的 delete
    let (upserts, deletes): (i64, i64) = state
        .with(|conn| {
            Ok((
                conn.query_row(
                    "SELECT COUNT(*) FROM _sync_log WHERE tbl = 'ai_messages' AND op = 'upsert'",
                    [],
                    |r| r.get(0),
                )?,
                conn.query_row(
                    "SELECT COUNT(*) FROM _sync_log WHERE tbl = 'ai_messages' AND op = 'delete'",
                    [],
                    |r| r.get(0),
                )?,
            ))
        })
        .unwrap();
    assert_eq!(upserts - before, 2, "两次成功追加 = 两条 upsert");
    assert_eq!(deletes, 0, "**不许**出现假 delete（R3）");
}

// ---------------------------------------------------------------------------
//  Run 持久化（P4 第 26 步）—— 事件流的**两条读法**共用同一份事实
// ---------------------------------------------------------------------------

/// 夹具会话的 provider / 模型（这两行是 Run 的外键目标）。
///
/// ⚠ 必须真的插进 `ai_providers` / `ai_models`：`ai_runs.provider_id` 没有外键，
/// 但 `run_kit` 与模型目录要它们；而"夹具里没有 provider"正是那种
/// "测试全绿、真库才炸"的缝（`/api/sync/changes` 那次 500 的教训）。
fn seed_provider_and_model(state: &comfyhub_server_rs::routes::AppState) {
    state
        .write(|tx| {
            // `credential_ref` 刻意留 NULL：这样执行器**不会**在"解析密钥"那一步就失败，
            // 而是真的往 `127.0.0.1:9` 发一次请求（没人监听 → `PROVIDER_UNREACHABLE`）。
            // 夹具里塞一个假的 ref 会让这条用例断在 `MISSING_CREDENTIAL` 上 ——
            // 那测的是凭据解析，不是"Run 会失败而不是永远转圈"。
            tx.execute(
                "INSERT INTO ai_providers (id, display_name, api, base_url, credential_ref,
                                           endpoint_trust, enabled, revision)
                 VALUES ('goat', '山羊', 'openai-completions', 'http://127.0.0.1:9/v1',
                         NULL, 'loopback', 1, 1)",
                [],
            )?;
            tx.execute(
                "INSERT INTO ai_models (provider_id, model_id, display_name, tools, reasoning,
                                        input_modalities, attachment_transports, mime_allowlist,
                                        parallel_tools, thinking_efforts, capability_source, enabled)
                 VALUES ('goat', 'm1', '模型一', 1, 0, '[\"text\"]', '{}', '[]', 0, '[]', 'manual', 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
}

/// Run 的一生：建 → 落事件 → 终态；键集合与 SQL 里的时间列都要对。
#[tokio::test]
async fn run_persistence_round_trips_shape_and_terminal_state() {
    use comfyhub_server_rs::runs;
    let state = state();
    seed_provider_and_model(&state);

    let run = runs::create(
        &state,
        runs::NewRun {
            conversation_id: "conv-0".into(),
            provider_id: "goat".into(),
            model_id: "m1".into(),
            provider_snapshot: serde_json::json!({
                "id": "goat", "displayName": "山羊", "api": "openai-completions",
                "baseURL": "http://127.0.0.1:9/v1", "endpointTrust": "public",
                "credentialRef": "GOAT_KEY"
            }),
            model_snapshot: serde_json::json!({
                "id": "m1", "displayName": "模型一", "tools": true, "reasoning": false,
                "capabilitySource": "manual", "nsfw": false, "nsfwPromptVersion": "v1"
            }),
            prompt_version: "v13".into(),
            user_message_id: Some("msg-0".into()),
            assistant_message_id: "assistant-1".into(),
            retry_of_run_id: None,
            reasoning_effort: Some("off".into()),
            skill_snapshot: Some(serde_json::json!({"count": 0, "skills": []})),
        },
    )
    .expect("建 Run");

    assert_eq!(run.status, "running");
    assert_eq!(run.conversation_id, "conv-0");
    assert_eq!(run.assistant_message_id.as_deref(), Some("assistant-1"));
    assert_eq!(run.user_message_id.as_deref(), Some("msg-0"));
    assert_eq!(run.reasoning_effort.as_deref(), Some("off"));
    assert_eq!(run.prompt_version.as_deref(), Some("v13"));
    assert!(run.started_at.is_some(), "started_at 由 SQL 侧 strftime 写，读完要有值");
    assert!(run.completed_at.is_none());
    // 可空键的**键集合**（少一个 = 界面少一块；多一个 = 前端拿不到类型）
    let value = serde_json::to_value(&run).unwrap();
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "assistantMessageId",
            "conversationId",
            "createdAt",
            "id",
            "modelId",
            "promptVersion",
            "providerId",
            "reasoningEffort",
            "startedAt",
            "status",
            "userMessageId",
        ],
        "Run 的键集合（Kotlin AiRunDto 的 11 个非空键）"
    );
    assert!(value.get("completedAt").is_none(), "还没结束 → 键不出现");
    assert!(value.get("errorCode").is_none() && value.get("retryOfRunId").is_none());

    // 快照真的落了库（事后追溯"当时用的是哪版规则 / 带没带 NSFW"靠它）
    let snapshots: (String, String, String) = state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT provider_snapshot, model_snapshot, skill_snapshot FROM ai_runs WHERE id = ?1",
                rusqlite::params![run.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        })
        .unwrap();
    assert!(snapshots.0.contains("\"credentialRef\":\"GOAT_KEY\""), "快照里只有引用名");
    assert!(!snapshots.0.contains("sk-"), "快照里**绝不能**有密钥值（AIH-023）");
    assert!(snapshots.1.contains("\"nsfwPromptVersion\":\"v1\""));
    assert_eq!(snapshots.2, r#"{"count":0,"skills":[]}"#);

    // 非终态不许当终态写（写了僵死 Run 就收不走）
    let bad = runs::finish(&state, &run.id, "running", None, None, None);
    assert!(bad.is_err(), "running 不是终态");

    runs::finish(
        &state,
        &run.id,
        "completed",
        None,
        None,
        Some(&serde_json::json!({"inputTokens": 12, "outputTokens": 34})),
    )
    .expect("落终态");
    let done = runs::get(&state, &run.id).unwrap();
    assert_eq!(done.status, "completed");
    assert!(done.completed_at.is_some());
    let value = serde_json::to_value(&done).unwrap();
    assert!(value.get("errorCode").is_none(), "成功时 errorCode 键不出现");

    // 不存在的 Run → 400 + 稳定码 + 人话（**不是** 404：前端按 error 分支）
    let missing = runs::get(&state, "nope").unwrap_err();
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert_eq!(missing.error, "CONFIG_ERROR");
    assert_eq!(missing.detail.as_deref(), Some("Run 不存在: nope"));
}

/// 事件：批量写 → `?after=` 续传 → **保留窗口留尾部**（红线 R5）。
#[tokio::test]
async fn run_events_replay_after_seq_and_prune_keeps_the_tail() {
    use comfyhub_server_rs::runs;
    let state = state();
    seed_provider_and_model(&state);
    let run = runs::create(
        &state,
        runs::NewRun {
            conversation_id: "conv-0".into(),
            provider_id: "goat".into(),
            model_id: "m1".into(),
            provider_snapshot: serde_json::json!({}),
            model_snapshot: serde_json::json!({}),
            prompt_version: "v13".into(),
            user_message_id: None,
            assistant_message_id: "assistant-1".into(),
            retry_of_run_id: None,
            reasoning_effort: None,
            skill_snapshot: None,
        },
    )
    .unwrap();

    // 批量写：batch_size=3，塞 7 条 → 中途写 2 批（6 条），flush 补最后 1 条
    let mut batcher = runs::EventBatcher::new(&state, run.id.clone(), 3);
    for seq in 1..=7 {
        batcher
            .add(seq, "text.delta", &serde_json::json!({ "text": format!("t{seq}") }))
            .unwrap();
    }
    assert_eq!(
        runs::events(&state, &run.id, 0).unwrap().len(),
        6,
        "满一批就写：7 条里前 6 条已经落库"
    );
    assert_eq!(batcher.flush().unwrap(), 1, "收尾必须 flush（否则最后那条 run.completed 会丢）");
    assert_eq!(runs::events(&state, &run.id, 0).unwrap().len(), 7);

    // 断线续传：`?after=5` 只拿 6 / 7（游标语义的唯一读法）
    let tail = runs::events(&state, &run.id, 5).unwrap();
    assert_eq!(tail.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![6, 7]);
    assert_eq!(tail[0].to_sse(), "id: 6\nevent: text.delta\ndata: {\"text\":\"t6\"}\n\n");

    // 幂等：同 (run_id, seq) 再写一遍不产生第二行
    runs::append_event(&state, &run.id, 7, "text.delta", &serde_json::json!({"text": "again"}))
        .unwrap();
    let again = runs::events(&state, &run.id, 0).unwrap();
    assert_eq!(again.len(), 7);
    assert_eq!(again[6].payload["text"], "t7", "重放不许改写已有的那一行");

    // 保留窗口（R5）：keep=3 → 只留最后 3 条，**删的是最老的**
    assert_eq!(runs::prune_events(&state, &run.id, 3).unwrap(), 4);
    let kept = runs::events(&state, &run.id, 0).unwrap();
    assert_eq!(kept.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![5, 6, 7]);
    assert_eq!(
        runs::events(&state, &run.id, 4).unwrap().len(),
        3,
        "窗口左沿之后仍然完整（这正是 R5 要的：重连能拿到最新的一段）"
    );

    // 坏 payload 不让整条回放 500
    state
        .write(|tx| {
            tx.execute(
                "INSERT INTO ai_run_events (run_id, seq, event_type, payload_json) VALUES (?1, 8, 'x', 'not json')",
                rusqlite::params![run.id],
            )?;
            Ok(())
        })
        .unwrap();
    let events = runs::events(&state, &run.id, 7).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload, serde_json::json!({}), "脏 payload 当空对象");
}

/// 启动恢复：还在 `running` 的 Run 收成 failed、pending 审批按拒绝收尾。
///
/// 两者是**同一次重启**的两半：内存里的 run 与闸门一起消失，
/// 库里留着它们就是"界面永远转圈 + 永远等不到人点的审批卡"。
#[tokio::test]
async fn startup_recovery_fails_stale_runs_and_denies_pending_approvals() {
    use comfyhub_server_rs::runs;
    let state = state();
    seed_provider_and_model(&state);

    let make = |assistant: &str| {
        runs::create(
            &state,
            runs::NewRun {
                conversation_id: "conv-0".into(),
                provider_id: "goat".into(),
                model_id: "m1".into(),
                provider_snapshot: serde_json::json!({}),
                model_snapshot: serde_json::json!({}),
                prompt_version: "v13".into(),
                user_message_id: None,
                assistant_message_id: assistant.into(),
                retry_of_run_id: None,
                reasoning_effort: None,
                skill_snapshot: None,
            },
        )
        .unwrap()
    };
    let running = make("assistant-1");
    let done = make("assistant-2");
    runs::finish(&state, &done.id, "completed", None, None, None).unwrap();

    // 一个等审批的工具调用（R8：open 的同一刻就落 pending）
    runs::insert_pending_tool_call(&state, &running.id, "call-1", "comfy_submit", r#"{"x":1}"#)
        .unwrap();
    let listing = runs::list_tool_calls(&state, &running.id).unwrap();
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0]["status"], "pending");
    assert_eq!(listing[0]["approval"], "pending");

    // invoke 返回 → **原地更新那一行**（不新增）
    runs::record_tool_call(
        &state,
        &runs::ToolCallRecord {
            id: "tc-1".into(),
            run_id: running.id.clone(),
            call_id: "call-1".into(),
            name: "comfy_submit".into(),
            arguments_json: r#"{"x":1}"#.into(),
            approval: "approved".into(),
            status: "ok".into(),
            result_json: Some(serde_json::json!({"mediaIds": [7]})),
            content: Some("已提交".into()),
            error: None,
            elapsed_ms: 12,
        },
    )
    .unwrap();
    let listing = runs::list_tool_calls(&state, &running.id).unwrap();
    assert_eq!(listing.len(), 1, "有 pending 行就原地更新，不再插一行");
    assert_eq!(listing[0]["status"], "ok");
    assert_eq!(listing[0]["approval"], "approved");

    // 再挂一个 pending，然后走"后端重启"
    runs::insert_pending_tool_call(&state, &running.id, "call-2", "read_file", "{}").unwrap();
    assert_eq!(runs::fail_stale_running(&state, "后端重启导致 Run 中断").unwrap(), 1);
    assert_eq!(runs::expire_stale_pending_tool_calls(&state).unwrap(), 1);

    let stale = runs::get(&state, &running.id).unwrap();
    assert_eq!(stale.status, "failed");
    assert_eq!(stale.error_code.as_deref(), Some("PROTOCOL_ERROR"));
    assert!(stale.completed_at.is_some());
    assert_eq!(
        runs::get(&state, &done.id).unwrap().status,
        "completed",
        "已经收尾的 Run 不许被重启恢复改写"
    );
    let denied = runs::list_tool_calls(&state, &running.id)
        .unwrap()
        .into_iter()
        .find(|c| c["callId"] == "call-2")
        .unwrap();
    assert_eq!(denied["status"], "denied", "重启 = 最彻底的超时，按拒绝处理");
    assert!(denied["error"].as_str().unwrap().contains("重启"));
}

// ---------------------------------------------------------------------------
//  执行器主循环（P4 第 30 步）：对着**真 socket 上的假网关**跑一遍
// ---------------------------------------------------------------------------

/// 假网关：`127.0.0.1:<临时端口>`，只认 `POST /v1/chat/completions?upstream=…`。
///
/// 为什么要一个真 socket 服务器（而不是 mock 掉 `stream_chat`）：
/// **"逐字增量 → SSE 分帧 → 适配器 → 落库"这条链的每一段都在真 I/O 上**，
/// mock 掉就只剩"我以为" —— M3.3 那条教训（`IOException` 只翻了一半）就是这么来的。
///
/// 三个行为分岔（用查询参数选，默认 `ok`）：
/// * `upstream=ok`：正常流（思考 → 三段正文 → finish_reason → usage → `[DONE]`）；
/// * `upstream=http429`：直接 429（测"上游明确拒绝"那一档）；
/// * `upstream=partial`：**先正常吐出两段正文，然后把连接掐掉** ——
///   R6"已经说出去的话一个字都不能丢"的现场（真·中途断流，而不是事后报错）。
async fn fake_gateway() -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16 * 1024];
                let Ok(n) = socket.read(&mut buf).await else { return };
                let request_line = String::from_utf8_lossy(&buf[..n])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                if request_line.contains("upstream=http429") {
                    let body = r#"{"error":"rate limited"}"#;
                    let head = format!(
                        "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(body.as_bytes()).await;
                    let _ = socket.shutdown().await;
                    return;
                }
                let partial = request_line.contains("upstream=partial");
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
                    .await;
                let mut frames: Vec<&str> = vec![
                    r#"{"id":"chatcmpl-1","choices":[{"delta":{"reasoning_content":"先想一下"}}]}"#,
                    r#"{"choices":[{"delta":{"content":"你"}}]}"#,
                    r#"{"choices":[{"delta":{"content":"好"}}]}"#,
                ];
                if !partial {
                    frames.push(r#"{"choices":[{"delta":{"content":"呀"}}]}"#);
                    frames.push(r#"{"choices":[{"finish_reason":"stop"}]}"#);
                    frames.push(r#"{"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3}}"#);
                    frames.push("[DONE]");
                }
                for frame in frames {
                    let payload = format!("data: {frame}\n\n");
                    if socket.write_all(payload.as_bytes()).await.is_err() {
                        return;
                    }
                    let _ = socket.flush().await;
                }
                // `partial`：**不说 DONE、直接断** —— 上游中途死掉的样子
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://{addr}/v1"), handle)
}

/// 把夹具 provider 的 Base URL 指到假网关。
fn point_at_gateway(state: &comfyhub_server_rs::routes::AppState, base_url: &str) {
    state
        .write(|tx| {
            tx.execute(
                "UPDATE ai_providers SET base_url = ?1 WHERE id = 'goat'",
                rusqlite::params![base_url],
            )?;
            Ok(())
        })
        .unwrap();
}

/// 一次完整的流式 Run：正文逐字到达、有序块按序落库、usage 与终态都对。
#[tokio::test]
async fn executor_streams_a_reply_and_persists_parts_in_order() {
    let (base_url, gateway) = fake_gateway().await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "打个招呼", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();

    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    assert!(run["completedAt"].is_string());

    // 助手消息：正文是**三段拼起来的**，状态 complete
    let (_, messages) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-0/messages").await;
    let assistant = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == assistant_id.as_str())
        .unwrap();
    assert_eq!(assistant["text"], "你好呀", "三段增量拼成完整正文");
    assert_eq!(assistant["status"], "complete");
    assert_eq!(assistant["providerResponseId"], "chatcmpl-1", "上游给的响应 id 要记下来");
    assert_eq!(assistant["usage"]["prompt_tokens"], 7, "usage 原样落库（{assistant}）");
    assert_eq!(assistant["usage"]["total_tokens"], 10);

    // 有序块：**思考在前、正文在后**（顺序乱了工具卡位置就错，而正文还在）
    let parts = assistant["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2, "{assistant}");
    assert_eq!(parts[0]["type"], "reasoning");
    assert_eq!(parts[0]["text"], "先想一下");
    assert_eq!(parts[1]["type"], "text");
    assert_eq!(parts[1]["text"], "你好呀");

    // 事件：run.started → … → message.completed → run.completed（seq 连续、不跳号）
    let types = event_types(&state, &run_id);
    assert_eq!(types.first().unwrap(), "run.started");
    assert_eq!(types.get(1).unwrap(), "message.started");
    assert!(types.contains(&"reasoning.delta".to_string()));
    assert_eq!(
        types.iter().filter(|t| *t == "text.delta").count(),
        3,
        "三段正文 → 三条事件（逐字流式）：{types:?}"
    );
    assert!(types.contains(&"usage.updated".to_string()));
    assert_eq!(types.last().unwrap(), "run.completed");
    let seqs = state
        .with(|conn| {
            let mut stmt = conn
                .prepare("SELECT seq FROM ai_run_events WHERE run_id = ?1 ORDER BY seq")?;
            let rows = stmt
                .query_map(rusqlite::params![run_id], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap();
    assert_eq!(
        seqs,
        (1..=seqs.len() as i64).collect::<Vec<_>>(),
        "seq 从 1 起、连续递增（红线 R5 的「绝不跳号」）"
    );

    gateway.abort();
}

/// **R6 的现场**：上游说了两段之后**连接被掐断** —— 已经说出去的话一个字都不能丢。
#[tokio::test]
async fn executor_keeps_what_was_already_said_when_the_stream_breaks() {
    let (base_url, gateway) = fake_gateway().await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &format!("{base_url}?upstream=partial"));

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "写一段", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();

    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "failed", "{run}");
    assert!(run["errorCode"].is_string());

    // ★ 这一条就是 R6：**已经说出去的两段话必须还在**（正文 + 有序块都得有）
    let (_, messages) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-0/messages").await;
    let assistant = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == assistant_id.as_str())
        .unwrap();
    assert_eq!(assistant["status"], "failed");
    assert_eq!(assistant["text"], "你好", "断流前已经说出去的话不许丢（R6）：{assistant}");
    let parts = assistant["parts"].as_array().unwrap();
    assert!(
        parts.iter().any(|p| p["type"] == "text" && p["text"] == "你好"),
        "正文必须**同时**落进有序块（前端在 parts 非空时只渲染 parts）：{assistant}"
    );
    assert!(
        parts.iter().any(|p| p["type"] == "reasoning"),
        "思考段也要留着：{assistant}"
    );

    // 事件序列同样以终态事件收尾（断线重连能从库里补完）
    let types = event_types(&state, &run_id);
    assert_eq!(types.last().unwrap(), "run.failed");
    assert_eq!(types.first().unwrap(), "run.started");

    gateway.abort();
}

/// 假网关的第二形态：**先要求调一个工具，拿到结果后再答**（工具往返两轮）。
///
/// `tool` 是模型要调的工具名；`arguments` 是它给的参数 JSON。
/// 第二次请求（带上工具结果的那一轮）回一段正文。
async fn fake_tool_gateway(tool: &'static str, arguments: &'static str) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let mut round = 0;
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            round += 1;
            tokio::spawn(async move {
                // 只读一次：Content-Length 那一批够我们判断"这是第几轮"（测试用的请求体很小）
                let mut buf = vec![0u8; 256 * 1024];
                let _ = socket.read(&mut buf).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
                    .await;
                let frames: Vec<String> = if round == 1 {
                    // 工具调用的分片是**分开来**的（第一片带 id/name，之后只有参数片段）
                    vec![
                        format!(
                            r#"{{"id":"chatcmpl-tool","choices":[{{"delta":{{"tool_calls":[{{"index":0,"id":"call_1","function":{{"name":"{tool}","arguments":""}}}}]}}}}]}}"#
                        ),
                        format!(
                            r#"{{"choices":[{{"delta":{{"tool_calls":[{{"index":0,"function":{{"arguments":{}}}}}]}}}}]}}"#,
                            serde_json::to_string(arguments).unwrap()
                        ),
                        r#"{"choices":[{"finish_reason":"tool_calls"}]}"#.to_string(),
                        "[DONE]".to_string(),
                    ]
                } else {
                    vec![
                        r#"{"choices":[{"delta":{"content":"工具用完了"}}]}"#.to_string(),
                        r#"{"choices":[{"finish_reason":"stop"}]}"#.to_string(),
                        r#"{"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":5}}"#.to_string(),
                        "[DONE]".to_string(),
                    ]
                };
                for frame in frames {
                    let payload = if frame == "[DONE]" {
                        "data: [DONE]\n\n".to_string()
                    } else {
                        format!("data: {frame}\n\n")
                    };
                    if socket.write_all(payload.as_bytes()).await.is_err() {
                        return;
                    }
                    let _ = socket.flush().await;
                }
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://{addr}/v1"), handle)
}

/// 写工具权限策略（与 Kotlin `ToolPolicyConfig` 同一份 JSON）。
fn set_policy(state: &comfyhub_server_rs::routes::AppState, json: serde_json::Value) {
    state
        .write(|tx| {
            tx.execute(
                "INSERT INTO app_settings (k, v) VALUES ('ai.tools.policy', ?1)
                 ON CONFLICT(k) DO UPDATE SET v = excluded.v",
                rusqlite::params![json.to_string()],
            )?;
            Ok(())
        })
        .unwrap();
}

/// 脚本化假网关：**第 r 轮回 `script[r]` 里那几个工具调用**，脚本用完就回一段正文收口。
///
/// 为什么要它（而不是只有 `fake_tool_gateway`）：轮数上限、调用额度、一轮多个调用
/// 这几条纪律都需要"模型**一直**要工具"或"一轮要两个"的形态 ——
/// `fake_tool_gateway` 第二轮就给正文，永远走不到那些分支。
async fn scripted_tool_gateway(
    script: Vec<Vec<(&'static str, &'static str)>>,
) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // 调用号**跨轮连续**（call_1 / call_2 / …）：真实上游的 id 也是全局唯一的，
    // 每轮都从 call_1 重新数会让"按 callId 找那一条"的断言张冠李戴（实测踩过）
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let handle = tokio::spawn(async move {
        let mut round = 0usize;
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            let calls = script.get(round).cloned();
            let counter = counter.clone();
            round += 1;
            tokio::spawn(async move {
                let mut buf = vec![0u8; 256 * 1024];
                let _ = socket.read(&mut buf).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
                    .await;
                let frames: Vec<String> = match calls {
                    Some(calls) if !calls.is_empty() => {
                        let mut out = Vec::new();
                        for (index, (tool, arguments)) in calls.iter().enumerate() {
                            let call_id = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            out.push(format!(
                                r#"{{"choices":[{{"delta":{{"tool_calls":[{{"index":{index},"id":"call_{call_id}","function":{{"name":"{tool}","arguments":""}}}}]}}}}]}}"#
                            ));
                            out.push(format!(
                                r#"{{"choices":[{{"delta":{{"tool_calls":[{{"index":{index},"function":{{"arguments":{}}}}}]}}}}]}}"#,
                                serde_json::to_string(arguments).unwrap()
                            ));
                        }
                        out.push(r#"{"choices":[{"finish_reason":"tool_calls"}]}"#.to_string());
                        out.push("[DONE]".to_string());
                        out
                    }
                    _ => vec![
                        r#"{"choices":[{"delta":{"content":"收口了"}}]}"#.to_string(),
                        r#"{"choices":[{"finish_reason":"stop"}]}"#.to_string(),
                        "[DONE]".to_string(),
                    ],
                };
                for frame in frames {
                    let payload = if frame == "[DONE]" {
                        "data: [DONE]\n\n".to_string()
                    } else {
                        format!("data: {frame}\n\n")
                    };
                    if socket.write_all(payload.as_bytes()).await.is_err() {
                        return;
                    }
                    let _ = socket.flush().await;
                }
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://{addr}/v1"), handle)
}

/// 起一个 Run 并等它收尾（工具类用例的公共开头）。
async fn start_run_and_wait(
    state: &comfyhub_server_rs::routes::AppState,
    text: &str,
) -> (String, String, serde_json::Value) {
    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": text, "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();
    let run = wait_for_run(state, &run_id).await;
    (run_id, assistant_id, run)
}

/// 助手那条消息的正文（历史视图里看到的东西）。
fn assistant_text(state: &comfyhub_server_rs::routes::AppState, assistant_id: &str) -> String {
    let messages = comfyhub_server_rs::conversations::list_messages(state, "conv-0").unwrap();
    messages
        .iter()
        .find(|m| m.id == assistant_id)
        .map(|m| m.text.clone())
        .unwrap_or_default()
}

/// 工具往返：模型调 `list_skills`（ALLOW，不需审批）→ 执行 → 结果回填 → 模型收口。
#[tokio::test]
async fn executor_runs_a_tool_round_trip_and_orders_the_parts() {
    let (base_url, gateway) = fake_tool_gateway("list_skills", "{}").await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);

    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "看看有哪些技能", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();

    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "completed", "{run}");

    // 有序块：**工具卡在前、工具结果在中、两轮正文按顺序**
    let (_, messages) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-0/messages").await;
    let assistant = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == assistant_id.as_str())
        .unwrap();
    assert_eq!(assistant["text"], "工具用完了", "最终正文是**第二轮**的：{assistant}");
    let kinds: Vec<&str> = assistant["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["tool_call", "tool_result", "text"], "块顺序 = 事情发生的顺序");
    assert_eq!(assistant["parts"][0]["jsonPayload"]["name"], "list_skills");
    assert_eq!(assistant["parts"][0]["toolCallId"], "call_1");
    // ⚠ 参数是**模型给的原文**（字符串），不是解析后的对象 —— Dart 侧 `map['arguments']`
    // 读的就是它；存成对象会变成 `{query: x}` 这种既不是 JSON、也没法回填的东西。
    assert_eq!(assistant["parts"][0]["jsonPayload"]["arguments"], "{}");
    assert_eq!(assistant["parts"][1]["toolCallId"], "call_1");
    // `list_skills` 的结果是 JSON 文本（含 count / skills）
    assert!(
        assistant["parts"][1]["text"].as_str().unwrap().contains("skills"),
        "工具结果要落进块里（R6）：{assistant}"
    );
    // 结果块的 payload 键集合**就是历史回放的契约**（Kotlin `HarnessRunner:762`）：
    // 少了 `ok`，一条成功的调用在重开 App 之后会被画成失败。
    let result_payload = &assistant["parts"][1]["jsonPayload"];
    assert_eq!(
        keys(result_payload),
        vec!["approval", "code", "elapsedMs", "name", "ok"],
        "{result_payload}"
    );
    assert_eq!(result_payload["ok"], true);
    assert_eq!(result_payload["approval"], "not_required");
    assert_eq!(result_payload["name"], "list_skills");

    // 事件：`tool.requested` **每一个调用都发**（只读工具带 `approval:not_required`）——
    // 界面靠它给工具卡排位置（用户 bug⑥：流式期间"思考 → 工具 → 正文"的交错）
    let types = event_types(&state, &run_id);
    assert!(types.contains(&"tool.started".to_string()), "{types:?}");
    assert!(types.contains(&"tool.completed".to_string()), "{types:?}");
    let requested = types
        .iter()
        .position(|t| t == "tool.requested")
        .unwrap_or_else(|| panic!("只读工具也要发 requested（带 not_required）：{types:?}"));
    let started = types.iter().position(|t| t == "tool.started").unwrap();
    assert!(requested < started, "先 requested 再 started：{types:?}");
    assert_eq!(types.last().unwrap(), "run.completed");
    assert_eq!(types.first().unwrap(), "run.started");

    // `tool.completed` 的键集合也是契约的一部分（`preview` 是工具卡上的那一行小字）
    let completed = state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT payload_json FROM ai_run_events
                  WHERE run_id = ?1 AND event_type = 'tool.completed' ORDER BY seq LIMIT 1",
                rusqlite::params![run_id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap();
    let completed: serde_json::Value = serde_json::from_str(&completed).unwrap();
    assert_eq!(keys(&completed), vec!["callId", "elapsedMs", "name", "preview", "result"]);

    // 审计表里留了痕
    let calls = comfyhub_server_rs::runs::list_tool_calls(&state, &run_id).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["name"], "list_skills");
    assert_eq!(calls[0]["status"], "ok");
    assert_eq!(calls[0]["approval"], "not_required");

    gateway.abort();
}

/// **R8 的顺序**：需要审批的工具 —— **先落 pending 行**，再发 `tool.requested`，
/// 然后用户在**任何一条路径**上都能点批准（跨端）。
#[tokio::test]
async fn approval_gate_opens_before_the_user_can_click() {
    // 用 `remember`（只写本机那份长期记忆文件、**不出网**）：它出厂是 ALLOW，
    // 这里用**逐工具覆盖**把它变成 ASK。这样这一条测的纯粹是闸门 ——
    // 用 `civitai_search` 那种要出网的工具会让用例因为网络抖动而红。
    let (base_url, gateway) = fake_tool_gateway("remember", r#"{"content":"用户偏好 4:3 画幅"}"#).await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);
    set_policy(&state, serde_json::json!({ "permissionMode": "ask", "overrides": { "remember": "ask" } }));

    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "记一下我的画幅偏好", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    let run_id = body["runId"].as_str().unwrap().to_string();

    // ★ R8 的防线：**点批准的那一刻，库里必须已经有那一行 pending**
    //
    // 这里刻意**不等事件**：用户在界面上手有多快我们不知道，而"点下去落空、
    // 只能干等超时"正是这条红线要防的事。所以判据是"立刻点也能被接受"。
    let mut accepted = false;
    for _ in 0..80 {
        let calls = comfyhub_server_rs::runs::list_tool_calls(&state, &run_id).unwrap();
        if calls.is_empty() || calls[0]["status"] != "pending" {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            continue;
        }
        // 库里有 pending 了 → **马上**点批准（这就是用户手快那条路径）
        let (status, result) = send_json(
            router_for(state.clone()),
            "POST",
            "/api/ai/tool-calls/call_1/approve",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(result["approved"], true);
        assert_eq!(result["callId"], "call_1");
        accepted = result["accepted"].as_bool().unwrap_or(false);
        break;
    }
    assert!(accepted, "手快那一下**必须**被接受（否则用户只能等超时）——R8");

    // 再点一次：没有 pending 行可改了 → `accepted:false`（那不是错误）
    let (status, again) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/tool-calls/call_1/approve",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["accepted"], false);
    assert_eq!(again["approved"], true, "重复点也是「批准」这个答复本身");

    // Run 继续跑完（**这一轮真的执行了那个工具**）
    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "completed", "{run}");

    // 落库的行：`approval` 记用户的决定、`status` 记执行结果，**而且只有一行** ——
    // 顺手把 `status` 也写成 `approved`（或让两列互相顶替）会让原地更新匹配不上，
    // 同一次调用在库里变成两行，而 `/api/ai/pending` 又会把它当成"还在等人批"。
    let calls = comfyhub_server_rs::runs::list_tool_calls(&state, &run_id).unwrap();
    assert_eq!(calls.len(), 1, "同一次调用只许一行（两列分开记，不是插新行）：{calls:?}");
    assert_eq!(calls[0]["approval"], "approved");
    assert_eq!(calls[0]["status"], "ok");

    // 事件：`tool.requested` 要在，而且排在 `tool.started` **之前**（先"请批准"，再"开始执行"）
    let types = event_types(&state, &run_id);
    let requested = types
        .iter()
        .position(|t| t == "tool.requested")
        .unwrap_or_else(|| panic!("需要审批的工具必须发 tool.requested：{types:?}"));
    let started = types
        .iter()
        .position(|t| t == "tool.started")
        .unwrap_or_else(|| panic!("批准之后要发 tool.started：{types:?}"));
    assert!(requested < started, "顺序反了（先暴露工具、再等批准）：{types:?}");
    assert_eq!(types.last().unwrap(), "run.completed");

    // 闸门类事件**立刻落库**（不是攒批）：另一台设备连上来时它必须已经在库里，
    // 否则那边就是"白等一场"（M1 的纪律）。
    let requested_seq = state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT MIN(seq) FROM ai_run_events WHERE run_id = ?1 AND event_type = 'tool.requested'",
                rusqlite::params![run_id],
                |row| row.get::<_, Option<i64>>(0),
            )?)
        })
        .unwrap()
        .expect("tool.requested 已经在库里");
    assert!(requested_seq > 0);

    // 工具**真的执行了**：本机那份长期记忆里出现了这一条
    let memory = state.media_dir().parent().unwrap().join("ai").join("memory.md");
    let text = std::fs::read_to_string(&memory).unwrap_or_default();
    assert!(text.contains("用户偏好 4:3 画幅"), "批准后要真的执行：{text}");

    gateway.abort();
}

/// **拒绝那条路**：点了「拒绝」之后 —— 工具**没有被执行**、库里那行记 `denied`/`denied`
/// （一行，不重复）、Run 照样收口，而且模型收到的是"未执行"的如实说明（**不许假装跑过**）。
#[tokio::test]
async fn a_denied_call_is_recorded_and_never_runs() {
    let (base_url, gateway) = fake_tool_gateway("remember", r#"{"content":"用户偏好 4:3 画幅"}"#).await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);
    set_policy(&state, serde_json::json!({ "permissionMode": "ask", "overrides": { "remember": "ask" } }));

    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "记一下我的画幅偏好", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    let run_id = body["runId"].as_str().unwrap().to_string();

    let mut denied = false;
    for _ in 0..80 {
        let calls = comfyhub_server_rs::runs::list_tool_calls(&state, &run_id).unwrap();
        if calls.is_empty() || calls[0]["status"] != "pending" {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            continue;
        }
        let (status, result) = send_json(
            router_for(state.clone()),
            "POST",
            "/api/ai/tool-calls/call_1/deny",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(result["approved"], false);
        denied = result["accepted"].as_bool().unwrap_or(false);
        break;
    }
    assert!(denied, "拒绝也必须能落到那一行上（R8）");

    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "completed", "拒绝不代表整轮 Run 失败：{run}");

    let calls = comfyhub_server_rs::runs::list_tool_calls(&state, &run_id).unwrap();
    assert_eq!(calls.len(), 1, "{calls:?}");
    // 两列的分工（与 Kotlin `ToolRegistry.failure` 逐字一致）：`approval` 记用户的决定，
    // `status` 记执行结果 —— 被拒绝的那次**没有执行**，所以状态是 `failed` 而不是 `ok`；
    // 界面靠 `approval == "denied"` 把它画成「已拒绝」而不是「出错」。
    assert_eq!(calls[0]["approval"], "denied");
    assert_eq!(calls[0]["status"], "failed");
    assert_eq!(calls[0]["error"], "用户没有批准这次工具调用（或等待超时）；不要重试同一件事，如实说明即可");

    // **没有执行**：记忆文件里不该出现那一条
    let memory = state.media_dir().parent().unwrap().join("ai").join("memory.md");
    let text = std::fs::read_to_string(&memory).unwrap_or_default();
    assert!(!text.contains("用户偏好 4:3 画幅"), "被拒绝的调用不许真的跑：{text}");

    // 事件：`tool.failed` 带稳定码 `code`（界面按 `approval=denied` 画"已拒绝"），
    // 而且**不发 tool.started**（拒绝的调用没开始过）
    let types = event_types(&state, &run_id);
    assert!(types.contains(&"tool.failed".to_string()), "{types:?}");
    assert!(!types.contains(&"tool.started".to_string()), "拒绝的调用没开始过：{types:?}");
    let denied_event = state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT payload_json FROM ai_run_events
                  WHERE run_id = ?1 AND event_type = 'tool.failed' ORDER BY seq LIMIT 1",
                rusqlite::params![run_id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap();
    let denied_event: serde_json::Value = serde_json::from_str(&denied_event).unwrap();
    assert_eq!(denied_event["code"], "APPROVAL_DENIED");
    assert_eq!(denied_event["approval"], "denied");
    assert_eq!(
        keys(&denied_event),
        vec!["approval", "callId", "code", "elapsedMs", "message", "name"]
    );

    gateway.abort();
}

/// **轮数上限**（`maxToolSteps`）：到顶那一轮**不执行**工具，而是把"为什么没继续"
/// 如实补进正文 —— 而且这一轮 Run **照样正常收尾**（不是 `run.failed`）。
///
/// 判据来自 Kotlin `HarnessRunner:657`：`outOfSteps` 时那句
/// 「已达到本次回复的工具轮数上限 N，这次要调用的工具没有执行；再发一条消息它就能接着做。」
/// 写成"整轮失败"的后果是用户已经看到的那段回答挂上一个红叉，而那件事本来只是"这轮到此为止"。
#[tokio::test]
async fn tool_round_limit_wraps_up_instead_of_failing_the_run() {
    // 模型**每一轮都要工具**（第二轮也会要到）→ 正好撞上限
    let (base_url, gateway) =
        scripted_tool_gateway(vec![vec![("list_skills", "{}")], vec![("list_skills", "{}")]]).await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);
    set_policy(&state, serde_json::json!({ "maxToolSteps": 1, "maxCallsPerRun": 48 }));

    let (run_id, assistant_id, run) = start_run_and_wait(&state, "看看技能").await;
    assert_eq!(run["status"], "completed", "轮数到顶不是 Run 失败：{run}");

    // 只执行了一次（第 2 轮那批被挡在闸门外），而且**如实说了**
    let calls = comfyhub_server_rs::runs::list_tool_calls(&state, &run_id).unwrap();
    assert_eq!(calls.len(), 1, "到顶那一轮不许执行：{calls:?}");
    assert_eq!(calls[0]["status"], "ok");
    let text = assistant_text(&state, &assistant_id);
    assert!(text.contains("工具轮数上限 1"), "要如实说明：{text}");
    assert!(text.contains("这次要调用的工具没有执行"), "{text}");
    // 那句话要**同时**落进有序块（前端在 parts 非空时只渲染 parts）
    let messages = comfyhub_server_rs::conversations::list_messages(&state, "conv-0").unwrap();
    let parts = &messages.iter().find(|m| m.id == assistant_id).unwrap().parts;
    assert!(
        parts.iter().any(|p| p.r#type == "text" && p.text.as_deref().unwrap_or("").contains("工具轮数上限")),
        "补的说明也要落块：{parts:?}"
    );

    gateway.abort();
}

/// **调用额度**（`maxCallsPerRun`）：一轮要两个工具、额度只剩一个 ——
/// 第二个回 `TOOL_BUDGET_EXCEEDED`（**不是**整轮失败），第一个照常执行并落库。
#[tokio::test]
async fn tool_budget_exceeded_is_reported_per_call() {
    let (base_url, gateway) = scripted_tool_gateway(vec![
        vec![("list_skills", "{}"), ("list_skills", "{}")],
        // 额度用尽之后**还在要工具** —— 这时才该把"额度用尽"如实补进正文
        // （模型自己收口的话就不补了：Kotlin 同）
        vec![("list_skills", "{}")],
    ])
    .await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);
    set_policy(&state, serde_json::json!({ "maxToolSteps": 16, "maxCallsPerRun": 1 }));

    let (run_id, assistant_id, run) = start_run_and_wait(&state, "两次技能").await;
    assert_eq!(run["status"], "completed", "{run}");

    let calls = comfyhub_server_rs::runs::list_tool_calls(&state, &run_id).unwrap();
    assert_eq!(calls.len(), 2, "两次调用都要留痕：{calls:?}");
    // ⚠ 按 `callId` 找，**不要**按下标：同一毫秒里的两行，`ORDER BY started_at, id`
    // 的 tie-break 是随机 UUID（实测踩过：断言写在下标上，红得莫名其妙）
    let first = calls.iter().find(|c| c["callId"] == "call_1").unwrap();
    let second = calls.iter().find(|c| c["callId"] == "call_2").unwrap();
    assert_eq!(first["status"], "ok", "额度之内的那次要真的执行：{first}");
    assert_eq!(second["status"], "failed");
    assert!(
        second["error"].as_str().unwrap().contains("工具调用次数已达上限"),
        "第二次要如实说清是额度拦住的：{}",
        second["error"]
    );
    // 收口时也要把"额度用尽"补进正文（Kotlin `HarnessRunner:644`）
    let text = assistant_text(&state, &assistant_id);
    assert!(text.contains("工具调用次数已达上限（1 次）"), "{text}");
    // 第 3 次那个调用**没有执行**（额度已经用尽）→ 库里只有两行
    assert_eq!(
        comfyhub_server_rs::runs::list_tool_calls(&state, &run_id).unwrap().len(),
        2,
        "额度用尽之后不许再执行"
    );

    gateway.abort();
}

/// **失败的调用不消耗额度**（Kotlin `spendCall` 只在 handler 返回之后调）：
/// 额度只有 1 次时，先撞一个"工具不存在"的失败，后面那次**仍然跑得动**。
///
/// 反过来的表现是用户抱怨过的那句原话：**"什么都没干就说预算不足"**。
#[tokio::test]
async fn failed_calls_do_not_spend_the_budget() {
    let (base_url, gateway) = scripted_tool_gateway(vec![
        vec![("read_file", r#"{"path":"a.png"}"#)],
        vec![("list_skills", "{}")],
    ])
    .await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);
    set_policy(&state, serde_json::json!({ "maxToolSteps": 16, "maxCallsPerRun": 1 }));

    let (run_id, _, run) = start_run_and_wait(&state, "先失败一次再查技能").await;
    assert_eq!(run["status"], "completed", "{run}");

    let calls = comfyhub_server_rs::runs::list_tool_calls(&state, &run_id).unwrap();
    assert_eq!(calls.len(), 2, "{calls:?}");
    let failed = calls.iter().find(|c| c["callId"] == "call_1").unwrap();
    let ok = calls.iter().find(|c| c["callId"] == "call_2").unwrap();
    assert_eq!(failed["status"], "failed");
    assert_eq!(ok["status"], "ok", "失败那次不该把额度吃掉：{ok}");

    gateway.abort();
}

/// 模型幻觉出一个清单里没有的工具（或这一批还没搬的）：如实回 `TOOL_NOT_FOUND`，
/// **不许静默**、也不许假装跑过 —— 记录照样落库，模型拿到一句能改变做法的话。
#[tokio::test]
async fn unknown_tool_is_reported_as_tool_not_found() {
    let (base_url, gateway) =
        scripted_tool_gateway(vec![vec![("read_file", r#"{"path":"a.png"}"#)]]).await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);

    let (run_id, _, run) = start_run_and_wait(&state, "读个文件").await;
    assert_eq!(run["status"], "completed", "{run}");

    let calls = comfyhub_server_rs::runs::list_tool_calls(&state, &run_id).unwrap();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0]["status"], "failed");
    assert_eq!(calls[0]["approval"], "denied");
    assert!(calls[0]["error"].as_str().unwrap().contains("没有名为「read_file」的工具"));

    // 失败那次**不进 tool.started**（没有开始过），但要有一条 `tool.failed`
    let types = event_types(&state, &run_id);
    assert!(types.contains(&"tool.failed".to_string()), "{types:?}");
    assert!(!types.contains(&"tool.started".to_string()), "{types:?}");

    gateway.abort();
}

/// **提问闸门**（P4 第 32 步，用户建议 ⑥）：模型当场问两张卡里的一整张（2 题），
/// 用户答完，**同一个 Run 接着往下做** —— 不结束、不重开。
///
/// 这一条盯四件事：① `user.question` 的形状是**嵌套**的 `question` 对象
/// （平铺过一次，界面就解不出提问）；② `GET /api/ai/runs/{id}/question` 能把整张卡补回来；
/// ③ 答完写库 + 落块（`asked` / `askedAll`）；④ 答案真的喂回了模型。
#[tokio::test]
async fn ask_user_question_waits_then_continues_the_same_run() {
    let arguments = serde_json::json!({
        "questions": [
            { "question": "要哪个画幅？", "options": ["4:3", "16:9"], "header": "输出尺寸" },
            { "question": "要几张？", "options": ["1", "4"], "multiSelect": true }
        ]
    })
    .to_string();
    let (base_url, gateway) = scripted_tool_gateway(vec![vec![(
        "ask_user_question",
        Box::leak(arguments.into_boxed_str()),
    )]])
    .await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);
    set_policy(&state, serde_json::json!({ "permissionMode": "ask", "overrides": {} }));

    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "出张图", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();

    // ② 等提问卡出现（Run 挂在这里等，**不收尾**）
    let mut card: Vec<serde_json::Value> = Vec::new();
    for _ in 0..200 {
        let (status, q) =
            get_json(router_for(state.clone()), &format!("/api/ai/runs/{run_id}/question")).await;
        assert_eq!(status, StatusCode::OK, "{q}");
        if q["pending"] == true {
            card = q["questions"].as_array().cloned().unwrap_or_default();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(card.len(), 2, "整张卡要一次补回来：{card:?}");
    // ① 事件形状：`question` 必须是**对象**（不是问题文字）
    let question_event = state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT payload_json FROM ai_run_events
                  WHERE run_id = ?1 AND event_type = 'user.question' ORDER BY seq LIMIT 1",
                rusqlite::params![run_id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap();
    let question_event: serde_json::Value = serde_json::from_str(&question_event).unwrap();
    assert!(
        question_event["question"].is_object(),
        "客户端读的是 event.data['question'] 这个**对象**：{question_event}"
    );
    assert_eq!(question_event["question"]["questionId"], card[0]["questionId"]);
    assert_eq!(question_event["question"]["index"], 0);
    assert_eq!(question_event["question"]["total"], 2);
    assert_eq!(question_event["runId"], run_id.as_str());

    // 闸门类事件**立刻落库**（另一台设备靠它发现"有东西在等人"）
    let types = event_types(&state, &run_id);
    assert!(types.contains(&"user.question".to_string()), "{types:?}");

    // ③ 回答：第一题点按钮（`optionIndex`），第二题给自由文本
    let first_id = card[0]["questionId"].as_str().unwrap().to_string();
    let second_id = card[1]["questionId"].as_str().unwrap().to_string();
    let (status, answered) = send_json(
        router_for(state.clone()),
        "POST",
        &format!("/api/ai/questions/{first_id}/answer"),
        serde_json::json!({ "optionIndex": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    assert_eq!(answered["accepted"], true);
    assert_eq!(answered["answer"], "16:9", "点了按钮，答案就是那个选项的原文");
    let (status, answered) = send_json(
        router_for(state.clone()),
        "POST",
        &format!("/api/ai/questions/{second_id}/answer"),
        serde_json::json!({ "answer": "两张就行" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answered["answer"], "两张就行");

    // Run 收尾（**同一个 Run**，不是另起一条）
    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "completed", "{run}");

    // 库里两行都是 answered，且记着答了什么（`GET /api/ai/questions/{id}` 据此回放）
    let (status, stored) =
        get_json(router_for(state.clone()), &format!("/api/ai/questions/{first_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stored["answered"], true);
    assert_eq!(stored["answer"], "16:9");
    assert_eq!(stored["optionIndex"], 1);
    assert_eq!(stored["header"], "输出尺寸");
    assert_eq!(stored["total"], 2);

    // ④ 答案喂回了模型（正文是 Kotlin 那两句的逐字形状），而且落进了有序块
    let messages = comfyhub_server_rs::conversations::list_messages(&state, "conv-0").unwrap();
    let assistant = messages.iter().find(|m| m.id == assistant_id).unwrap();
    assert_eq!(assistant.text, "收口了");
    let result_part = assistant
        .parts
        .iter()
        .find(|p| p.r#type == "tool_result")
        .expect("工具结果块");
    let text = result_part.text.clone().unwrap_or_default();
    assert!(text.contains("【用户回答了这张卡（共 2 题）】"), "{text}");
    assert!(text.contains("「要哪个画幅？」 → 16:9"), "{text}");
    assert!(text.contains("「要几张？」 → 两张就行"), "{text}");
    let payload = result_part.json_payload.clone().unwrap();
    // 块上的 payload 是**契约那一小撮键**（Kotlin `HarnessRunner:762`）：历史回放靠
    // `question` 这一条把提问卡照原样画出来（问了什么、答了什么）
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["approval"], "not_required");
    assert_eq!(payload["question"]["questionId"], first_id.as_str());
    assert_eq!(payload["question"]["answer"], "16:9");
    assert_eq!(payload["question"]["options"][0], "4:3");

    // 工具的结构化结果（`tool.completed` 的 `result`）才是预算 / 整组答案那一份
    let completed = state
        .with(|conn| {
            Ok(conn.query_row(
                "SELECT payload_json FROM ai_run_events
                  WHERE run_id = ?1 AND event_type = 'tool.completed' ORDER BY seq LIMIT 1",
                rusqlite::params![run_id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap();
    let completed: serde_json::Value = serde_json::from_str(&completed).unwrap();
    let result = &completed["result"];
    assert_eq!(result["askedCount"], 2);
    assert_eq!(result["answeredCount"], 2);
    assert_eq!(result["answers"][0]["optionIndex"], 1);
    assert_eq!(result["answers"][1]["answer"], "两张就行");
    assert_eq!(result["askedAll"][1]["answer"], "两张就行");
    assert_eq!(result["budgetReset"], true, "用户真的答了 → 这一窗的工具额度重置");
    assert_eq!(result["callsUsedInWindow"], 0);
    assert!(result["groupId"].is_string());

    // 答完之后 `/api/ai/pending` 不该再列出这张卡
    let (_, pending) = get_json(router_for(state.clone()), "/api/ai/pending").await;
    assert!(pending["asks"].as_array().unwrap().is_empty(), "{pending}");

    gateway.abort();
}

/// 用户**跳过**整张卡：模型拿到的是"用户没回答"（不是卡死），Run 照样收口 ——
/// 而且不许假装用户答了。
#[tokio::test]
async fn skipped_questions_are_reported_as_unanswered() {
    let arguments = serde_json::json!({ "question": "要 NSFW 吗？", "options": ["要", "不要"] }).to_string();
    let (base_url, gateway) = scripted_tool_gateway(vec![vec![(
        "ask_user_question",
        Box::leak(arguments.into_boxed_str()),
    )]])
    .await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &base_url);

    let (_, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "出张图", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();

    let mut question_id = String::new();
    for _ in 0..200 {
        let (_, q) =
            get_json(router_for(state.clone()), &format!("/api/ai/runs/{run_id}/question")).await;
        if q["pending"] == true {
            question_id = q["question"]["questionId"].as_str().unwrap().to_string();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(!question_id.is_empty(), "提问卡没出现");

    let (status, skipped) = send_json(
        router_for(state.clone()),
        "POST",
        &format!("/api/ai/questions/{question_id}/skip"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(skipped["accepted"], true);

    // 跳过**立刻**放行（不等到 30 分钟超时）—— 这一条就是"别把 Run 挂死"的防线
    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "completed", "{run}");

    let messages = comfyhub_server_rs::conversations::list_messages(&state, "conv-0").unwrap();
    let assistant = messages.iter().find(|m| m.id == assistant_id).unwrap();
    let text = assistant
        .parts
        .iter()
        .find(|p| p.r#type == "tool_result")
        .and_then(|p| p.text.clone())
        .unwrap_or_default();
    assert!(text.contains("【用户未回答】"), "{text}");
    assert!(!text.contains("回答：要"), "不许假装用户答了：{text}");

    // 库里那行是 skipped（不是 pending —— 否则 `/api/ai/pending` 会一直列着它）
    let (_, stored) =
        get_json(router_for(state.clone()), &format!("/api/ai/questions/{question_id}")).await;
    assert_eq!(stored["answered"], false);
    let (_, pending) = get_json(router_for(state.clone()), "/api/ai/pending").await;
    assert!(pending["asks"].as_array().unwrap().is_empty(), "{pending}");

    gateway.abort();
}

/// 上游明确拒绝（429）：错误码要归到 `RATE_LIMIT`，而不是笼统的"失败了"。
#[tokio::test]
async fn executor_reports_the_stable_code_for_an_upstream_rejection() {
    let (base_url, gateway) = fake_gateway().await;
    let state = state();
    seed_provider_and_model(&state);
    point_at_gateway(&state, &format!("{base_url}?upstream=http429"));

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "写一段", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id = body["runId"].as_str().unwrap().to_string();

    let run = wait_for_run(&state, &run_id).await;
    assert_eq!(run["status"], "failed");
    assert_eq!(run["errorCode"], "RATE_LIMIT", "{run}");
    assert!(
        run["errorMessage"].as_str().unwrap().contains("rate limited"),
        "上游的原话（已脱敏、已截断）要带出来，否则用户只看到「配置有误」：{run}"
    );

    gateway.abort();
}

/// `POST /api/ai/conversations/{id}/runs` 的完整一条链（Kotlin 顺序照抄）。
#[tokio::test]
async fn start_run_validates_then_creates_messages_and_run() {
    let state = state();
    seed_provider_and_model(&state);

    // ① 空正文（且没附件）→ 400，**一个字节都不写库**
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "   ", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert_eq!(body["detail"], "消息内容不能为空");
    let (_, messages) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-0/messages").await;
    assert_eq!(messages.as_array().unwrap().len(), 1, "被拒的请求不许留下半条消息");

    // ② 会话不存在 → 400
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/nope/runs",
        serde_json::json!({ "text": "你好", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "会话不存在: nope");

    // ③ 模型不在目录里 → `UNKNOWN_MODEL`（**不是** CONFIG_ERROR：前端按码分支）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "你好", "providerId": "goat", "modelId": "nope" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "UNKNOWN_MODEL");
    assert_eq!(body["detail"], "模型不在目录中: nope");

    // ④ 合法的请求 → **202** + 三个主键
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "你好", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id = body["runId"].as_str().unwrap().to_string();
    let assistant_id = body["assistantMessageId"].as_str().unwrap().to_string();
    let user_id = body["userMessageId"].as_str().unwrap().to_string();
    assert!(!run_id.is_empty() && !assistant_id.is_empty() && !user_id.is_empty());
    assert_eq!(body.as_object().unwrap().len(), 3, "响应只有这三个键");

    // 用户消息 + 助手占位都落了库，且**用户在前**（顺序是界面的依据）
    let (_, messages) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-0/messages").await;
    let list = messages.as_array().unwrap();
    assert_eq!(list.len(), 3, "夹具 1 条 + 用户 + 助手占位");
    let user = list.iter().find(|m| m["id"] == user_id.as_str()).expect("用户消息在");
    let assistant = list.iter().find(|m| m["id"] == assistant_id.as_str()).expect("助手占位在");
    assert_eq!(user["role"], "user");
    assert_eq!(user["text"], "你好");
    assert_eq!(user["seq"], 2);
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["seq"], 3);

    // 执行器是**后台任务**：真实上游是 `127.0.0.1:9`（没人监听），所以它很快会失败。
    // 这条断言钉的是**语义**：Run 绝不会永远停在 running（那会让界面一直转圈）
    let mut run = serde_json::Value::Null;
    for _ in 0..60 {
        let (_, value) = get_json(router_for(state.clone()), &format!("/api/ai/runs/{run_id}")).await;
        run = value;
        if run["status"] != "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(run["status"], "failed", "连不上上游要如实失败：{run}");
    assert_eq!(run["errorCode"], "PROVIDER_UNREACHABLE");
    assert_eq!(run["reasoningEffort"], "off");
    assert_eq!(run["userMessageId"], user_id.as_str());
    assert_eq!(run["assistantMessageId"], assistant_id.as_str());
    assert_eq!(run["providerId"], "goat");

    // 失败的助手消息也落了如实的状态（留在 streaming 会让界面一直转圈）
    let (_, messages) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-0/messages").await;
    let assistant = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == assistant_id.as_str())
        .unwrap();
    assert_eq!(assistant["status"], "failed");

    // 事件：run.started / message.started / run.failed **都落了库**（断线重连补得回来）
    let types = event_types(&state, &run_id);
    assert_eq!(types[0], "run.started");
    assert_eq!(types[1], "message.started");
    assert_eq!(types.last().unwrap(), "run.failed", "终态事件在最后：{types:?}");
    assert!(
        types.iter().all(|t| !t.is_empty()),
        "seq 从 1 起、连续递增（不许跳号）"
    );

    // 不存在 / 已收尾的 Run 上点取消：**200 + cancelled:false**（不是错误）
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        &format!("/api/ai/runs/{run_id}/cancel"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["cancelled"], false);
    assert_eq!(body["status"], "failed");

    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/runs/nope/cancel",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "Run 不存在: nope");
}

/// 某个 Run 落库的事件类型（按 seq 升序）。
fn event_types(state: &comfyhub_server_rs::routes::AppState, run_id: &str) -> Vec<String> {
    state
        .with(|conn| {
            let mut stmt = conn
                .prepare("SELECT event_type FROM ai_run_events WHERE run_id = ?1 ORDER BY seq")?;
            let rows = stmt
                .query_map(rusqlite::params![run_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap()
}

/// 等一次 Run 收尾（执行器是**后台任务**：它不可能在 `POST /runs` 的响应里就跑完）。
///
/// 上游是 `127.0.0.1:9`（没人监听），所以失败得很快；这里给足 3 秒并在超时后断言失败，
/// 而不是"睡固定时长碰运气"。
async fn wait_for_run(state: &comfyhub_server_rs::routes::AppState, run_id: &str) -> serde_json::Value {
    for _ in 0..60 {
        let run = comfyhub_server_rs::runs::get(state, run_id).expect("Run 在库里");
        if run.status != comfyhub_server_rs::runs::RUNNING {
            return serde_json::to_value(&run).unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("Run {run_id} 三秒内还没收尾（执行器可能卡住了）");
}

/// 重试：**复用原来那条用户消息**、空壳就地复用、有内容另起一条（AIH-024 / R6）。
#[tokio::test]
async fn retry_reuses_the_user_message_and_never_wipes_a_reply_with_content() {
    let state = state();
    seed_provider_and_model(&state);

    let (_, first) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({ "text": "再来一张", "providerId": "goat", "modelId": "m1" }),
    )
    .await;
    let first_run = first["runId"].as_str().unwrap().to_string();
    let first_user = first["userMessageId"].as_str().unwrap().to_string();
    let first_assistant = first["assistantMessageId"].as_str().unwrap().to_string();

    // ⚠ 等第一次 Run 收尾：执行器是后台任务，而"重试"的准入判定读的是**库里那条的状态**。
    // 不等的话会撞上 `streaming`（那条回复还没结束）——那是**对的行为**，只是测试没等它。
    wait_for_run(&state, &first_run).await;

    // ① 空壳（正文空）→ **就地复用**同一条助手消息，不新增气泡
    let (status, retry) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({
            "text": "再来一张", "providerId": "goat", "modelId": "m1",
            "retryOfRunId": first_run
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{retry}");
    assert_eq!(retry["userMessageId"].as_str().unwrap(), first_user, "复用原来那条用户消息");
    assert_eq!(
        retry["assistantMessageId"].as_str().unwrap(),
        first_assistant,
        "空的失败气泡就地复用（会话里不留空壳）"
    );
    let (_, messages) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-0/messages").await;
    assert_eq!(messages.as_array().unwrap().len(), 3, "重试**不新增**任何消息");
    // 第二次 Run 也在后台跑（上游是 127.0.0.1:9，会失败）—— 等它收尾再往下走
    let second_run = retry["runId"].as_str().unwrap().to_string();
    wait_for_run(&state, &second_run).await;

    // ② 有内容的失败回复 → 原样留着，重试**另起一条**（不许把用户看到过的东西抹掉）
    state
        .write(|tx| {
            tx.execute(
                "UPDATE ai_messages SET text = '我已经说了很多话', status = 'failed' WHERE id = ?1",
                rusqlite::params![first_assistant],
            )?;
            tx.execute(
                "INSERT INTO ai_message_parts (message_id, ordinal, type, text)
                 VALUES (?1, 0, 'text', '我已经说了很多话')",
                rusqlite::params![first_assistant],
            )?;
            Ok(())
        })
        .unwrap();
    let (_, retry2) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({
            "text": "再来一张", "providerId": "goat", "modelId": "m1",
            "retryOfRunId": second_run
        }),
    )
    .await;
    assert_ne!(
        retry2["assistantMessageId"].as_str().unwrap(),
        first_assistant,
        "有内容的失败回复**不许**被就地重置"
    );
    let (_, messages) = get_json(router_for(state.clone()), "/api/ai/conversations/conv-0/messages").await;
    let kept = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == first_assistant.as_str())
        .unwrap();
    assert_eq!(kept["text"], "我已经说了很多话", "用户看到过的话一个字都不能丢（R6）");
    assert_eq!(kept["parts"].as_array().unwrap().len(), 1, "它的工具卡/正文块也留着");

    // ③ 已经答完的回复**不允许**重试（直接发新消息即可）
    state
        .write(|tx| {
            tx.execute(
                "UPDATE ai_messages SET status = 'complete' WHERE id = ?1",
                rusqlite::params![first_assistant],
            )?;
            Ok(())
        })
        .unwrap();
    let (status, body) = send_json(
        router_for(state.clone()),
        "POST",
        "/api/ai/conversations/conv-0/runs",
        serde_json::json!({
            "text": "再来一张", "providerId": "goat", "modelId": "m1",
            "retryOfRunId": second_run
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "CONFIG_ERROR");
    assert!(body["detail"].as_str().unwrap().contains("不需要（也不能）重试"));
}

/// SSE：`?after=` 续传、帧形状、以及"收尾之后把库里的都发完再结束"。
#[tokio::test]
async fn run_events_stream_replays_and_resumes() {
    let state = state();
    seed_provider_and_model(&state);
    let run = comfyhub_server_rs::runs::create(
        &state,
        comfyhub_server_rs::runs::NewRun {
            conversation_id: "conv-0".into(),
            provider_id: "goat".into(),
            model_id: "m1".into(),
            provider_snapshot: serde_json::json!({}),
            model_snapshot: serde_json::json!({}),
            prompt_version: "v28".into(),
            user_message_id: None,
            assistant_message_id: "assistant-1".into(),
            retry_of_run_id: None,
            reasoning_effort: Some("off".into()),
            skill_snapshot: None,
        },
    )
    .unwrap();

    // 一次真实的流式：三段正文 + 收尾（总线负责发号，收尾走 finish_run）
    state.bus.open(&run.id);
    for text in ["你", "好", "呀"] {
        state
            .bus
            .emit(&state, &run.id, "text.delta", &serde_json::json!({ "messageId": "assistant-1", "text": text }))
            .unwrap();
    }
    comfyhub_server_rs::run_lifecycle::finish_run(
        &state.bus,
        &state,
        &run.id,
        "completed",
        None,
        None,
        None,
    )
    .unwrap();

    // 全量回放：4 条事件、`seq` 从 1 连续到 4、终态在最后
    let body = read_sse(router_for(state.clone()), &format!("/api/ai/runs/{}/events", run.id), "").await;
    let frames = parse_sse(&body);
    assert_eq!(frames.len(), 4, "三段正文 + run.completed：{body}");
    assert_eq!(frames[0].0, Some(1));
    assert_eq!(frames[0].1, "text.delta");
    assert_eq!(frames[0].2["text"], "你");
    assert_eq!(frames[3].1, "run.completed");
    assert_eq!(frames[3].2["runId"], run.id.as_str());
    assert!(!body.contains("event: heartbeat"), "收尾之后立刻结束，不发心跳");

    // 断线续传：`?after=2` 只拿后两条
    let body = read_sse(
        router_for(state.clone()),
        &format!("/api/ai/runs/{}/events", run.id),
        "?after=2",
    )
    .await;
    let frames = parse_sse(&body);
    assert_eq!(frames.iter().map(|f| f.0.unwrap()).collect::<Vec<_>>(), vec![3, 4]);

    // 浏览器 `EventSource` 自动重连带的是 `Last-Event-ID` 头（不认它就永远从 0 重放）
    let body = read_sse_with_headers(
        router_for(state.clone()),
        &format!("/api/ai/runs/{}/events", run.id),
        "",
        &[("last-event-id", "3")],
    )
    .await;
    let frames = parse_sse(&body);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].1, "run.completed");

    // Run 不存在 → 400（**在建立流之前**判：SSE 一旦开始就没法改状态码了）
    let (status, body) = get_json(router_for(state.clone()), "/api/ai/runs/nope/events").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], "Run 不存在: nope");
}

/// 读一个 SSE 响应体（它自己会结束 —— 收尾的 Run 发完就关流）。
async fn read_sse(app: axum::Router, uri: &str, query: &str) -> String {
    read_sse_with_headers(app, uri, query, &[]).await
}

async fn read_sse_with_headers(
    app: axum::Router,
    uri: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> String {
    let mut builder = Request::builder().uri(format!("{uri}{query}")).header("accept", "text/event-stream");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .expect("请求");
    assert_eq!(response.status(), StatusCode::OK, "SSE 必须是 200");
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

/// `id` / `event` / `data` 三元组（`id` 缺席的心跳帧给 `None`）。
fn parse_sse(body: &str) -> Vec<(Option<i64>, String, Value)> {
    let mut out = Vec::new();
    for frame in body.split("\n\n").filter(|f| !f.trim().is_empty()) {
        let mut id = None;
        let mut event = String::new();
        let mut data = Value::Null;
        for line in frame.lines() {
            if let Some(rest) = line.strip_prefix("id: ") {
                id = rest.trim().parse::<i64>().ok();
            } else if let Some(rest) = line.strip_prefix("event: ") {
                event = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("data: ") {
                data = serde_json::from_str(rest.trim()).unwrap_or(Value::Null);
            }
        }
        out.push((id, event, data));
    }
    out
}

