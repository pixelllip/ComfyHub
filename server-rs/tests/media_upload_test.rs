//! `/api/media/upload` 的请求体上限回归（2026-11-13）。
//!
//! 上传 3MB 的视频：它超过 axum 默认的 2MB 请求体上限，但**没有**超过
//! `/api/media/upload` 自己的上限。修之前这条会 413（客户端表现为
//! "Write failed" —— 服务端在请求体还没写完时就把连接关了），修之后必须是 201。
//!
//! 为什么用 3MB 而不是刚好 2MB+1：`DefaultBodyLimit` 算的是整个 multipart 体，
//! 3MB 留出足够的余量，避免边界值随 boundary/字段长度漂移。

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use rusqlite::Connection;
use serde_json::Value;
use tower::ServiceExt;

use comfyhub_server_rs::web::routes::{router, AppState};

/// 用 `db/schema.sqlite.sql`（**唯一真源**）建库；落盘目录走 `testpaths::scratch`。
fn state(label: &str) -> (AppState, PathBuf) {
    let dir = comfyhub_server_rs::infra::testpaths::scratch(&format!("media-upload-{label}"));
    std::fs::create_dir_all(dir.join("storage").join("media")).unwrap();

    let conn = Connection::open_in_memory().unwrap();
    let schema_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("db")
        .join("schema.sqlite.sql");
    let schema = std::fs::read_to_string(&schema_path).expect("读 schema");
    conn.execute_batch(&schema).expect("建 schema");

    let state = AppState::new(conn, ":memory:", dir.join("storage").join("media"));
    (state, dir)
}

/// 手工拼 multipart（字段名 `files`，与 Dart 客户端一致）。
async fn upload_media(state: AppState, file_name: &str, bytes: Vec<u8>) -> (StatusCode, Value) {
    const BOUNDARY: &str = "----comfyhubmediaupload";
    let expected = bytes.len();
    let mut body: Vec<u8> = Vec::with_capacity(expected + 256);
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"files\"; filename=\"{file_name}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(&bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());

    let request = Request::builder()
        .method("POST")
        .uri("/api/media/upload")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap();
    let response = router(state).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn a_three_megabyte_video_is_accepted() {
    let (state, dir) = state("three-mb");
    let bytes = vec![0x5Au8; 3 * 1024 * 1024];
    let (status, body) = upload_media(state, "clip.mp4", bytes).await;

    assert_eq!(
        status,
        StatusCode::CREATED,
        "3MB 的上传必须能过（axum 默认 2MB 上限不能落在这条路由上）：{body}"
    );
    assert_eq!(body["items"][0]["sizeBytes"], 3 * 1024 * 1024);
    assert_eq!(body["items"][0]["kind"], "VIDEO");

    // 原件必须真的落盘，不是只写了库。
    let stored = body["items"][0]["storedName"]
        .as_str()
        .expect("响应里要有 storedName");
    let path = dir.join("storage").join("media").join(stored);
    assert!(path.exists(), "原件没落盘：{}", path.display());
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        3 * 1024 * 1024,
        "落盘大小必须等于上传大小"
    );
}
