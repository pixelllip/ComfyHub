//! **附件（R9 / AIH-027~031）的回归防线** —— 五条端点 + 三层准入里"最要紧的那一层"。
//!
//! # 这一组在防什么
//!
//! 总纲 §6 的 R9 有三层准入：① 选完预检 → ② 发送前前端禁用按钮 → ③ **创建 Run 之前后端
//! 用库里的事实再验一次**。前两层在界面上，抓不住；**第三层是唯一能用用例钉住的**，也是
//! `e2e-ai-tools-test.ps1` 第 9 幕要证的那件事：
//!
//! * 不通过 → **`400 UNSUPPORTED_CONTENT`**，而且**上游请求数为 0**（在 append 用户消息之前返回）；
//! * 通过之后**不许静默丢掉**：这一版适配器还不能把附件内联进请求体，所以必须在那一轮正文后面
//!   附一句"以下附件未随本次请求发送：…"（AIH-030 的"看起来发出去了、其实没有"）。
//!
//! 另外两条形状上的事实也在这里钉住：
//! ① 上传**只认签名**（声称 image/png 而内容是二进制 → 400 + 人话）；
//! ② 附件块要真的落进用户消息的有序块里（少了它界面画不出缩略图、执行器也不知道带了图）。

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use rusqlite::Connection;
use serde_json::{json, Value};
use tower::ServiceExt;

use comfyhub_server_rs::routes::{router, AppState};

/// 用 `db/schema.sqlite.sql`（**唯一真源**）建库。
fn state(label: &str) -> (AppState, std::path::PathBuf) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("comfyhub-r9-{label}-{nanos}"));
    std::fs::create_dir_all(dir.join("storage").join("media")).unwrap();

    let conn = Connection::open_in_memory().unwrap();
    let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("db")
        .join("schema.sqlite.sql");
    let schema = std::fs::read_to_string(&schema_path).expect("读 schema");
    conn.execute_batch(&schema).expect("建 schema");

    let state = AppState::new(conn, ":memory:", dir.join("storage").join("media"));
    (state, dir)
}

/// 一条会话 + 一个 provider + 一个模型（`modalities` 决定它能不能吃图）。
fn seed(state: &AppState, modalities: &str) {
    state
        .write(|tx| {
            tx.execute(
                "INSERT INTO ai_conversations (id, title, created_at, updated_at) \
                 VALUES ('conv-1', '附件用例', '2026-09-26T00:00:00.000Z', '2026-09-26T00:00:00.000Z')",
                [],
            )?;
            tx.execute(
                "INSERT INTO ai_providers (id, display_name, api, base_url, credential_ref, \
                   endpoint_trust, enabled, revision) \
                 VALUES ('goat', '山羊', 'openai-completions', 'http://127.0.0.1:9/v1', NULL, 'loopback', 1, 1)",
                [],
            )?;
            tx.execute(
                "INSERT INTO ai_models (provider_id, model_id, display_name, tools, reasoning, \
                   input_modalities, attachment_transports, mime_allowlist, parallel_tools, \
                   thinking_efforts, capability_source, enabled) \
                 VALUES ('goat', 'm1', '模型一', 0, 0, ?1, '{}', '[]', 0, '[]', 'manual', 1)",
                rusqlite::params![modalities],
            )?;
            Ok(())
        })
        .unwrap();
}

fn point_at(state: &AppState, base_url: &str) {
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

async fn send_json(state: &AppState, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
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

async fn get_json(state: &AppState, path: &str) -> (StatusCode, Value) {
    let request = Request::builder().method("GET").uri(path).body(Body::empty()).unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
    (status, body)
}

/// 手工拼一个 multipart 请求（字段名 `files`，与 Dart 客户端一致）。
async fn upload(state: &AppState, files: &[(&str, &str, Vec<u8>)]) -> (StatusCode, Value) {
    const BOUNDARY: &str = "----comfyhubtestboundary";
    let mut body: Vec<u8> = Vec::new();
    for (field, filename, bytes) in files {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{field}\"; filename=\"{filename}\"\r\n").as_bytes(),
        );
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());

    let request = Request::builder()
        .method("POST")
        .uri("/api/ai/attachments")
        .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body))
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
    (status, body)
}

/// 一张**真 PNG**（入库那条路要走图片探测器）。
fn png_bytes() -> Vec<u8> {
    let image = image::RgbImage::from_fn(7, 5, |x, _| image::Rgb([(x * 30) as u8, 90, 200]));
    let mut buffer = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut buffer, image::ImageFormat::Png)
        .unwrap();
    buffer.into_inner()
}

/// 一台**记录请求体**的假网关：返回一段最小 SSE（让 Run 能收尾），并把请求体留给我们断言。
async fn recording_gateway() -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let store = seen.clone();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            let store = store.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 256 * 1024];
                let Ok(n) = socket.read(&mut buf).await else { return };
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                store.lock().unwrap().push(request);
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
                    .await;
                for frame in [
                    r#"{"choices":[{"delta":{"content":"好的"}}]}"#,
                    r#"{"choices":[{"finish_reason":"stop"}]}"#,
                    r#"[DONE]"#,
                ] {
                    let payload = format!("data: {frame}\n\n");
                    if socket.write_all(payload.as_bytes()).await.is_err() {
                        return;
                    }
                }
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), seen, handle)
}

#[tokio::test]
async fn 上传只认签名_谎报类型一律拒收() {
    let (state, dir) = state("upload");
    // ① 声称 image/png、内容是二进制 → 400 + 人话
    let (status, body) = upload(&state, &[("files", "evil.png", vec![0, 1, 2, 3, 255, 254])]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], json!("UNSUPPORTED_CONTENT"));
    assert!(
        body["detail"].as_str().unwrap().contains("无法识别文件 evil.png"),
        "{body}"
    );

    // ② 真 PNG → 201，尺寸 / kind / sha256 都对
    let (status, body) = upload(&state, &[("files", "参考图.png", png_bytes())]).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let item = &body["items"][0];
    assert_eq!(item["kind"], json!("image"));
    assert_eq!(item["modality"], json!("image"));
    assert_eq!(item["mimeType"], json!("image/png"));
    assert_eq!((item["width"].as_i64(), item["height"].as_i64()), (Some(7), Some(5)));
    assert!(item["sha256"].as_str().map(|s| s.len() == 64).unwrap_or(false), "{item}");
    // 可空键不出现（Kotlin `explicitNulls = false`）
    assert!(item.as_object().unwrap().get("hasPreview").is_none(), "{item}");
    let id = item["id"].as_str().unwrap().to_string();

    // ③ 缩略图：图片给 JPEG，带 `X-Thumbnail: 1`
    let request = Request::builder()
        .uri(format!("/api/ai/attachments/{id}/thumb"))
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("x-thumbnail").unwrap(), "1");
    assert_eq!(response.headers().get("content-type").unwrap(), "image/jpeg");

    // ④ 不存在的附件：400 + `CONFIG_ERROR` + detail（与 Kotlin 的 AiException 同一个信封）
    let (status, body) = get_json(&state, "/api/ai/attachments/nope").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], json!("CONFIG_ERROR"));
    assert!(body["detail"].as_str().unwrap().contains("附件不存在"), "{body}");

    // ⑤ 文本附件没有封面 → 204（界面退化成文件图标）
    let (status, body) = upload(&state, &[("files", "notes.md", b"# hello\nworld\n".to_vec())]).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let text_id = body["items"][0]["id"].as_str().unwrap().to_string();
    let request = Request::builder()
        .uri(format!("/api/ai/attachments/{text_id}/thumb"))
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn 预检_逐个附件给结论且顺序与请求一致() {
    let (state, dir) = state("preflight");
    seed(&state, "[\"text\"]");
    let (_, body) = upload(&state, &[("files", "a.png", png_bytes())]).await;
    let id = body["items"][0]["id"].as_str().unwrap().to_string();

    let (status, body) = send_json(
        &state,
        "POST",
        "/api/ai/preflight",
        json!({
            "providerId": "goat",
            "modelId": "m1",
            "attachmentIds": [id, "不存在"],
            // 老用法：前端给线索 + 文件头（谎报 image/png，内容是 PDF）
            "attachments": [
                { "name": "p.pdf", "modality": "image", "mimeType": "image/png", "sizeBytes": 10,
                  "headBase64": "JVBERi0xLjc=" }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["allowed"], json!(false));
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "数量与请求一一对应：{body}");
    assert_eq!(items[0]["attachmentId"], json!(id));
    assert_eq!(items[1]["name"], json!("不存在"));
    assert!(
        items[1]["blockers"][0].as_str().unwrap().contains("附件不存在或已被删除"),
        "{body}"
    );
    // 第三个：按签名判成文档（前端说 image 不算数），而模型不支持文档 → 阻断
    assert_eq!(items[2]["name"], json!("p.pdf"));
    let third = items[2]["blockers"].as_array().unwrap();
    assert!(
        third.iter().any(|b| b.as_str().unwrap().contains("未声明支持文档输入")),
        "{body}"
    );
    // 顶层 blockers 是逐条的汇总
    assert!(!body["blockers"].as_array().unwrap().is_empty());

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn 创建_run_之前拦住不支持的附件_上游零请求() {
    let (state, dir) = state("gate-block");
    seed(&state, "[\"text\"]"); // 模型只吃文本
    let (base_url, seen, gateway) = recording_gateway().await;
    point_at(&state, &base_url);

    let (_, body) = upload(&state, &[("files", "a.png", png_bytes())]).await;
    let id = body["items"][0]["id"].as_str().unwrap().to_string();

    let (status, body) = send_json(
        &state,
        "POST",
        "/api/ai/conversations/conv-1/runs",
        json!({ "text": "看看这张图", "providerId": "goat", "modelId": "m1", "attachmentIds": [id] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], json!("UNSUPPORTED_CONTENT"));
    let detail = body["detail"].as_str().unwrap();
    assert!(detail.contains("有附件未通过准入"), "{detail}");
    assert!(detail.contains("未产生任何上游请求"), "{detail}");
    assert!(detail.contains("未声明支持图片输入"), "{detail}");

    // **R9 的核心断言**：一个字节都没发出去
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(seen.lock().unwrap().is_empty(), "预检拦下时不许有任何上游请求");
    // 用户消息也不该落库（在 append 之前就返回了）
    let count: i64 = state
        .with(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM ai_messages", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(count, 0, "被拦下时不该往会话里塞消息");

    gateway.abort();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn 支持的图片会被真的内联_发不出去的必须如实告知() {
    let (state, dir) = state("gate-allow");
    seed(&state, "[\"text\",\"image\"]"); // 这个模型吃图
    let (base_url, seen, gateway) = recording_gateway().await;
    point_at(&state, &base_url);

    let (_, body) = upload(&state, &[("files", "参考图.png", png_bytes())]).await;
    let id = body["items"][0]["id"].as_str().unwrap().to_string();
    // 顺带传一个**文本**附件：文案类附件模型/协议都还没有传输方式实现 → 只能如实说没发出去
    let (_, body) = upload(&state, &[("files", "notes.md", b"# hello\n".to_vec())]).await;
    let text_id = body["items"][0]["id"].as_str().unwrap().to_string();

    let (status, body) = send_json(
        &state,
        "POST",
        "/api/ai/conversations/conv-1/runs",
        json!({
            "text": "看看这张图",
            "providerId": "goat",
            "modelId": "m1",
            "attachmentIds": [id, text_id]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let user_message_id = body["userMessageId"].as_str().unwrap().to_string();

    // ① 两个附件块都真的落进了用户消息（界面靠它画缩略图）
    let parts: Vec<(String, Option<String>)> = state
        .with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT type, attachment_id FROM ai_message_parts WHERE message_id = ?1 ORDER BY ordinal",
            )?;
            let rows = stmt.query_map(rusqlite::params![user_message_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .unwrap();
    assert_eq!(parts.iter().filter(|p| p.0 == "attachment").count(), 2, "{parts:?}");

    // ② 发出去的请求体：图片是**真的内联块**，文本附件在"未随本次请求发送"里
    let mut request = String::new();
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let seen = seen.lock().unwrap();
        if let Some(found) = seen.first() {
            request = found.clone();
            break;
        }
    }
    assert!(!request.is_empty(), "应当真的发出了一次上游请求");
    assert!(request.contains("\"type\":\"image_url\""), "图片要变成内联块：{request}");
    assert!(request.contains("data:image/png;base64,"), "要带 data URL：{request}");
    assert!(request.contains("data:image/png;base64,iVBORw0KGgo"), "PNG 的 base64 头要接在 data URL 后面：{request}");
    assert!(request.contains("以下附件未随本次请求发送"), "文本附件要如实告知：{request}");
    assert!(request.contains("notes.md"), "{request}");
    assert!(!request.contains("参考图.png（id="), "发出了的图片**不该**出现在未发送名单里：{request}");
    // ③ 系统提示里那句"附带图片附件"要与事实一致（内联了才说有）
    assert!(request.contains("本次请求附带图片附件: 有"), "{request}");

    gateway.abort();
    let _ = std::fs::remove_dir_all(dir);
}
