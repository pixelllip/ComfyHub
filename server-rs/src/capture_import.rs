//! 捕获的两条**推送 / 批量**入口（P4 第 49 步）。
//!
//! | 端点 | Kotlin | 这里 |
//! | --- | --- | --- |
//! | `POST /api/ingest/comfyui` | `CaptureRoutes` 的 `post("/ingest/comfyui")` → `ComfyCapture.ingest` | [`ingest_json`] |
//! | `POST /api/capture/import-workflows` | `ComfyWorkflowFiles.importAll` | [`import_workflows`] |
//!
//! # 为什么这两条要一起做
//!
//! 它们是**"库里怎么才会有东西"**的另一半：轮询与 `comfy_submit` 收的是"跑过的运行"，
//! 而这两条收的是**外部推过来的运行**（ComfyUI 自定义节点 / e2e 脚本）与
//! **用户早就存在盘上的工作流文件**（用户 bug ⑤：首次使用时库必然是空的，而
//! `user\<用户>\workflows\*.json` 早就存在）。
//!
//! # 三条纪律
//!
//!  1. **`ingest` 与轮询走同一条路**：`begin_run` → `ingest_claimed`。抢不到就**如实**回
//!     `alreadyCaptured` + 已有的 `promptId`（绝不重复入库）；幂等靠 `run_key` + 文件 sha256。
//!  2. **批量导入按 `run_key = file:<sha256>` 判重**（与 `comfy_load_workflow` 同一把钥匙），
//!     所以"导两次"只有一行；`tolerateUnsupported = true` —— 批量时"转换不了"绝不能让整份工作流
//!     消失（它照样是用户库里的一份资产，能不能跑由 `runnable` 如实标出来）。
//!  3. **逐条结果如实分开**：`imported` / `duplicates` / `failed` 三个计数 +
//!     每条一个 `{name, path, status, promptId?, runnable?, reason?}` —— 失败**不许静默跳过**。

use serde_json::{json, Map, Value};

use crate::capture::{capture_config, CaptureConfig};
use crate::routes::{ApiError, AppState};

/// 批量导入的上限（Kotlin `ComfyWorkflowFiles.MAX_FILES`）。
pub const MAX_FILES: i64 = 200;

/// `POST /api/ingest/comfyui`：外部把一条**已经跑完**的运行推过来。
///
/// 请求体就是 Kotlin 的 `IngestRequest`（键是驼峰）：
/// `{runKey, source?, status?, error?, comfyUrl?, outputDir?, prompt?, workflow?, outputs[],
///   extra:{nsfw?}, title?, notes?, tags[]?, raw?}`。
pub async fn ingest_json(state: &AppState, body: &Value) -> Result<Value, ApiError> {
    let run_key = body.get("runKey").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if run_key.is_empty() {
        return Err(ApiError::bad_request("runKey 不能为空"));
    }
    let config = capture_config(state)?;

    // Kotlin `ComfyCapture.ingest`：source 空就当 ComfyUI；`raw` 原样落进 capture_runs.raw
    let source = body
        .get("source")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("ComfyUI")
        .to_string();
    let raw = body.get("raw").cloned().unwrap_or_else(|| body.clone());
    let claim = crate::capture_ingest::begin_run(state, &run_key, &source, Some(&raw), false)?;
    if !claim.claimed {
        return Ok(json!({
            "runKey": run_key,
            "promptId": claim.existing_prompt_id,
            "created": false,
            "mediaIds": [],
            "imported": 0,
            "duplicates": 0,
            "failed": 0,
            "alreadyCaptured": true,
            "message": "这次运行已经捕获过了",
        }));
    }

    let request = request_from_body(body, &run_key, &source, &config)?;
    let result = crate::capture_ingest::ingest_claimed(state, &request, &config).await;
    Ok(result.to_json())
}

/// 把推送过来的那份 JSON 映射成内部的 [`crate::capture_ingest::IngestRequest`]。
///
/// ⚠ 键名是 **Kotlin 的驼峰**（`outputDir` / `type` / `nodeId` / `extra.nsfw`），
/// 内部结构体是 snake_case —— 这层映射就是"两端形状"的唯一接触点，写错了不会报错，
/// 只会"产物一条都收不进来"。
fn request_from_body(
    body: &Value,
    run_key: &str,
    source: &str,
    config: &CaptureConfig,
) -> Result<crate::capture_ingest::IngestRequest, ApiError> {
    let outputs: Vec<crate::capture_ingest::IngestOutput> = body
        .get("outputs")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| crate::capture_ingest::IngestOutput {
                    filename: item.get("filename").and_then(Value::as_str).unwrap_or("").to_string(),
                    subfolder: item.get("subfolder").and_then(Value::as_str).map(|s| s.to_string()),
                    type_field: item.get("type").and_then(Value::as_str).map(|s| s.to_string()),
                    kind: item.get("kind").and_then(Value::as_str).map(|s| s.to_string()),
                    node_id: item
                        .get("nodeId")
                        .map(primitive_text)
                        .filter(|s| !s.is_empty()),
                    node_type: item.get("nodeType").and_then(Value::as_str).map(|s| s.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();

    let tags = body
        .get("tags")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(|s| s.to_string()).collect())
        .unwrap_or_default();

    // `extra.nsfw`（Kotlin 把它放在 `extra` 里；也有老客户端直接给顶层 `nsfw`）
    let nsfw = body
        .get("extra")
        .and_then(|extra| extra.get("nsfw"))
        .and_then(Value::as_bool)
        .or_else(|| body.get("nsfw").and_then(Value::as_bool))
        .unwrap_or(false);

    Ok(crate::capture_ingest::IngestRequest {
        run_key: run_key.to_string(),
        source: source.to_string(),
        status: body.get("status").and_then(Value::as_str).unwrap_or("").to_string(),
        error: body.get("error").and_then(Value::as_str).map(|s| s.to_string()),
        comfy_url: body
            .get("comfyUrl")
            .and_then(Value::as_str)
            .map(|s| s.to_string())
            .or_else(|| Some(config.comfy_url.clone())),
        output_dir: body
            .get("outputDir")
            .and_then(Value::as_str)
            .map(|s| s.to_string())
            .or_else(|| config.output_dir.clone()),
        prompt: body.get("prompt").cloned(),
        workflow: body.get("workflow").cloned(),
        outputs,
        nsfw,
        title: body.get("title").and_then(Value::as_str).map(|s| s.to_string()),
        notes: body.get("notes").and_then(Value::as_str).map(|s| s.to_string()),
        tags,
        raw: body.get("raw").cloned(),
    })
}

fn primitive_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

/// `POST /api/capture/import-workflows`：把本机 ComfyUI 里已保存的工作流文件**批量读进库**。
///
/// 请求体 `{dir?, limit?, query?}`（Kotlin `ImportWorkflowsRequest`）：
/// `dir` 不填就自动发现（[`crate::comfy_dirs::workflow_dirs`]），填了就当它是那个目录本身。
pub async fn import_workflows(state: &AppState, body: &Value) -> Result<Value, ApiError> {
    let project_root = state.project_root();
    let configured_output = capture_config(state).ok().and_then(|config| config.output_dir);

    let dirs: Vec<std::path::PathBuf> = match body.get("dir").and_then(Value::as_str).map(str::trim) {
        Some(raw) if !raw.is_empty() => {
            let path = std::path::PathBuf::from(raw);
            if !path.is_dir() {
                return Err(ApiError::bad_request(format!("目录不存在：{raw}")));
            }
            vec![path]
        }
        _ => crate::comfy_dirs::workflow_dirs(&project_root, configured_output.as_deref()),
    };

    let limit = body
        .get("limit")
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
        .unwrap_or(MAX_FILES)
        .clamp(1, MAX_FILES) as usize;
    let query = body
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty());

    // ⚠ 只扫**能用的**（坏 JSON / 空 / 非 .json / 格式认不出来的都不列）：
    // 它们 `comfy_load_workflow` 一定失败，收进来只会让用户看见一堆"失败"
    let files = crate::comfy_dirs::scan(&dirs, query, limit);

    let mut imported = 0i64;
    let mut duplicates = 0i64;
    let mut failed = 0i64;
    let mut prompt_ids: Vec<i64> = Vec::new();
    let mut items: Vec<Value> = Vec::new();

    for file in files.iter() {
        let run_key = format!("file:{}", file.sha256);
        // 幂等：库里已经有这份文件（同一把钥匙 `file:<sha256>`）→ 算"已在库"，不重复导入
        match crate::capture::find_run(state, &run_key) {
            Ok(Some(existing)) if existing.prompt_id.is_some() => {
                duplicates += 1;
                items.push(json!({
                    "name": file.name,
                    "path": file.path,
                    "status": "duplicate",
                    "promptId": existing.prompt_id,
                }));
                continue;
            }
            _ => {}
        }
        let path = std::path::PathBuf::from(&file.path);
        match crate::workflow_load::load_from_file(state, &path, None, false, true, true).await {
            Ok(loaded) => {
                let prompt_id = loaded.get("promptId").and_then(|v| v.as_i64());
                let runnable = loaded.get("runnable").and_then(Value::as_bool);
                imported += 1;
                if let Some(id) = prompt_id {
                    prompt_ids.push(id);
                }
                let mut item = Map::new();
                item.insert("name".into(), json!(file.name));
                item.insert("path".into(), json!(file.path));
                item.insert("status".into(), json!("imported"));
                if let Some(id) = prompt_id {
                    item.insert("promptId".into(), json!(id));
                }
                if let Some(runnable) = runnable {
                    item.insert("runnable".into(), json!(runnable));
                    if !runnable {
                        item.insert(
                            "reason".into(),
                            json!("转换时摘掉了纯前端节点，缺口见 comfy_load_workflow 的 openInputs / unresolvedInputs"),
                        );
                    }
                }
                items.push(Value::Object(item));
            }
            Err(failure) => {
                failed += 1;
                items.push(json!({
                    "name": file.name,
                    "path": file.path,
                    "status": "failed",
                    "reason": format!("{}: {}", failure.code, take_chars(&failure.message, 200)),
                }));
            }
        }
    }

    let message = if dirs.is_empty() {
        "没找到本机 ComfyUI 的 workflows 目录（`user\\<用户>\\workflows`）—— \
         先在「设置 → ComfyUI 自动捕获」里确认 ComfyUI 的位置。"
            .to_string()
    } else if files.is_empty() {
        format!(
            "这些目录里没有工作流文件（.json）：{}",
            dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join("、")
        )
    } else {
        format!(
            "扫描 {} 份文件：新导入 {imported}，已在库里 {duplicates}，失败 {failed}",
            files.len()
        )
    };

    Ok(json!({
        "dirs": dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
        "scanned": files.len(),
        "imported": imported,
        "duplicates": duplicates,
        "failed": failed,
        "promptIds": prompt_ids,
        "items": items,
        "message": message,
    }))
}

fn take_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
            std::env::temp_dir(),
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
            std::env::temp_dir(),
        );
        let error = import_workflows(&state, &json!({ "dir": "D:\\definitely-not-here" }))
            .await
            .expect_err("目录不存在");
        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(error.human().contains("目录不存在"), "{}", error.human());
    }
}
