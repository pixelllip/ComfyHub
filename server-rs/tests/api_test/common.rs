//! 这一组用例的共享夹具（2026-09-30 从 `api_test.rs` 拆出来）。
//!
//! 拆法是**单测试二进制 + 子模块**：`api_test.rs` 只声明 `mod`，各个域一个文件，
//! 夹具全在这里。子模块开头写 `use super::common::*;` 就能拿到全部（含重新导出的 use）。

#![allow(dead_code)]

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

pub(crate) use axum::body::Body;
pub(crate) use axum::http::{Request, StatusCode};
pub(crate) use http_body_util::BodyExt;
pub(crate) use rusqlite::Connection;
pub(crate) use serde_json::Value;
pub(crate) use tower::ServiceExt;

/// 用 `db/schema.sqlite.sql`（**唯一真源**）建库，再灌一点测试数据。
pub(crate) fn test_conn() -> Connection {
    let conn = comfyhub_server_rs::infra::db::open_memory().expect("内存库");
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

pub(crate) fn app() -> axum::Router {
    router_for(state())
}

/// 共享状态的库（写端点用例要在**同一条连接**上断言写完之后的样子）。
///
/// 顺带把产物目录准备好：夹具里 `media_assets.id=100` 的 `stored_name` 是 `a.png`，
/// 文件流端点要真的能在磁盘上找到它（内容已知，便于断言 Range 的字节）。
pub(crate) fn state() -> comfyhub_server_rs::web::routes::AppState {
    // ⚠ 唯一性靠"**纳秒 + 进程内计数器**"，只靠纳秒**撞过**：Windows 上
    // `SystemTime` 的粒度约 100ns，并行用例里两个 `state()` 拿到同一个值，
    // 于是两个用例共用同一个临时根 —— 一个写进去的长期记忆被另一个读到，
    // 表现是"拒绝了的调用居然写进了记忆"这种莫名其妙的红（2026-09-25 实测）。
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = comfyhub_server_rs::infra::testpaths::scratch(&format!("api-{seq}"));
    let media = root.join("media");
    std::fs::create_dir_all(&media).unwrap();
    std::fs::write(media.join("a.png"), b"0123456789abcdef").unwrap();
    comfyhub_server_rs::web::routes::AppState::new(test_conn(), ":memory:", media)
}

pub(crate) fn router_for(state: comfyhub_server_rs::web::routes::AppState) -> axum::Router {
    comfyhub_server_rs::web::routes::router(state)
}

/// 发一个带 JSON body 的请求（写端点用）。
pub(crate) async fn send_json(
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

pub(crate) async fn send_empty(app: axum::Router, method: &str, uri: &str) -> (StatusCode, Value) {
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

pub(crate) async fn get_json(app: axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .expect("请求");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

pub(crate) fn keys(value: &Value) -> Vec<String> {
    let mut out: Vec<String> = value.as_object().expect("对象").keys().cloned().collect();
    out.sort();
    out
}

// ===========================================================================
//  错误体形状（2026-09-24 对齐）：`{error, detail?}` —— **没有 `message` 这个键**
// ===========================================================================

/// `error` 是不是一个"机器码"（`NOT_FOUND` / `BAD_REQUEST` / `SCHEMA_MISMATCH` 那种全大写）。
pub(crate) fn looks_like_a_machine_code(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
}
// ===========================================================================
//  本机工作流文件（P4 第 14 步）：GET /api/capture/workflows
// ===========================================================================

/// 起一个"磁盘上真有工作流文件"的环境，并把它接进 `AppState`。
///
/// 目录布局照**发布包/源码树的真实形状**摆：`<根>\storage\media` 是产物目录，
/// 于是 `AppState::project_root()` 算出来就是 `<根>`（与脚本传的 `COMFYHUB_PROJECT_ROOT` 等价）。
pub(crate) fn state_with_workflow_files() -> (
    comfyhub_server_rs::web::routes::AppState,
    std::path::PathBuf,
) {
    let root = comfyhub_server_rs::infra::testpaths::scratch("api-workflows");

    // 项目内的 comfyui（三份目录来源里的第 ② 份）
    let home = root.join("comfyui");
    std::fs::create_dir_all(home.join("comfy")).unwrap();
    std::fs::write(home.join("main.py"), "print(1)").unwrap();
    let workflows = home.join("user").join("default").join("workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(
        workflows.join("krea2_hd.json"),
        r#"{"nodes":[],"links":[]}"#,
    )
    .unwrap();
    std::fs::write(workflows.join("broken.json"), "{ 不是 json").unwrap();

    let media = root.join("storage").join("media");
    std::fs::create_dir_all(&media).unwrap();

    let conn = test_conn();
    // 其中一份已经在库里（`comfy_load_workflow` 用的就是 `run_key = file:<sha256>`）
    let sha = comfyhub_server_rs::comfy::comfy_dirs::sha256_hex(br#"{"nodes":[],"links":[]}"#);
    conn.execute(
        "INSERT INTO capture_runs (run_key, status, media_count, raw, prompt_id, created_at)
         VALUES (?1, 'success', 0, NULL, 10, '2026-09-24T00:00:00.000Z')",
        rusqlite::params![format!("file:{sha}")],
    )
    .unwrap();

    (
        comfyhub_server_rs::web::routes::AppState::new(conn, ":memory:", media),
        home,
    )
}
// ===========================================================================
//  上传（P4 第 6 步）：multipart 多文件 + sha256 判重
// ===========================================================================

/// 构造一个 multipart 体（避免引入第三方 crate：这一小段手工拼接即可，且**结构与 curl 一致**）。
pub(crate) fn multipart_body(
    boundary: &str,
    fields: &[(&str, &str)],
    files: &[(&str, &str, &[u8])],
) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, value) in fields {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        out.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    for (name, filename, bytes) in files {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        out.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n")
                .as_bytes(),
        );
        out.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

pub(crate) async fn post_multipart(
    app: axum::Router,
    uri: &str,
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let boundary = "----comfyhubtest";
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let response = app.oneshot(request).await.expect("请求");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

// ---------------------------------------------------------------------------
//  POST /api/sync/push（M4 的 DoD 就在这里）
// ---------------------------------------------------------------------------

/// ⚠ 信封是 camelCase（`kind` / `refId` / `deviceId`），但 **payload 里是数据库列名（snake_case）**
/// —— 契约 §2 的例子就是 `{ "id": 12, "text": "…", "updated_at": "…" }`。
/// payload 走的是"完整行直接覆盖本地行"，所以它必须与列名同形，不能再转一道驼峰。
pub(crate) fn conv_payload(id: &str, title: &str, updated_at: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "system_prompt_version": "v1",
        "archived": 0,
        "created_at": "2026-09-24T00:00:00.000Z",
        "updated_at": updated_at,
    })
}

pub(crate) fn push_item(kind: &str, ref_id: &str, payload: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "kind": kind, "refId": ref_id, "op": "upsert", "payload": payload })
}

pub(crate) async fn push(
    state: &comfyhub_server_rs::web::routes::AppState,
    batch: Vec<serde_json::Value>,
) -> (StatusCode, Value) {
    send_json(
        router_for(state.clone()),
        "POST",
        "/api/sync/push",
        serde_json::json!({ "deviceId": "phone-test", "batch": batch }),
    )
    .await
}

// ===========================================================================
//  run-kit 工件（P4 第 22 步）：GET /api/ai/run-kit{,/status}
// ===========================================================================

/// 把夹具摆成"一台配好的 Provider + 三个模型（两个启用）+ 一条长期记忆 + 一个 skill"。
///
/// 这几个正是 `systemPrompt` 里那几段的来源：环境头、工具清单、白名单、记忆、Skills ——
/// 少灌一样，"那一段在提示词里到底写对没写对"就完全没被走到。
pub(crate) fn seed_run_kit(state: &comfyhub_server_rs::web::routes::AppState) {
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
    std::fs::write(
        storage.join("ai").join("memory.md"),
        "- 用户偏好 4:3 画幅\n",
    )
    .unwrap();
    std::fs::write(
        storage
            .join("ai")
            .join("skills")
            .join("demo-skill")
            .join("SKILL.md"),
        "---\nname: demo-skill\ndescription: 演示用\nwhenToUse: 演示\n---\n正文\n",
    )
    .unwrap();
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
pub(crate) async fn send_as(
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
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(addr));
    let response = app.oneshot(request).await.expect("请求");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// 往 `app_settings` 里写一行（闸门状态就存在那儿：`auth.pairing`）。
pub(crate) fn put_setting(
    state: &comfyhub_server_rs::web::routes::AppState,
    key: &str,
    value: &str,
) {
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

pub(crate) fn read_setting(
    state: &comfyhub_server_rs::web::routes::AppState,
    key: &str,
) -> Option<Value> {
    state
        .with(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT v FROM app_settings WHERE k = ?1",
                    rusqlite::params![key],
                    |r| r.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten();
            Ok(raw)
        })
        .unwrap()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

/// 闸门开着 + 一台已配对设备 + 一个全局令牌。
pub(crate) fn seed_auth(
    state: &comfyhub_server_rs::web::routes::AppState,
    allow_credentials: bool,
) {
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
pub(crate) fn auth_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

// ---------------------------------------------------------------------------
//  Run 持久化（P4 第 26 步）—— 事件流的**两条读法**共用同一份事实
// ---------------------------------------------------------------------------

/// 夹具会话的 provider / 模型（这两行是 Run 的外键目标）。
///
/// ⚠ 必须真的插进 `ai_providers` / `ai_models`：`ai_runs.provider_id` 没有外键，
/// 但 `run_kit` 与模型目录要它们；而"夹具里没有 provider"正是那种
/// "测试全部通过、真实库才失败"的缺口（`/api/sync/changes` 那次 500 的教训）。
pub(crate) fn seed_provider_and_model(state: &comfyhub_server_rs::web::routes::AppState) {
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

// ---------------------------------------------------------------------------
//  执行器主循环（P4 第 30 步）：对着**真 socket 上的假网关**跑一遍
// ---------------------------------------------------------------------------

/// 假网关读请求的**唯一入口**：把这一次请求（含 body）读干净，返回原始字节。
///
/// # 为什么不能"只读一次就回包"
///
/// 请求体是大件（系统提示 + 37 个工具的 JSON Schema，几十 KB），而假网关原来**最多读
/// 16KB**（最窄的那处只有 8KB）、甚至**一个字都不读**（429 那条路径），然后就写响应并
/// `close`。Windows 上**带着未读数据关闭会回 RST**，而客户端此刻可能还在写 body ——
/// 它的 `send()` 就报 `error sending request`，用例把它读成"连不上上游"
/// （`PROVIDER_UNREACHABLE`），于是一条真断言变成**偶发红灯**。
/// 2026-10 实测：全量并行跑约 **1/10 轮**红、单跑必绿；**HEAD 上也一样** ——
/// 这是假网关的既有毛病，不是被测代码的问题。
///
/// 认不出 `content-length`（GET / chunked）时**立刻返回**：那种情况下对端在等我们的回包，
/// 一直读到对端关闭会**死等**。整段读另有 2 秒上限 —— 宁可少读，也不让用例卡住。
pub(crate) async fn read_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    let mut all: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    let mut headers_done = false;
    let mut complete_at: Option<usize> = None;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match socket.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => all.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
            if !headers_done {
                let Some(pos) = all.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                headers_done = true;
                complete_at = content_length_of(&all[..pos]).map(|len| pos + 4 + len);
            }
            match complete_at {
                // 头 + body 都收齐了
                Some(total) if all.len() >= total => break,
                // 认不出长度 → 别等对端关（它在等我们回包）
                None => break,
                // 还没读够 → 继续
                _ => {}
            }
        }
    })
    .await;
    all
}

/// 请求行（第一个 `\n` 之前那一段）—— 假网关的分流判据读的就是它。
pub(crate) fn request_line_of(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string()
}

/// 从请求头里取 `content-length`（没有 / 认不出 → `None`）。
fn content_length_of(head: &[u8]) -> Option<usize> {
    String::from_utf8_lossy(head).lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("content-length") {
            return None;
        }
        value.trim().parse().ok()
    })
}

/// 假网关：`127.0.0.1:<临时端口>`，只认 `POST /v1/chat/completions?upstream=…`。
///
/// 为什么要一个真 socket 服务器（而不是 mock 掉 `stream_chat`）：
/// **"逐字增量 → SSE 分帧 → 适配器 → 落库"这条链的每一段都在真 I/O 上**，
/// mock 掉就只剩"我以为" —— M3.3 那条教训（`IOException` 只翻了一半）就是这么来的。
///
/// 三个行为分岔（用查询参数选，默认 `ok`）：
/// * `upstream=ok`：正常流（思考 → 三段正文 → finish_reason → usage → `[DONE]`）；
/// * `upstream=http429`：直接 429（测"上游明确拒绝"那一档）；
/// * `upstream=usage-then-die`：第一轮**正常跑完**（但 finish_reason=length，触发"接着写"），
///   第二轮直接 429 —— 测"未完成的对话也要统计已消耗的 token"；
/// * `upstream=partial`：**先正常吐出两段正文，然后把连接掐掉** ——
///   R6"已经说出去的话一个字都不能丢"的现场（真·中途断流，而不是事后报错）。
pub(crate) async fn fake_gateway() -> (String, tokio::task::JoinHandle<()>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let hits = hits.clone();
            tokio::spawn(async move {
                // 读干净（含 body）再回 —— 不读就 close 会回 RST，客户端把它读成
                // "连不上上游"。理由与代价见 [`read_request`]。
                let request_line = request_line_of(&read_request(&mut socket).await);
                let reply_429 = |mut socket: tokio::net::TcpStream| async move {
                    // ⚠ 这里**不再读请求**：调用点已经用 `read_request` 把 body 收干净了
                    // （不读就 close 会回 RST，客户端报的是"连不上上游"）。
                    let body = r#"{"error":"rate limited"}"#;
                    let head = format!(
                        "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(body.as_bytes()).await;
                    let _ = socket.shutdown().await;
                };
                if request_line.contains("upstream=http429") {
                    return reply_429(socket).await;
                }
                // 第一轮跑完、第二轮 429：分岔靠**请求计数**（"接着写"是一次新的上游请求）
                if request_line.contains("upstream=usage-then-die") {
                    let hit = hits.fetch_add(1, Ordering::SeqCst);
                    if hit > 0 {
                        return reply_429(socket).await;
                    }
                }
                let partial = request_line.contains("upstream=partial");
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
                    .await;
                let truncated = request_line.contains("upstream=usage-then-die");
                let mut frames: Vec<&str> = vec![
                    r#"{"id":"chatcmpl-1","choices":[{"delta":{"reasoning_content":"先想一下"}}]}"#,
                    r#"{"choices":[{"delta":{"content":"你"}}]}"#,
                    r#"{"choices":[{"delta":{"content":"好"}}]}"#,
                ];
                if partial {
                    // `partial`：**不说 DONE、直接断** —— 上游中途死掉的样子
                } else if truncated {
                    // 第一轮"被输出长度截断"：usage 已给（这轮的账已经发生），但话没说完
                    frames.push(r#"{"choices":[{"finish_reason":"length"}]}"#);
                    frames.push(
                        r#"{"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3}}"#,
                    );
                    frames.push("[DONE]");
                } else {
                    frames.push(r#"{"choices":[{"delta":{"content":"呀"}}]}"#);
                    frames.push(r#"{"choices":[{"finish_reason":"stop"}]}"#);
                    // usage 带上 OpenAI 那两处**嵌套明细**（缓存命中 / 思考）：
                    // 它们曾经在归一化时被整批丢掉。断言在 `ai_executor.rs` 的
                    // `executor_streams_a_reply_and_persists_parts_in_order`（落库四字段
                    // + `usage.updated` 平铺四字段那两条）。
                    // ⚠ 必须**单行**：SSE 是按行切的，JSON 里带换行会被切成半截帧。
                    frames.push(
                        r#"{"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":4},"completion_tokens_details":{"reasoning_tokens":1}}}"#,
                    );
                    frames.push("[DONE]");
                }
                for frame in frames {
                    let payload = format!("data: {frame}\n\n");
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

/// 把夹具 provider 的 Base URL 指到假网关。
pub(crate) fn point_at_gateway(state: &comfyhub_server_rs::web::routes::AppState, base_url: &str) {
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

/// 假网关的第二形态：**先要求调一个工具，拿到结果后再答**（工具往返两轮）。
///
/// `tool` 是模型要调的工具名；`arguments` 是它给的参数 JSON。
/// 第二次请求（带上工具结果的那一轮）回一段正文。
pub(crate) async fn fake_tool_gateway(
    tool: &'static str,
    arguments: &'static str,
) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let mut round = 0;
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            round += 1;
            tokio::spawn(async move {
                // 读干净再回（理由见 [`read_request`]）—— 这里不看请求内容，只按**轮次**分流。
                // ⚠ 别退回"只读一次"：请求体里带着整份系统提示 + 37 个工具的 schema，不小。
                let _ = read_request(&mut socket).await;
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
                        r#"{"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":5}}"#
                            .to_string(),
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
pub(crate) fn set_policy(
    state: &comfyhub_server_rs::web::routes::AppState,
    json: serde_json::Value,
) {
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
pub(crate) async fn scripted_tool_gateway(
    script: Vec<Vec<(&'static str, &'static str)>>,
) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // 调用号**跨轮连续**（call_1 / call_2 / …）：真实上游的 id 也是全局唯一的，
    // 每轮都从 call_1 重新数会让"按 callId 找那一条"的断言张冠李戴（实测踩过）
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let handle = tokio::spawn(async move {
        let mut round = 0usize;
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let calls = script.get(round).cloned();
            let counter = counter.clone();
            round += 1;
            tokio::spawn(async move {
                // 读干净再回（理由见 [`read_request`]）
                let _ = read_request(&mut socket).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
                    .await;
                let frames: Vec<String> = match calls {
                    Some(calls) if !calls.is_empty() => {
                        let mut out = Vec::new();
                        for (index, (tool, arguments)) in calls.iter().enumerate() {
                            let call_id =
                                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
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

/// 脚本化假网关（`openai-responses`，思考模式）：**第 2 轮起必须看见 `reasoning` 项**，
/// 不可见时按该上游原文返回 400 —— 这正是此前遇到的情形：
/// `The reasoning_text in the thinking mode must be passed back to the API.`
///
/// 同一轮里还按 OpenCode Go 的校验再来一次：`reasoning` 项必须带 `summary` 数组，
/// 只发 `content` 时回 `input[1].summary: expected an array`（2026-11-11 现场）。
///
/// 返回 `(base_url, 记录到的每一轮请求体)`：第 2 份就是"工具结果回填"那次，
/// 用例直接断言它的形状（`reasoning` 项在 `function_call` 之前、`summary` 与
/// `content` 两个字段都在）。
pub(crate) async fn reasoning_echo_gateway() -> (
    String,
    tokio::task::JoinHandle<()>,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let recorder = seen.clone();
    let handle = tokio::spawn(async move {
        let mut round = 0usize;
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            round += 1;
            let recorder = recorder.clone();
            tokio::spawn(async move {
                // 读**整份**请求（要按内容断言「思考有没有被回传」），顺便避免
                // "没读完就 close"那颗 RST —— 理由见 [`read_request`]。
                let request = String::from_utf8_lossy(&read_request(&mut socket).await).to_string();
                recorder.lock().unwrap().push(request.clone());
                if round >= 2 && !request.contains("reasoning_text") {
                    let body = r#"{"error":{"message":"The `reasoning_text` in the thinking mode must be passed back to the API.","type":"invalid_request_error"}}"#;
                    let _ = socket
                        .write_all(
                            format!(
                                "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                                body.len()
                            )
                            .as_bytes(),
                        )
                        .await;
                    let _ = socket.shutdown().await;
                    return;
                }
                if round >= 2
                    && request.contains(r#""type":"reasoning""#)
                    && !request.contains(r#""type":"summary_text""#)
                {
                    let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"input[1].summary: expected an array"}}"#;
                    let _ = socket
                        .write_all(
                            format!(
                                "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                                body.len()
                            )
                            .as_bytes(),
                        )
                        .await;
                    let _ = socket.shutdown().await;
                    return;
                }
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
                    .await;
                let frames: Vec<&str> = if round == 1 {
                    vec![
                        r#"{"type":"response.created","response":{"id":"resp_1","status":"in_progress"}}"#,
                        r#"{"type":"response.reasoning_text.delta","delta":"先看看技能"}"#,
                        r#"{"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"item_1","call_id":"call_1","name":"list_skills","arguments":""}}"#,
                        r#"{"type":"response.function_call_arguments.delta","output_index":1,"item_id":"item_1","delta":"{}"}"#,
                        r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}}}"#,
                    ]
                } else {
                    vec![
                        r#"{"type":"response.output_text.delta","delta":"收口了"}"#,
                        r#"{"type":"response.completed","response":{"id":"resp_2","status":"completed","usage":{"input_tokens":9,"output_tokens":3,"total_tokens":12}}}"#,
                    ]
                };
                for frame in frames {
                    if socket
                        .write_all(format!("data: {frame}\n\n").as_bytes())
                        .await
                        .is_err()
                    {
                        return;
                    }
                    let _ = socket.flush().await;
                }
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), handle, seen)
}

/// 起一个 Run 并等它收尾（工具类用例的公共开头）。
pub(crate) async fn start_run_and_wait(
    state: &comfyhub_server_rs::web::routes::AppState,
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
pub(crate) fn assistant_text(
    state: &comfyhub_server_rs::web::routes::AppState,
    assistant_id: &str,
) -> String {
    let messages = comfyhub_server_rs::ai::conversations::list_messages(state, "conv-0").unwrap();
    messages
        .iter()
        .find(|m| m.id == assistant_id)
        .map(|m| m.text.clone())
        .unwrap_or_default()
}

/// 某个 Run 落库的事件类型（按 seq 升序）。
///
/// ⚠ **先 flush 再查**：事件是**攒批**写的（`EVENT_BATCH = 200`），而 `finish_run` 的顺序是
/// `emit(run.completed)` → `runs::finish`（**状态在这一步变终态**）→ `bus.close`（flush）。
/// 所以 `wait_for_run` 一返回，事件**可能还没落库** —— 直接查库就是一条偶发红灯
/// （2026-10 全量并行跑撞到过两次，单跑必绿）。
/// 生产侧的订阅路径正是这么做的：`SseStream::poll_next` 每轮读库前先 `bus.flush`，同一个理由。
pub(crate) fn event_types(
    state: &comfyhub_server_rs::web::routes::AppState,
    run_id: &str,
) -> Vec<String> {
    let _ = state.bus.flush(state, run_id);
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

/// 某个 Run 里**第一条**指定类型事件的载荷（`None` = 没收过这个类型）。
///
/// 与 [`event_types`] 同样先 flush。用它，**别在用例里自己 `query_row(..).unwrap()`** ——
/// 后者对"还没落库"和"落了两条"都会 panic，是一颗偶发红灯。
pub(crate) fn event_payload(
    state: &comfyhub_server_rs::web::routes::AppState,
    run_id: &str,
    kind: &str,
) -> Option<Value> {
    use rusqlite::OptionalExtension;
    let _ = state.bus.flush(state, run_id);
    let raw: Option<String> = state
        .with(|conn| {
            conn.query_row(
                "SELECT payload_json FROM ai_run_events WHERE run_id = ?1 AND event_type = ?2 \
                     ORDER BY seq LIMIT 1",
                rusqlite::params![run_id, kind],
                |row| row.get::<_, String>(0),
            )
            .optional()
        })
        .unwrap();
    raw.map(|text| serde_json::from_str(&text).expect("payload_json 必须是合法 JSON"))
}

/// 等一次 Run 收尾（执行器是**后台任务**：它不可能在 `POST /runs` 的响应里就跑完）。
///
/// 上游是 `127.0.0.1:9`（没人监听），所以失败得很快；这里给足 3 秒并在超时后断言失败，
/// 而不是"睡固定时长碰运气"。
pub(crate) async fn wait_for_run(
    state: &comfyhub_server_rs::web::routes::AppState,
    run_id: &str,
) -> serde_json::Value {
    // ⚠ 这个上限是**"卡住了"的判据**，不是性能判据：假网关跑完一轮通常几十毫秒，
    //   但 150 个用例并行（每个都起自己的 axum + tokio 任务）时实测能压到秒级 ——
    //   原来写 60×50ms = 3s，负载高时会**偶发**在这里超（2026-09-30 撞到一次：
    //   同一个用例单跑必绿、全量并行时红）。放到 10s：真卡住照样抓得住，
    //   机器一时忙却不再报假红。
    for _ in 0..200 {
        let run = comfyhub_server_rs::ai::runs::get(state, run_id).expect("Run 在库里");
        if run.status != comfyhub_server_rs::ai::runs::RUNNING {
            return serde_json::to_value(&run).unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("Run {run_id} 十秒内还没收尾（执行器可能真的卡住了）");
}

/// 读一个 SSE 响应体（它自己会结束 —— 收尾的 Run 发完就关流）。
pub(crate) async fn read_sse(app: axum::Router, uri: &str, query: &str) -> String {
    read_sse_with_headers(app, uri, query, &[]).await
}

pub(crate) async fn read_sse_with_headers(
    app: axum::Router,
    uri: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> String {
    let mut builder = Request::builder()
        .uri(format!("{uri}{query}"))
        .header("accept", "text/event-stream");
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

// ---------------------------------------------------------------------------
//  Skills 六条（P4 第 35 步）
// ---------------------------------------------------------------------------

/// 夹具里的两个 skill 根：`<root>/skills/builtin`（只读）与 `<root>/ai/skills`（投放口）。
pub(crate) fn skill_dirs(
    state: &comfyhub_server_rs::web::routes::AppState,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let storage = state.media_dir().parent().unwrap().to_path_buf();
    (
        state.project_root().join("skills").join("builtin"),
        storage.join("ai").join("skills"),
    )
}

// ---------------------------------------------------------------------------
//  长期记忆五条（P4 第 36 步）
// ---------------------------------------------------------------------------

/// `<storage>\ai` —— 记忆文件的目录（端点内部就是这么拼的）。
pub(crate) fn memory_dir(state: &comfyhub_server_rs::web::routes::AppState) -> std::path::PathBuf {
    state.media_dir().parent().unwrap().join("ai")
}

/// `id` / `event` / `data` 三元组（`id` 缺席的心跳帧给 `None`）。
pub(crate) fn parse_sse(body: &str) -> Vec<(Option<i64>, String, Value)> {
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

// ---------------------------------------------------------------------------
//  凭据的**写侧**（P4 第 34 步）
// ---------------------------------------------------------------------------

/// 往夹具里加一个 provider（凭据端点的三段判定里第一段就是 `requireProvider`）。
pub(crate) fn add_provider(
    state: &comfyhub_server_rs::web::routes::AppState,
    id: &str,
    reference: Option<&str>,
) {
    state
        .with(|conn| {
            conn.execute(
                "INSERT INTO ai_providers
                   (id, display_name, api, base_url, credential_ref, endpoint_trust, enabled,
                    revision, created_at, updated_at)
                 VALUES (?1, ?2, 'openai', 'https://api.example.com', ?3, 'public', 1, 1,
                         '2026-09-25T00:00:00.000Z', '2026-09-25T00:00:00.000Z')",
                rusqlite::params![id, format!("夹具 {id}"), reference],
            )?;
            Ok(())
        })
        .expect("插 provider");
}

// ===========================================================================
//  桌面文件工具五件（P4 第 40 步）
//
//  这一批的判据是**真的落盘 / 真的拒绝**：写进去的文件要真的在磁盘上、
//  越界写要拿到 `PATH_DENIED`（而不是"写成功了但写到别处"）。
// ===========================================================================

/// 直接调工具层（不起 HTTP）：`execute` 就是执行器的入口，
/// 这一层的用例不该为了调一个工具去搭假网关。
pub(crate) async fn call_tool(
    state: &comfyhub_server_rs::web::routes::AppState,
    name: &str,
    arguments: serde_json::Value,
) -> Result<
    comfyhub_server_rs::tools::tools::ToolOutcome,
    comfyhub_server_rs::tools::tools::ToolFailure,
> {
    let policy = comfyhub_server_rs::tools::policy::ToolPolicy::load(state).unwrap();
    let mut session = comfyhub_server_rs::tools::tools::ToolSession::default();
    comfyhub_server_rs::tools::tools::execute(
        state,
        &policy,
        "run-x",
        "conv-x",
        name,
        &arguments,
        &mut session,
    )
    .await
}

// ===========================================================================
//  图像编辑三件（投放 + 遮罩）
//
//  判据是**真的落盘 + 真的按尺寸画**：投放上去的文件要在 ComfyUI 的 input 目录里、
//  内容与来源**逐字节相同**；遮罩要在对的像素上是白的、尺寸与底图一致。
//  为什么盯着这两条：ComfyUI 的 `LoadImage` / `LoadImageMask` 只认 input 目录里的
//  裸文件名，写错目录（安装形态 vs 共享目录）的表现是 `Invalid image file` ——
//  看似"文件不存在"，实际写入了另一个目录；而遮罩尺寸与底图不一致时采样直接失败。
// ===========================================================================

/// 造一张**真能解码**的 PNG（投放与遮罩两条链路都必须吃得下真图）。
pub(crate) fn write_test_png(path: &std::path::Path, w: u32, h: u32) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut img = image::RgbImage::new(w, h);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        *pixel = image::Rgb([(x % 251) as u8, (y % 241) as u8, 7]);
    }
    img.save(path).unwrap();
}

/// 往夹具里塞一条附件（盘上那份真图由调用方写）。
pub(crate) fn seed_attachment(
    state: &comfyhub_server_rs::web::routes::AppState,
    id: &str,
    name: &str,
    kind: &str,
) {
    let dto = comfyhub_server_rs::media::attachments::AttachmentDto {
        id: id.to_string(),
        name: name.to_string(),
        kind: kind.to_string(),
        modality: kind.to_string(),
        mime_type: if kind == "image" {
            "image/png".into()
        } else {
            "video/mp4".into()
        },
        size_bytes: 1024,
        width: None,
        height: None,
        sha256: None,
        status: "ready".to_string(),
        created_at: Some("2026-09-26T00:00:00.000Z".to_string()),
    };
    comfyhub_server_rs::media::attachments::insert(state, &dto, &format!("{id}.png"), "upload")
        .unwrap();
}

/// 把"当前生效的输出目录"钉到 `<root>/shared/output`（ComfyUI Desktop 那种
/// 程序与共享数据分家的布局），并在旁边建出 `input`。
pub(crate) fn configure_shared_output(
    state: &comfyhub_server_rs::web::routes::AppState,
    root: &std::path::Path,
) {
    let shared = root.join("shared");
    std::fs::create_dir_all(shared.join("input")).unwrap();
    std::fs::create_dir_all(shared.join("output")).unwrap();
    state
        .write(|tx| {
            tx.execute(
                "INSERT INTO app_settings (k, v) VALUES ('capture.config', ?1) \
                 ON CONFLICT(k) DO UPDATE SET v = excluded.v",
                rusqlite::params![serde_json::json!({
                    "enabled": false,
                    "comfyUrl": "http://127.0.0.1:9",
                    "outputDir": shared.join("output").display().to_string(),
                    "pollSeconds": 4,
                    "autoTag": "ComfyUI",
                    "maxPerPoll": 20,
                    "downloadFallback": true,
                })
                .to_string()],
            )?;
            Ok(())
        })
        .unwrap();
}

// ===========================================================================
//  /api/capture/{status,jobs} + comfy_get_status（P4 第 42 步）
//
//  用**真 socket** 的假 ComfyUI（不出网）：不这么做就只能测"连不上"那一支，
//  而"连上了、队列里有几个、队首叫什么"才是这个端点真正要回答的。
// ===========================================================================

/// 假 ComfyUI：只认 `/queue`，回一份队列（形状与真 ComfyUI 一致：
/// `[number, prompt_id, graph, extra_data, outputs]`，标题在
/// `extra_data.extra_pnginfo.workflow.extra.title`）。
pub(crate) async fn fake_comfy_queue(
    queue_json: &'static str,
) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                // 读干净再回（理由见 [`read_request`]）；这一档只看轮次/路径，不看内容
                let _ = read_request(&mut socket).await;
                let body = queue_json;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), handle)
}

pub(crate) fn queue_body() -> &'static str {
    // 运行中 1 条（带标题）、等待中 2 条
    r#"{"queue_running":[[1,"uuid-1",{},{"extra_pnginfo":{"workflow":{"extra":{"title":"krea2 出图"}}}} ,{}]],
        "queue_pending":[[2,"uuid-2",{},{},{}],[3,"uuid-3",{},{},{}]]}"#
}

// ===========================================================================
//  生成任务的**进度百分比 + 当前步骤**（P4 第 72 步）
//
//  这里喂进去的帧分两类，注释里逐条标明：
//   ★ 真帧 = 2026-09-27 在本机 ComfyUI 上抓到的原文（`ws://127.0.0.1:8188/ws`）；
//   ✎ 手写帧 = 本机那条路径没走到的形状（抄 ComfyUI 源码里 `send_sync` 的那一行）。
//  判据是"界面能被喂到的东西"：`current` + `submissions[].progress` 一起对。
// ===========================================================================

/// 那一次抓帧用的 `prompt_id`（真帧原文里的那个）。
pub(crate) const PROGRESS_PROMPT_ID: &str = "3cf2b8f7-babe-4a75-9b1c-4d120af3bc19";

/// ★ 真帧：`execution_start`（ComfyUI `execution.py:742`）。
pub(crate) const REAL_EXECUTION_START: &str = r#"{"type": "execution_start", "data": {"prompt_id": "3cf2b8f7-babe-4a75-9b1c-4d120af3bc19", "timestamp": 1790510532242}}"#;
/// ★ 真帧：`executing`（execution.py:496）—— 轮到节点 5 了。
pub(crate) const REAL_EXECUTING: &str = r#"{"type": "executing", "data": {"node": "5", "display_node": "5", "prompt_id": "3cf2b8f7-babe-4a75-9b1c-4d120af3bc19"}}"#;
/// ★ 真帧：`execution_success`（execution.py:824）。
pub(crate) const REAL_EXECUTION_SUCCESS: &str = r#"{"type": "execution_success", "data": {"prompt_id": "3cf2b8f7-babe-4a75-9b1c-4d120af3bc19", "timestamp": 1790510888972}}"#;

// ===========================================================================
//  捕获入库（P4 第 43 步第二半）：poll → 建提示词 → 产物进画廊
//
//  用**真 socket** 的假 ComfyUI（不出网）+ 真文件系统 + 真库：
//  这一层是"AI 说生成了、画廊里却没有"那个最难查的问题的最后一道防线。
// ===========================================================================

/// 一张真的小 PNG（缩略图与宽高探测那条链必须真能解码，不然等于没测）。
pub(crate) fn tiny_png(width: u32, height: u32) -> Vec<u8> {
    use image::{ImageBuffer, Rgb};
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(width, height, Rgb([10, 20, 30]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

/// 假 ComfyUI：认 `/history`、`/queue`、`/view` 三条（真 socket，不出网）。
pub(crate) async fn fake_comfy(
    history: &'static str,
    view_body: &'static [u8],
) -> (String, tokio::task::JoinHandle<()>) {
    fake_comfy_full(history, view_body, r#"{}"#).await
}

/// 同上，但 `/object_info` 也由我们控制（界面格式工作流的转换要用它）。
pub(crate) async fn fake_comfy_full(
    history: &'static str,
    view_body: &'static [u8],
    object_info: &'static str,
) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                // 读干净再回（理由见 [`read_request`]）—— 下面的分流读的是**请求行**
                let head = request_line_of(&read_request(&mut socket).await);
                if head.contains("/object_info") {
                    let body = object_info;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                } else if head.contains("/history") {
                    let body = history;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                } else if head.contains("/view") {
                    let mut response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: image/png\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        view_body.len()
                    )
                    .into_bytes();
                    response.extend_from_slice(view_body);
                    let _ = socket.write_all(&response).await;
                } else {
                    let body = r#"{"queue_running":[],"queue_pending":[]}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                }
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), handle)
}

/// 一条"跑完了、有一个 png 产物"的 /history 条目（API 图 + outputs + status）。
pub(crate) fn history_with_image(filename: &str, run_key: &str) -> String {
    serde_json::json!({
        run_key: {
            "prompt": {
                "3": {"class_type":"KSampler","inputs":{"seed":4242,"steps":12,"cfg":2.5,
                      "sampler_name":"euler","scheduler":"normal",
                      "positive":["6",0],"negative":["7",0],"latent_image":["5",0]}},
                "5": {"class_type":"EmptyLatentImage","inputs":{"width":768,"height":1024}},
                "6": {"class_type":"CLIPTextEncode","inputs":{"text":"1girl, ink painting"}},
                "7": {"class_type":"CLIPTextEncode","inputs":{"text":"lowres"}},
                "9": {"class_type":"SaveImage","inputs":{"filename_prefix":"krea2/t2i"}}
            },
            "outputs": {
                "9": {"images":[{"filename": filename, "subfolder":"", "type":"output"}]}
            },
            "status": {"completed": true, "status_str": "success"}
        }
    })
    .to_string()
}

// ===========================================================================
//  comfy_load_workflow（P4 第 44 步）：把一份本机工作流文件读进库
//
//  该问题的解法：收到一个 .json 路径时，不再返回
//  "我只能提交库里的 promptId"。幂等靠 `file:<sha256>`（与 /api/capture/workflows
//  的 inLibrary 判定同一把钥匙）。
// ===========================================================================

/// 一份 API 格式节点图（原样提交，**零转换、不需要 ComfyUI 在场**）。
pub(crate) fn api_workflow_file() -> String {
    serde_json::json!({
        "3": {"class_type":"KSampler","inputs":{"seed":11,"steps":9,"cfg":1.5,
              "sampler_name":"euler","scheduler":"normal",
              "positive":["6",0],"latent_image":["5",0]}},
        "5": {"class_type":"EmptyLatentImage","inputs":{"width":640,"height":960}},
        "6": {"class_type":"CLIPTextEncode","inputs":{"text":"api format prompt"}},
        "9": {"class_type":"SaveImage","inputs":{"filename_prefix":"api"}}
    })
    .to_string()
}

/// 一份**界面格式**工作流（nodes/links + 位置式 widgets_values）。
///
/// ⚠ 里面刻意放了两类"服务端根本没有这个类"的节点（对应工具描述里的两种出口）：
/// * `4 Weird Frontend Thing` —— 既不广播也不显示，**谁都改写不了**，只能如实报错；
/// * `5 PreviewImage` —— 纯显示节点，`emulateFrontend=true`（默认）时前端自己会丢掉它。
pub(crate) fn ui_workflow_file() -> String {
    serde_json::json!({
        "nodes": [
            {"id": 1, "type": "CLIPTextEncode", "widgets_values": ["ui format prompt"]},
            {"id": 2, "type": "EmptyLatentImage", "widgets_values": [512, 512, 1]},
            {"id": 3, "type": "KSampler",
             "inputs": [
               {"name":"positive","type":"CONDITIONING","link":1},
               {"name":"latent_image","type":"LATENT","link":2}],
             "widgets_values": [777, "fixed", 6, 2.0, "euler", "normal"]},
            {"id": 4, "type": "Weird Frontend Thing", "widgets_values": []},
            {"id": 5, "type": "PreviewImage",
             "inputs": [{"name":"images","type":"IMAGE","link":3}]}
        ],
        "links": [[1,1,0,3,0,"CONDITIONING"],[2,2,0,3,1,"LATENT"],[3,3,0,5,0,"IMAGE"]]
    })
    .to_string()
}

/// 界面格式转换需要的 `/object_info`（三个节点）。
pub(crate) fn object_info_json() -> String {
    serde_json::json!({
        "CLIPTextEncode": {"input": {"required": {"text": ["STRING"], "clip": ["CLIP"]}}},
        "EmptyLatentImage": {"input": {"required": {"width": ["INT"], "height": ["INT"], "batch_size": ["INT"]}}},
        "KSampler": {"input": {"required": {
            "model": ["MODEL"],
            "seed": ["INT", {"control_after_generate": true}],
            "steps": ["INT"], "cfg": ["FLOAT"],
            "sampler_name": [["euler"]], "scheduler": [["normal"]],
            "positive": ["CONDITIONING"], "negative": ["CONDITIONING"], "latent_image": ["LATENT"]
        }}}
    })
    .to_string()
}

/// 一份带**广播型纯前端节点**、并且**一个输出节点都没有**的界面格式工作流。
///
/// 两件事都只在开启「模拟前端行为」时才会发生：`14 Anything Everywhere` 把上游那一路
/// 填进空着的同类型输入、前端会自己挂一个 `SaveImage`。
pub(crate) fn ui_workflow_broadcast_file() -> String {
    serde_json::json!({
        "nodes": [
            {"id": 1, "type": "CheckpointLoaderSimple", "widgets_values": ["a.safetensors"]},
            {"id": 2, "type": "CLIPTextEncode",
             "inputs": [{"name":"clip","type":"CLIP","link":10}],
             "widgets_values": ["1girl"]},
            {"id": 3, "type": "KSampler",
             "inputs": [
               {"name":"model","type":"MODEL","link":11},
               {"name":"latent_image","type":"LATENT","link":12}],
             "widgets_values": [1, "fixed", 20, 7.0, "euler", "normal"]},
            {"id": 4, "type": "EmptyLatentImage", "widgets_values": [512, 512, 1]},
            {"id": 5, "type": "VAEDecode",
             "inputs": [{"name":"samples","type":"LATENT","link":13}]},
            {"id": 14, "type": "Anything Everywhere",
             "inputs": [{"name":"anything","link":14}],
             "properties": {"Input Type": "CONDITIONING"}}
        ],
        "links": [
            [10, 1, 1, 2, 0, "CLIP"],
            [11, 1, 0, 3, 0, "MODEL"],
            [12, 4, 0, 3, 3, "LATENT"],
            [13, 3, 0, 5, 0, "LATENT"],
            [14, 2, 0, 14, 0, "CONDITIONING"]
        ]
    })
    .to_string()
}

/// 上一条夹具要的 `/object_info`（**故意没有** `Anything Everywhere` —— 它正是靠
/// "查不到 + 名字在广播表里"才被当成纯前端节点的）。`VAEDecode` 声明了 `output: IMAGE`，
/// 用来验"补的那个 `SaveImage` 接在哪一路"。
pub(crate) fn object_info_for_broadcast() -> String {
    serde_json::json!({
        "CheckpointLoaderSimple": {"input": {"required": {"ckpt_name": [["a.safetensors"]]}}},
        "CLIPTextEncode": {"input": {"required": {"text": ["STRING"], "clip": ["CLIP"]}}},
        "EmptyLatentImage": {"input": {"required": {
            "width": ["INT"], "height": ["INT"], "batch_size": ["INT"]}}},
        "KSampler": {"input": {"required": {
            "model": ["MODEL"], "seed": ["INT", {"control_after_generate": true}],
            "steps": ["INT"], "cfg": ["FLOAT"],
            "sampler_name": [["euler"]], "scheduler": [["normal"]],
            "positive": ["CONDITIONING"], "negative": ["CONDITIONING"],
            "latent_image": ["LATENT"]}}},
        "VAEDecode": {"input": {"required": {"samples": ["LATENT"]}}, "output": ["IMAGE"]}
    })
    .to_string()
}

// ===========================================================================
//  体检两件（P4 第 62 步）：comfy_check_inputs / comfy_fit_workflow//
//  两件都是"只报不改"的诊断工具，判据全在**如实**两个字上：`check_inputs` 漏报一条，
//  模型会误认为图已齐；`fit_workflow` 报错时，会被引导去下载一个已在硬盘上的文件。
//  所以这里断言到**具体字段**，不是"整串包含"。
// ===========================================================================

/// 与 [`call_tool`] 同一层，但让用例自己拿 session —— `comfy_check_inputs` 要看
/// `session.has_image_attachment`（那个事实由执行器在 Run 开始时钉住）。
pub(crate) async fn call_tool_with_session(
    state: &comfyhub_server_rs::web::routes::AppState,
    name: &str,
    arguments: Value,
    session: &mut comfyhub_server_rs::tools::tools::ToolSession,
) -> Result<
    comfyhub_server_rs::tools::tools::ToolOutcome,
    comfyhub_server_rs::tools::tools::ToolFailure,
> {
    let policy = comfyhub_server_rs::tools::policy::ToolPolicy::load(state).unwrap();
    comfyhub_server_rs::tools::tools::execute(
        state, &policy, "run-x", "conv-x", name, &arguments, session,
    )
    .await
}

/// 一份只有"缺一张参考图"的工作流：`LoadImage.image` 绑着上次捕获的旧文件名
/// （该情形下模型若不确认就会提交一个不存在的文件）。
pub(crate) fn graph_with_image_gap() -> Value {
    serde_json::json!({
        "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "sdxl.safetensors"}},
        "89": {"class_type": "LoadImage", "inputs": {"image": "4789521234.jpg"}},
        "3": {"class_type": "KSampler", "inputs": {"seed": 1}},
        "9": {"class_type": "SaveImage", "inputs": {"images": ["3", 0]}}
    })
}

/// 假 ComfyUI 的 `/object_info`：只声明一个 checkpoint 枚举 + 一个普通采样节点。
pub(crate) const FIT_OBJECT_INFO: &str = r#"{
  "CheckpointLoaderSimple": {"input": {"required": {"ckpt_name": [["krea2_turbo_int8.safetensors"], {"tooltip": "选一个底模"}]}}},
  "KSampler": {"input": {"required": {"steps": ["INT", {"default": 20}], "sampler_name": [["euler"]]}}}
}"#;

// ---------------------------------------------------------------------------
//  PUT /api/ai/credentials（密钥上行：手机改桌面上的 key，docs/pairing-design.md §14）
// ---------------------------------------------------------------------------

/// 闸门开着 + 一台已配对设备，两个凭据开关**分开**给（§14.1）。
pub(crate) fn seed_auth_device(
    state: &comfyhub_server_rs::web::routes::AppState,
    allow_read: bool,
    allow_upload: bool,
) {
    let file = serde_json::json!({
        "schema": "comfyhub-inbound-auth/2",
        "enabled": true,
        "token": "global-token-43chars-aaaaaaaaaaaaaaaaaaaaaa",
        "createdAt": "2026-09-24T00:00:00Z",
        "devices": [{
            "id": "phone-up",
            "name": "夹具手机",
            "token": DEVICE_TOKEN,
            "pairedAt": "2026-09-24T00:00:00Z",
            "allowCredentials": allow_read,
            "allowCredentialsUpload": allow_upload
        }]
    });
    put_setting(state, "auth.pairing", &file.to_string());
}

pub(crate) const DEVICE_TOKEN: &str = "device-token-43chars-bbbbbbbbbbbbbbbbbbbbbb";

/// 带配对令牌的 JSON 请求（凭据端点那条闸门只认"已配对 + 已授权"的设备）。
pub(crate) async fn send_json_token(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: serde_json::Value,
    token: &str,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-comfyhub-token", token)
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.expect("请求");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}
