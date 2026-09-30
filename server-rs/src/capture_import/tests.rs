//! `capture_import` 的单元测试（主体代码在 [`capture_import`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;
use crate::capture::CaptureConfig;

fn config() -> CaptureConfig {
    CaptureConfig {
        enabled: true,
        comfy_url: "http://127.0.0.1:8188".to_string(),
        output_dir: Some("D:\\out".to_string()),
        poll_seconds: 4,
        auto_tag: "ComfyUI".to_string(),
        max_per_poll: 20,
        download_fallback: true,
    }
}

#[test]
fn 推送体的键名映射一条都不能错() {
    let body = json!({
        "runKey": "push-1",
        "source": "ComfyUI",
        "status": "success",
        "outputs": [
            { "filename": "a.png", "subfolder": "sub", "type": "output", "nodeId": 9, "nodeType": "SaveImage" }
        ],
        "extra": { "nsfw": true },
        "title": "推来的",
        "tags": ["推送"],
        "prompt": { "1": { "class_type": "KSampler", "inputs": {} } }
    });
    let request = request_from_body(&body, "push-1", "ComfyUI", &config()).unwrap();
    assert_eq!(request.run_key, "push-1");
    assert_eq!(request.outputs.len(), 1);
    assert_eq!(request.outputs[0].filename, "a.png");
    assert_eq!(request.outputs[0].subfolder.as_deref(), Some("sub"));
    assert_eq!(request.outputs[0].type_field.as_deref(), Some("output"));
    assert_eq!(request.outputs[0].node_id.as_deref(), Some("9"), "数字 id 也要读出来");
    assert_eq!(request.outputs[0].node_type.as_deref(), Some("SaveImage"));
    assert!(request.nsfw, "`extra.nsfw` 必须映射到 nsfw");
    assert_eq!(request.title.as_deref(), Some("推来的"));
    assert_eq!(request.tags, vec!["推送".to_string()]);
    // 没给的走配置兜底（Kotlin 同）
    assert_eq!(request.comfy_url.as_deref(), Some("http://127.0.0.1:8188"));
    assert_eq!(request.output_dir.as_deref(), Some("D:\\out"));
    assert!(request.prompt.is_some());
}

#[test]
fn 顶层_nsfw_也认且默认关闭() {
    let body = json!({ "runKey": "push-2", "nsfw": true });
    assert!(request_from_body(&body, "push-2", "ComfyUI", &config()).unwrap().nsfw);
    let body = json!({ "runKey": "push-3" });
    assert!(!request_from_body(&body, "push-3", "ComfyUI", &config()).unwrap().nsfw);
}

#[tokio::test]
async fn 推送_缺_run_key_要报人话() {
    let state = crate::routes::AppState::new(
        crate::db::open_memory().unwrap(),
        ":memory:",
        crate::testpaths::scratch("media"),
    );
    let error = ingest_json(&state, &json!({ "outputs": [] })).await.expect_err("缺 runKey");
    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(error.human(), "runKey 不能为空");
}

#[tokio::test]
async fn 批量导入_目录不存在要报人话() {
    let state = crate::routes::AppState::new(
        crate::db::open_memory().unwrap(),
        ":memory:",
        crate::testpaths::scratch("media"),
    );
    let error = import_workflows(&state, &json!({ "dir": "D:\\definitely-not-here" }))
        .await
        .expect_err("目录不存在");
    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
    assert!(error.human().contains("目录不存在"), "{}", error.human());
}
