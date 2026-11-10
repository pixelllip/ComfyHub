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

use crate::comfy::capture::{capture_config, CaptureConfig};
use crate::web::routes::{ApiError, AppState};

/// 批量导入的上限（Kotlin `ComfyWorkflowFiles.MAX_FILES`）。
pub const MAX_FILES: i64 = 200;

/// `POST /api/ingest/comfyui`：外部把一条**已经跑完**的运行推过来。
///
/// 请求体就是 Kotlin 的 `IngestRequest`（键是驼峰）：
/// `{runKey, source?, status?, error?, comfyUrl?, outputDir?, prompt?, workflow?, outputs[],
///   extra:{nsfw?}, title?, notes?, tags[]?, raw?}`。
pub async fn ingest_json(state: &AppState, body: &Value) -> Result<Value, ApiError> {
    let run_key = body
        .get("runKey")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
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
    let claim =
        crate::comfy::capture_ingest::begin_run(state, &run_key, &source, Some(&raw), false)?;
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
    let result = crate::comfy::capture_ingest::ingest_claimed(state, &request, &config).await;
    Ok(result.to_json())
}

/// 把推送过来的那份 JSON 映射成内部的 [`crate::comfy::capture_ingest::IngestRequest`]。
///
/// ⚠ 键名是 **Kotlin 的驼峰**（`outputDir` / `type` / `nodeId` / `extra.nsfw`），
/// 内部结构体是 snake_case —— 这层映射就是"两端形状"的唯一接触点，写错了不会报错，
/// 只会"产物一条都收不进来"。
fn request_from_body(
    body: &Value,
    run_key: &str,
    source: &str,
    config: &CaptureConfig,
) -> Result<crate::comfy::capture_ingest::IngestRequest, ApiError> {
    let outputs: Vec<crate::comfy::capture_ingest::IngestOutput> = body
        .get("outputs")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| crate::comfy::capture_ingest::IngestOutput {
                    filename: item
                        .get("filename")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    subfolder: item
                        .get("subfolder")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string()),
                    type_field: item
                        .get("type")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string()),
                    kind: item
                        .get("kind")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string()),
                    node_id: item
                        .get("nodeId")
                        .map(primitive_text)
                        .filter(|s| !s.is_empty()),
                    node_type: item
                        .get("nodeType")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();

    let tags = body
        .get("tags")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    // `extra.nsfw`（Kotlin 把它放在 `extra` 里；也有老客户端直接给顶层 `nsfw`）
    let nsfw = body
        .get("extra")
        .and_then(|extra| extra.get("nsfw"))
        .and_then(Value::as_bool)
        .or_else(|| body.get("nsfw").and_then(Value::as_bool))
        .unwrap_or(false);

    Ok(crate::comfy::capture_ingest::IngestRequest {
        run_key: run_key.to_string(),
        source: source.to_string(),
        status: body
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        error: body
            .get("error")
            .and_then(Value::as_str)
            .map(|s| s.to_string()),
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
        title: body
            .get("title")
            .and_then(Value::as_str)
            .map(|s| s.to_string()),
        notes: body
            .get("notes")
            .and_then(Value::as_str)
            .map(|s| s.to_string()),
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

/// `POST /api/capture/import`：把一个目录里**已经生成好的产物**导入画廊（P4 第 52 步）。
///
/// 请求体（Kotlin `ImportFolderRequest`）：`{dir, recursive=true, limit=200, linkWorkflow=true, tags[]}`。
///
/// # 与 Kotlin `ComfyCapture.importFolder` 逐条对齐的三件事
///
///  1. **PNG 内嵌元数据还原成提示词**（`linkWorkflow=true` 时）：ComfyUI 把这次生成用的节点图
///     写在 PNG 的 `prompt` 文本块里，于是"一张图"能还原成"一条带参数的提示词"；
///     幂等靠 `run_key = import:<sha 前 40 位>`，但这条路用 **`force` 抢占**：
///     上面刚用 sha256 确认过文件不在库里，历史运行记录不该拦住重新导入。
///  2. **按内容 sha256 判重**：同一份文件重复导入 → `duplicates++`，不重复入库。
///  3. **别把自己 storage 里的文件再导入一遍**（`media` / `thumbs` / `ai-attachments` / `ai-thumbs` 跳过）。
///
/// 单个文件失败**不整批失败**：记进 `errors`（`名字: 原因`），其余继续 —— 不许静默跳过。
pub async fn import_folder_json(state: &AppState, body: &Value) -> Result<Value, ApiError> {
    let raw_dir = body
        .get("dir")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if raw_dir.is_empty() {
        return Err(ApiError::bad_request("dir 不能为空"));
    }
    let dir = std::path::PathBuf::from(&raw_dir);
    if !dir.is_dir() {
        return Err(ApiError::bad_request(format!("目录不存在: {raw_dir}")));
    }
    let recursive = body
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let link_workflow = body
        .get("linkWorkflow")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let limit = body
        .get("limit")
        .and_then(|v| v.as_i64())
        .unwrap_or(200)
        .clamp(1, 5000) as usize;
    let tags: Vec<String> = body
        .get("tags")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(|t| t.to_string())
                .collect()
        })
        .unwrap_or_default();

    // 自己的 storage 不许再导一遍（否则"导入一次"会变成"每导一次多几份副本"）
    let storage = crate::media::attachments::storage_root(state);
    let skip_dirs = [
        storage.join("media"),
        storage.join("thumbs"),
        storage.join("ai-attachments"),
        storage.join("ai-thumbs"),
    ];

    let files = collect_files(&dir, recursive, limit, &skip_dirs);

    let mut scanned = 0i64;
    let mut imported = 0i64;
    let mut duplicates = 0i64;
    let mut failed = 0i64;
    let mut prompts_created = 0i64;
    let mut prompt_ids: Vec<i64> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for path in files.iter() {
        scanned += 1;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        match import_one(state, path, &name, link_workflow, &tags).await {
            Ok(Outcome::Imported {
                prompt_id,
                created_prompt,
            }) => {
                imported += 1;
                if let Some(id) = prompt_id {
                    prompt_ids.push(id);
                    if created_prompt {
                        prompts_created += 1;
                    }
                }
            }
            Ok(Outcome::Duplicate) => duplicates += 1,
            Ok(Outcome::Failed { reason }) => {
                failed += 1;
                errors.push(format!("{name}: {reason}"));
            }
            Err(error) => {
                failed += 1;
                errors.push(format!("{name}: {}", error.human()));
            }
        }
    }

    Ok(json!({
        "dir": dir.display().to_string(),
        "scanned": scanned,
        "imported": imported,
        "duplicates": duplicates,
        "failed": failed,
        "promptsCreated": prompts_created,
        "promptIds": prompt_ids,
        "errors": errors,
    }))
}

enum Outcome {
    Imported {
        prompt_id: Option<i64>,
        created_prompt: bool,
    },
    /// 已经在库里（Kotlin 那边会把已有的 promptId 带出来，但 ImportFolderResult
    /// 只有计数与 rrors，带出来也没人用 —— 所以这里不存）。
    Duplicate,
    Failed {
        reason: String,
    },
}

/// 一个文件：判重 → （PNG）读内嵌图 → 建提示词 → 收进 storage。
async fn import_one(
    state: &AppState,
    path: &std::path::Path,
    name: &str,
    link_workflow: bool,
    tags: &[String],
) -> Result<Outcome, ApiError> {
    let sha = crate::media::media_upload::sha256_of(path)
        .map_err(|e| ApiError::internal(format!("读文件失败：{e}")))?;

    // ① 内容已经在库里 → 重复（把已有的 promptId 带回去，界面能显示"已关联哪条提示词"）
    let existing: Option<(i64, Option<i64>)> = state.with(|conn| {
        use rusqlite::OptionalExtension;
        conn.query_row(
            "SELECT id, prompt_id FROM media_assets WHERE sha256 = ?1 LIMIT 1",
            rusqlite::params![sha],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()
    })?;
    if let Some((_, prompt_id)) = existing {
        let _ = prompt_id;
        return Ok(Outcome::Duplicate);
    }

    // ② PNG 的内嵌元数据（ComfyUI 把这次生成用的 API 图写在 `prompt` 块里）
    let mut graph: Option<Value> = None;
    let mut workflow: Option<String> = None;
    if crate::media::media_upload::extension_of(name) == "png" {
        let chunks = crate::media::png_text::read_text_chunks(path);
        graph = chunks
            .get("prompt")
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .filter(|value| value.is_object());
        workflow = chunks
            .get("workflow")
            .cloned()
            .filter(|text| !text.trim().is_empty());
    }

    let run_key = format!("import:{}", &sha[..sha.len().min(40)]);
    let notes = format!(
        "从 {} 导入，参数取自 PNG 内嵌的 prompt 元数据",
        path.parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    );

    let mut prompt_id: Option<i64> = None;
    let mut created_prompt = false;
    // 从 PNG 内嵌的 API 图推断自动标签；没有工作流就没有可推断的依据。
    let mut auto_tags: Vec<crate::library::auto_tag::AutoTag> = Vec::new();
    if link_workflow {
        if let Some(graph) = graph.as_ref() {
            // `force = true`：上面已经确认过文件不在库里，历史运行记录不该拦住重新导入
            let claim = crate::comfy::capture_ingest::begin_run(
                state,
                &run_key,
                "ComfyUI-Import",
                None,
                true,
            )?;
            if !claim.claimed {
                return Ok(Outcome::Duplicate);
            }
            let parsed = crate::workflow::graph_parse::parse(graph.as_object());
            auto_tags = crate::library::auto_tag::infer(&parsed, false);
            let title = make_import_title(&parsed, name);
            let tags_owned = tags.to_vec();
            let workflow_ref = workflow.clone();
            let prompt = state.write(|tx| {
                crate::comfy::capture_ingest::create_captured_prompt(
                    tx,
                    &parsed,
                    &title,
                    "ComfyUI-Import",
                    Some(&run_key),
                    workflow_ref.as_deref(),
                    &tags_owned,
                    &auto_tags,
                    Some(&notes),
                )
            });
            match prompt {
                Ok(id) => {
                    created_prompt = true;
                    prompt_id = Some(id);
                }
                Err(error) => {
                    failed_claim_cleanup(state, &run_key);
                    return Ok(Outcome::Failed {
                        reason: format!("建提示词失败：{error:?}"),
                    });
                }
            }
        }
    }

    let kind = crate::media::media_upload::kind_of(name, None);
    let imported = crate::comfy::capture_ingest::import_file(
        state,
        path,
        name,
        Some(&kind),
        prompt_id,
        None,
        "ComfyUI-Import",
        Some(&run_key),
        workflow.as_deref(),
        Some(&notes),
        tags,
        &auto_tags,
        false,
        false,
    );
    match imported {
        crate::comfy::capture_ingest::Imported::Created { .. } => Ok(Outcome::Imported {
            prompt_id,
            created_prompt,
        }),
        crate::comfy::capture_ingest::Imported::Duplicate { .. } => Ok(Outcome::Duplicate),
        crate::comfy::capture_ingest::Imported::Failed { reason } => {
            failed_claim_cleanup(state, &run_key);
            Ok(Outcome::Failed { reason })
        }
    }
}

/// 导入失败时把抢占到的运行记录**删掉**：留着会让下次导入被判成"正在处理中"
/// （Kotlin 那条"删掉提示词之后再 load 永远回 LOAD_IN_PROGRESS"的亲戚）。
fn failed_claim_cleanup(state: &AppState, run_key: &str) {
    let _ = state.write(|conn| {
        conn.execute(
            "DELETE FROM capture_runs WHERE run_key = ?1",
            rusqlite::params![run_key],
        )?;
        Ok(())
    });
}

/// 导入产物的提示词标题（Kotlin `makeImportTitle`）。
fn make_import_title(parsed: &crate::workflow::graph_parse::Parsed, filename: &str) -> String {
    let prefix = parsed
        .title_hint
        .clone()
        .filter(|hint| !hint.trim().is_empty())
        .unwrap_or_else(|| {
            filename
                .rsplit_once('.')
                .map(|(stem, _)| stem)
                .unwrap_or(filename)
                .trim_end_matches(['_', ' ', '-'])
                .to_string()
        });
    let base = if prefix.trim().is_empty() {
        "导入的产物".to_string()
    } else {
        prefix
    };
    let title = match parsed.seed {
        Some(seed) => format!("{base} · seed {seed}"),
        None => base,
    };
    title.chars().take(250).collect()
}

/// 目录遍历：只收**媒体扩展名**、跳过自己的 storage、跳过点文件。
///
/// `recursive=false` 只看顶层；`limit` 是**扫描上限**（与 Kotlin 同：`scanned` 到上限就停）。
fn collect_files(
    dir: &std::path::Path,
    recursive: bool,
    limit: usize,
    skip_dirs: &[std::path::PathBuf],
) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        let mut children: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path()).collect();
        children.sort(); // 稳定顺序：同样的目录每次导入的处理次序一致
        for path in children {
            if out.len() >= limit {
                return out;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if recursive && !skip_dirs.contains(&path) {
                    stack.push(path);
                }
                continue;
            }
            if skip_dirs.iter().any(|skip| path.starts_with(skip)) {
                continue;
            }
            // 只认**媒体**扩展名（复用同一张 MIME 表，别另写一份扩展名清单）
            let mime = crate::media::media_upload::detect_mime(&name, None);
            if mime.starts_with("image/")
                || mime.starts_with("video/")
                || mime.starts_with("audio/")
            {
                out.push(path);
            }
        }
    }
    out.truncate(limit);
    out
}

/// `POST /api/capture/import-workflows`：把本机 ComfyUI 里已保存的工作流文件**批量读进库**。
///
/// 请求体 `{dir?, limit?, query?}`（Kotlin `ImportWorkflowsRequest`）：
/// `dir` 不填就自动发现（[`crate::comfy::comfy_dirs::workflow_dirs`]），填了就当它是那个目录本身。
pub async fn import_workflows(state: &AppState, body: &Value) -> Result<Value, ApiError> {
    let project_root = state.project_root();
    let configured_output = capture_config(state)
        .ok()
        .and_then(|config| config.output_dir);

    let dirs: Vec<std::path::PathBuf> = match body.get("dir").and_then(Value::as_str).map(str::trim)
    {
        Some(raw) if !raw.is_empty() => {
            let path = std::path::PathBuf::from(raw);
            if !path.is_dir() {
                return Err(ApiError::bad_request(format!("目录不存在：{raw}")));
            }
            vec![path]
        }
        _ => crate::comfy::comfy_dirs::workflow_dirs(&project_root, configured_output.as_deref()),
    };

    let limit = body
        .get("limit")
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        })
        .unwrap_or(MAX_FILES)
        .clamp(1, MAX_FILES) as usize;
    let query = body
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty());

    // ⚠ 只扫**能用的**（坏 JSON / 空 / 非 .json / 格式认不出来的都不列）：
    // 它们 `comfy_load_workflow` 一定失败，收进来只会让用户看见一堆"失败"
    let files = crate::comfy::comfy_dirs::scan(&dirs, query, limit);

    let mut imported = 0i64;
    let mut duplicates = 0i64;
    let mut failed = 0i64;
    let mut prompt_ids: Vec<i64> = Vec::new();
    let mut items: Vec<Value> = Vec::new();

    for file in files.iter() {
        let run_key = format!("file:{}", file.sha256);
        // 幂等：库里已经有这份文件（同一把钥匙 `file:<sha256>`）→ 算"已在库"，不重复导入
        match crate::comfy::capture::find_run(state, &run_key) {
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
        match crate::workflow::workflow_load::load_from_file(state, &path, None, false, true, true)
            .await
        {
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
            dirs.iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join("、")
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
mod tests;
