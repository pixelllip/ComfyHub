//! 捕获入库（P4 第 43 步第二半）—— **"产物真的进画廊"** 就落在这一层。
//!
//! # 与 Kotlin 的对应物
//!
//! | 这里 | Kotlin |
//! | --- | --- |
//! | [`begin_run`] / [`can_reclaim`] / [`finish_run`] | `CaptureRepo.beginRun` / `canReclaim` / `finishRun` |
//! | [`import_file`] | `CaptureRepo.importFile` |
//! | [`create_captured_prompt`] | `PromptRepo.createCaptured` |
//! | [`poll_once`] / [`ingest_claimed`] | `ComfyCapture.pollOnce` / `ingestClaimed` |
//! | [`build_ingest_request`] | `ComfyCapture.buildIngestRequest` |
//!
//! # 四条纪律（每一条都是实测踩出来的）
//!
//! 1. **`empty` / `error` 是可以重试的状态**（[`can_reclaim`]）：ComfyUI 的 `/history`
//!    先出现记录、产物文件晚到是常态。把 `empty` 当终态就会出现"AI 说生成了、画廊里却没有"。
//!    `success` 才是唯一不再重复收的终态（幂等靠它 + 文件 sha256）。
//! 2. **`running` 且没过 10 分钟要让开**：别的轮询正在收这一次。但**卡了超过 10 分钟的
//!    `running` 要能重新抢占** —— 否则后端被中途杀掉之后，那一次运行再也补不上。
//! 3. **一次运行只建一条提示词**：`prompts.source_ref = run_key`，加上 `capture_runs.run_key`
//!    的唯一约束 —— "AI 刚提交完就入库"与"轮询几秒后又看到它"不会变成两条数据。
//! 4. **产物按内容 sha256 判重**：同一张图重复导入只留一条（与上传那条路同一个口径）。
//!
//! # 一处**刻意不改**的行为
//!
//! `empty` / `error` 收尾时 `prompt_id` 写的是 **NULL**（Kotlin 同）：那一次运行没有产物，
//! 也就不该在库里留下一条空提示词。

use std::path::{Path, PathBuf};

use rusqlite::params;
use serde_json::{json, Map, Value};

use crate::comfy::capture::CaptureConfig;
use crate::web::routes::{ApiError, AppState};

/// SQLite 里"现在"的写法（Kotlin `Db.now` 的 SQLite 分支）。
const NOW_SQL: &str = "strftime('%Y-%m-%dT%H:%M:%fZ','now')";

/// 僵尸 `running` 的判定门限（Kotlin `canReclaim` 里那个 600 秒）。
pub const STALE_RUNNING_SECONDS: i64 = 600;

// ---------------------------------------------------------------------------
//  运行记录（claim / finish）
// ---------------------------------------------------------------------------

/// 一次运行的抢占结果（Kotlin `CaptureRepo.RunClaim`）。
#[derive(Debug, Clone, PartialEq)]
pub struct RunClaim {
    pub claimed: bool,
    pub existing_prompt_id: Option<i64>,
    pub existing_status: Option<String>,
}

/// 已经存在一条记录时，这次调用能不能抢占它（Kotlin `canReclaim`，**纯函数**）。
///
/// | 情况 | 结论 |
/// | --- | --- |
/// | `running` 且 ≤600 秒 | **不让**（别人正在收） |
/// | `empty` / `error` | **能重抢**（产物晚到是常态） |
/// | `force`（目录导入） | **能重抢**（那边已用 sha256 确认文件不在库里） |
/// | 卡住的 `running` | 能重抢（后端被杀过） |
/// | `success` | 不让（幂等靠它） |
pub fn can_reclaim(status: &str, age_seconds: i64, force: bool) -> bool {
    match status {
        "running" if age_seconds <= STALE_RUNNING_SECONDS => false,
        "empty" | "error" => true,
        _ if force => true,
        "running" => true,
        _ => false,
    }
}

/// 抢占一次运行（Kotlin `beginRun`）。
///
/// `INSERT OR IGNORE` + 唯一键：抢不到说明这次生成已经处理过（或正在处理）。
pub fn begin_run(
    state: &AppState,
    run_key: &str,
    source: &str,
    raw: Option<&Value>,
    force: bool,
) -> Result<RunClaim, ApiError> {
    let raw_text = raw.map(|value| value.to_string());
    let run_key = run_key.trim().to_string();
    state.write(|tx| {
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO capture_runs (run_key, source, status, raw) VALUES (?1,?2,'running',?3)",
            params![run_key, source, raw_text],
        )?;
        if inserted > 0 {
            return Ok(RunClaim { claimed: true, existing_prompt_id: None, existing_status: None });
        }
        // 僵尸 Run 的年龄：时间列是 ISO-8601 UTC 文本，`julianday` 认
        let existing: Option<(Option<i64>, Option<String>, Option<i64>)> = tx
            .query_row(
                "SELECT prompt_id, status, \
                   CAST((julianday('now') - julianday(created_at)) * 86400 AS INTEGER) AS age \
                   FROM capture_runs WHERE run_key = ?1",
                params![run_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok();
        let Some((prompt_id, status, age)) = existing else {
            // 行刚被别人删了（并发）——按"可以抢"处理，下面的 UPDATE 会再插一次
            return Ok(RunClaim { claimed: true, existing_prompt_id: None, existing_status: None });
        };
        let status = status.unwrap_or_default();
        let age = age.unwrap_or(0);
        if !can_reclaim(&status, age, force) {
            return Ok(RunClaim {
                claimed: false,
                existing_prompt_id: prompt_id,
                existing_status: Some(status),
            });
        }
        tx.execute(
            &format!(
                "UPDATE capture_runs SET status = 'running', error = NULL, created_at = {NOW_SQL} WHERE run_key = ?1"
            ),
            params![run_key],
        )?;
        Ok(RunClaim { claimed: true, existing_prompt_id: None, existing_status: None })
    })
}

/// 收尾一次运行（Kotlin `finishRun`）。
///
/// `title` 截到 255、`error` 截到 2000 字符（与 Kotlin 的 `take(...)` 同）。
pub fn finish_run(
    state: &AppState,
    run_key: &str,
    prompt_id: Option<i64>,
    status: &str,
    media_count: i64,
    title: Option<&str>,
    error: Option<&str>,
) -> Result<(), ApiError> {
    let title: Option<String> = title.map(|t| take_chars(t, 255));
    let error: Option<String> = error.map(|e| take_chars(e, 2000));
    state.write(|tx| {
        tx.execute(
            "UPDATE capture_runs SET prompt_id = ?1, status = ?2, media_count = ?3, title = ?4, error = ?5 \
             WHERE run_key = ?6",
            params![prompt_id, status, media_count, title, error, run_key.trim()],
        )?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
//  产物入库
// ---------------------------------------------------------------------------

/// 单次文件入库的结果（Kotlin `CaptureRepo.Imported`）。
#[derive(Debug, Clone, PartialEq)]
pub enum Imported {
    Created { media_id: i64, kind: String },
    Duplicate { existing_id: i64 },
    Failed { reason: String },
}

/// 把一个**已经在磁盘上**的文件收进 storage（Kotlin `importFile`）。
///
/// `move = true` 表示源文件是临时文件（下载下来的那份），入库后不再需要；
/// `false` 表示源文件属于用户（ComfyUI 的 output 目录），**只复制**。
#[allow(clippy::too_many_arguments)]
pub fn import_file(
    state: &AppState,
    source: &Path,
    original_name: &str,
    kind_hint: Option<&str>,
    prompt_id: Option<i64>,
    title: Option<&str>,
    source_label: &str,
    source_ref: Option<&str>,
    workflow_json: Option<&str>,
    notes: Option<&str>,
    tags: &[String],
    move_file: bool,
    nsfw: bool,
) -> Imported {
    if !source.is_file() {
        return Imported::Failed { reason: format!("文件不存在: {}", source.display()) };
    }
    let kind = kind_hint
        .map(|k| k.trim().to_uppercase())
        .filter(|k| crate::media::media_upload::KINDS.contains(&k.as_str()))
        .unwrap_or_else(|| crate::media::media_upload::kind_of(original_name, None));
    let sha = match crate::media::media_upload::sha256_of(source) {
        Ok(sha) => sha,
        Err(e) => return Imported::Failed { reason: format!("读取文件失败: {e}") },
    };

    // 内容去重：同一份文件重复导入时直接复用已有记录
    let existing: Result<Option<i64>, ApiError> = state.with(|conn| {
        use rusqlite::OptionalExtension;
        Ok(conn
            .query_row("SELECT id FROM media_assets WHERE sha256 = ?1 LIMIT 1", params![sha], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?)
    });
    match existing {
        Ok(Some(existing_id)) => {
            if move_file {
                let _ = std::fs::remove_file(source);
            }
            return Imported::Duplicate { existing_id };
        }
        Ok(None) => {}
        Err(e) => return Imported::Failed { reason: e.human().to_string() },
    }

    let stored_name = crate::media::media_upload::new_stored_name(original_name);
    let media_dir = state.media_dir().to_path_buf();
    let dest = media_dir.join(&stored_name);
    if let Err(e) = std::fs::create_dir_all(&media_dir) {
        return Imported::Failed { reason: format!("建不了存储目录: {e}") };
    }
    // move 跨盘可能失败 → 退回复制 + 删源（与 Kotlin 同）
    let copied = if move_file {
        match std::fs::rename(source, &dest) {
            Ok(()) => Ok(()),
            Err(_) => std::fs::copy(source, &dest).map(|_| {
                let _ = std::fs::remove_file(source);
            }),
        }
    } else {
        std::fs::copy(source, &dest).map(|_| ())
    };
    if let Err(e) = copied {
        return Imported::Failed { reason: format!("写入存储目录失败: {e}") };
    }

    let size = std::fs::metadata(&dest).map(|m| m.len() as i64).unwrap_or(0);
    let mime = crate::media::media_upload::detect_mime(original_name, None);
    let dims = if kind == "IMAGE" { crate::media::thumbs::probe_image_size(&dest) } else { None };
    let new_title = title.map(|t| take_chars(t, 255)).unwrap_or_else(|| original_name.to_string());

    let inserted = state.write(|tx| -> Result<i64, crate::library::repo::WriteError> {
        tx.execute(
            "INSERT INTO media_assets
               (prompt_id, kind, title, original_name, stored_name, mime_type, size_bytes,
                width, height, duration_ms, sha256, source, notes, source_ref, workflow_json, nsfw, favorite)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,NULL,?10,?11,?12,?13,?14,?15,0)",
            params![
                prompt_id,
                kind,
                new_title,
                original_name,
                stored_name,
                mime,
                size,
                dims.map(|d| d.0),
                dims.map(|d| d.1),
                sha,
                source_label,
                notes,
                source_ref,
                workflow_json,
                if nsfw { 1 } else { 0 },
            ],
        )?;
        let id = tx.last_insert_rowid();
        if !tags.is_empty() {
            crate::library::repo::add_media_tags(tx, id, tags)?;
        }
        crate::library::repo::refresh_use_counts(tx)?;
        Ok(id)
    });

    match inserted {
        Ok(media_id) => {
            if kind == "IMAGE" {
                let thumb = crate::media::thumbs::thumb_path(&media_dir, media_id);
                let _ = crate::media::thumbs::write_thumbnail(&dest, &thumb);
            }
            Imported::Created { media_id, kind }
        }
        Err(e) => {
            // 入库失败：把刚落的那份清掉（别留一地孤儿文件）
            let _ = std::fs::remove_file(&dest);
            Imported::Failed { reason: e.human().to_string() }
        }
    }
}

// ---------------------------------------------------------------------------
//  提示词入库
// ---------------------------------------------------------------------------

/// 建一条"捕获来的"提示词（Kotlin `PromptRepo.createCaptured`），返回新 id。
///
/// ⚠ 两个**空集合的写法**必须与 Kotlin 一致：`loras` 空写 **`[]`**、`extra_params` 空写
/// **`{}`**（Kotlin 走的是序列化器，永远给合法 JSON；写真 NULL 会让"库里这一列是什么形状"
/// 两端分叉 —— 真库实测 ComfyUI 来源的行全是 `[]` / `{}`）。
pub fn create_captured_prompt(
    tx: &rusqlite::Connection,
    parsed: &crate::workflow::graph_parse::Parsed,
    title: &str,
    source: &str,
    source_ref: Option<&str>,
    workflow_json: Option<&str>,
    tags: &[String],
    notes: Option<&str>,
) -> Result<i64, crate::library::repo::WriteError> {
    let title = {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            crate::library::repo::auto_title(&parsed.positive)
        } else {
            take_chars(trimmed, 255)
        }
    };
    let kind = {
        let upper = parsed.kind.to_uppercase();
        if crate::library::repo::KINDS.contains(&upper.as_str()) {
            upper
        } else {
            "IMAGE".to_string()
        }
    };
    let loras = json!(parsed
        .loras
        .iter()
        .map(|l| json!({ "name": l.name, "weight": l.weight }))
        .collect::<Vec<_>>())
    .to_string();
    // ⚠ `extra` 的**键顺序**是 `GraphParse` 的文档顺序（真库里那几行就是文档顺序），
    // 所以这里手工按顺序拼一个对象 —— 换成 `BTreeMap`/`json!(map)` 会把顺序改成键排序。
    let extra = {
        let mut map = Map::new();
        for (key, value) in &parsed.extra {
            map.insert(key.clone(), Value::String(value.clone()));
        }
        Value::Object(map).to_string()
    };

    tx.execute(
        "INSERT INTO prompts
           (title, kind, positive_prompt, negative_prompt, checkpoint, loras, sampler, scheduler,
            steps, cfg_scale, seed, width, height, batch_size, extra_params, notes, favorite,
            source, source_ref, workflow_json)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,0,?17,?18,?19)",
        params![
            title,
            kind,
            parsed.positive,
            parsed.negative.as_deref().filter(|t| !t.trim().is_empty()),
            parsed.checkpoint.as_deref().filter(|t| !t.trim().is_empty()),
            loras,
            parsed.sampler.as_deref().filter(|t| !t.trim().is_empty()),
            parsed.scheduler.as_deref().filter(|t| !t.trim().is_empty()),
            parsed.steps,
            parsed.cfg,
            parsed.seed,
            parsed.width,
            parsed.height,
            parsed.batch,
            extra,
            notes.filter(|t| !t.trim().is_empty()),
            take_chars(source, 32),
            source_ref.map(|r| take_chars(r, 128)),
            workflow_json,
        ],
    )?;
    let id = tx.last_insert_rowid();
    if !tags.is_empty() {
        crate::library::repo::add_prompt_tags(tx, id, tags)?;
    }
    crate::library::repo::refresh_use_counts(tx)?;
    Ok(id)
}

// ---------------------------------------------------------------------------
//  一次运行 → 入库
// ---------------------------------------------------------------------------

/// 一个待入库的产物条目（Kotlin `IngestOutput`）。
#[derive(Debug, Clone, PartialEq)]
pub struct IngestOutput {
    pub filename: String,
    pub subfolder: Option<String>,
    pub type_field: Option<String>,
    pub kind: Option<String>,
    pub node_id: Option<String>,
    pub node_type: Option<String>,
}

/// 一次运行的捕获请求（Kotlin `IngestRequest` 的**子集**：轮询这条路用得到的那些）。
#[derive(Debug, Clone)]
pub struct IngestRequest {
    pub run_key: String,
    pub source: String,
    pub status: String,
    pub error: Option<String>,
    pub comfy_url: Option<String>,
    pub output_dir: Option<String>,
    pub prompt: Option<Value>,
    pub workflow: Option<Value>,
    pub outputs: Vec<IngestOutput>,
    /// 成人内容标记（`comfy_submit(nsfw=true)` 走这里；轮询没有这个信息 → false）
    pub nsfw: bool,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    pub raw: Option<Value>,
}

/// 入库结果（Kotlin `IngestResult`；形状与键序都照它）。
#[derive(Debug, Clone, Default)]
pub struct IngestResult {
    pub run_key: String,
    pub prompt_id: Option<i64>,
    pub created: bool,
    pub media_ids: Vec<i64>,
    pub imported: i64,
    pub duplicates: i64,
    pub failed: i64,
    pub already_captured: bool,
    pub message: Option<String>,
}

impl IngestResult {
    /// Kotlin 是 `@Serializable` + `encodeDefaults=true` + 省略 null：
    /// 除 `promptId` / `message` 外**所有键都在**（哪怕值是默认值）。
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("runKey".into(), json!(self.run_key));
        if let Some(id) = self.prompt_id {
            out.insert("promptId".into(), json!(id));
        }
        out.insert("created".into(), json!(self.created));
        out.insert("mediaIds".into(), json!(self.media_ids));
        out.insert("imported".into(), json!(self.imported));
        out.insert("duplicates".into(), json!(self.duplicates));
        out.insert("failed".into(), json!(self.failed));
        out.insert("alreadyCaptured".into(), json!(self.already_captured));
        if let Some(message) = &self.message {
            out.insert("message".into(), json!(message));
        }
        Value::Object(out)
    }
}

/// 从一条 `/history` 条目拼出捕获请求（Kotlin `buildIngestRequest`）。
pub fn build_ingest_request(
    run_key: &str,
    entry: &Value,
    config: &CaptureConfig,
    is_error: bool,
) -> IngestRequest {
    let history = crate::comfy::capture::history::parse(entry);
    let graph_object = history.graph.as_ref().and_then(Value::as_object);
    let outputs = collect_outputs(entry, graph_object);
    IngestRequest {
        run_key: run_key.to_string(),
        source: "ComfyUI".to_string(),
        status: if is_error { "error".to_string() } else { "success".to_string() },
        error: None,
        comfy_url: Some(config.comfy_url.clone()),
        output_dir: config.output_dir.clone(),
        prompt: history.graph.clone(),
        workflow: history.workflow.clone(),
        outputs,
        nsfw: false,
        title: None,
        notes: None,
        tags: Vec::new(),
        raw: Some(entry.clone()),
    }
}

/// `entry["outputs"]` → 产物条目（Kotlin `buildIngestRequest` 里那个循环）。
fn collect_outputs(entry: &Value, graph: Option<&Map<String, Value>>) -> Vec<IngestOutput> {
    let Some(outputs) = entry.get("outputs").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out: Vec<IngestOutput> = Vec::new();
    for (node_id, node_output) in outputs {
        let Some(node) = node_output.as_object() else { continue };
        let node_type = graph
            .and_then(|g| g.get(node_id))
            .and_then(|n| n.get("class_type"))
            .and_then(Value::as_str)
            .map(str::to_string);
        for key in ["images", "gifs", "audio", "video", "videos", "files"] {
            let Some(items) = node.get(key).and_then(Value::as_array) else { continue };
            for item in items {
                let Some(item) = item.as_object() else { continue };
                let Some(filename) = item.get("filename").and_then(Value::as_str) else { continue };
                out.push(IngestOutput {
                    filename: filename.to_string(),
                    subfolder: item.get("subfolder").and_then(Value::as_str).map(str::to_string),
                    type_field: item
                        .get("type")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| Some("output".to_string())),
                    kind: None,
                    node_id: Some(node_id.clone()),
                    node_type: node_type.clone(),
                });
            }
        }
    }
    out
}

/// 已经抢到运行锁之后的实际入库（Kotlin `ingestClaimed`）。
///
/// `async` 的唯一理由是**下载兜底**（本地 output 目录里没有那份文件时才走 HTTP）。
pub async fn ingest_claimed(
    state: &AppState,
    req: &IngestRequest,
    config: &CaptureConfig,
) -> IngestResult {
    let run_key = req.run_key.clone();
    let source = if req.source.trim().is_empty() { "ComfyUI" } else { req.source.as_str() };
    match ingest_inner(state, req, config, source).await {
        Ok(result) => result,
        Err(message) => {
            let _ = finish_run(state, &run_key, None, "error", 0, None, Some(&message));
            IngestResult {
                run_key: run_key.clone(),
                message: Some(format!("捕获失败: {message}")),
                ..Default::default()
            }
        }
    }
}

async fn ingest_inner(
    state: &AppState,
    req: &IngestRequest,
    config: &CaptureConfig,
    source: &str,
) -> Result<IngestResult, String> {
    let run_key = req.run_key.clone();
    let graph = req.prompt.as_ref().and_then(Value::as_object);
    let parsed = crate::workflow::graph_parse::parse(graph);

    // 标签：ComfyUI 来源自动带上配置里的 autoTag，再加上调用方给的
    let mut tags: Vec<String> = Vec::new();
    if source == "ComfyUI" && !config.auto_tag.trim().is_empty() {
        push_unique(&mut tags, config.auto_tag.trim());
    }
    for tag in &req.tags {
        if !tag.trim().is_empty() {
            push_unique(&mut tags, tag.trim());
        }
    }

    // 只收真正的输出：文件名非空 + type 空或 output（大小写不敏感），并按三元组去重
    let mut files: Vec<&IngestOutput> = Vec::new();
    let mut seen: Vec<(String, String, String)> = Vec::new();
    for item in &req.outputs {
        if item.filename.trim().is_empty() {
            continue;
        }
        let type_field = item.type_field.clone().unwrap_or_default();
        if !type_field.trim().is_empty() && !type_field.eq_ignore_ascii_case("output") {
            continue;
        }
        let key = (
            item.filename.clone(),
            item.subfolder.clone().unwrap_or_default(),
            if type_field.trim().is_empty() { "output".to_string() } else { type_field },
        );
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        files.push(item);
    }

    if files.is_empty() {
        let status = if req.status == "error" { "error" } else { "empty" };
        finish_run(state, &run_key, None, status, 0, None, req.error.as_deref())
            .map_err(|e| e.human().to_string())?;
        return Ok(IngestResult {
            run_key,
            message: Some(
                if status == "error" {
                    "这次运行执行失败，只记录了日志".to_string()
                } else {
                    "这次运行没有产出文件".to_string()
                },
            ),
            ..Default::default()
        });
    }

    let title = make_title(req, &parsed, &files);
    // 优先存界面格式工作流（能拖回 ComfyUI）；没有就把 API 节点图原样存下来
    let workflow_json = req.workflow.as_ref().or(req.prompt.as_ref()).map(|v| v.to_string());

    let prompt_id = state
        .write(|tx| {
            create_captured_prompt(
                tx,
                &parsed,
                &title,
                source,
                Some(&run_key),
                workflow_json.as_deref(),
                &tags,
                req.notes.as_deref(),
            )
        })
        .map_err(|e| e.human().to_string())?;

    let comfy_url = req
        .comfy_url
        .clone()
        .unwrap_or_else(|| config.comfy_url.clone())
        .trim_end_matches('/')
        .to_string();
    let output_dir = req.output_dir.clone().or_else(|| config.output_dir.clone());

    let mut imported = 0i64;
    let mut duplicates = 0i64;
    let mut failed = 0i64;
    let mut media_ids: Vec<i64> = Vec::new();
    for item in &files {
        let local = resolve_local_output(output_dir.as_deref(), item);
        let mut source_path = local.clone();
        let mut is_temp = false;
        if source_path.is_none() && config.download_fallback && !comfy_url.is_empty() {
            source_path = download_output(&comfy_url, item, state).await;
            is_temp = source_path.is_some();
        }
        let Some(path) = source_path else {
            failed += 1;
            continue;
        };
        let kind_hint = kind_for(item);
        match import_file(
            state,
            &path,
            &item.filename,
            Some(&kind_hint),
            Some(prompt_id),
            None,
            source,
            Some(&run_key),
            workflow_json.as_deref(),
            req.notes.as_deref(),
            &tags,
            is_temp,
            req.nsfw,
        ) {
            Imported::Created { media_id, .. } => {
                imported += 1;
                media_ids.push(media_id);
            }
            Imported::Duplicate { .. } => duplicates += 1,
            Imported::Failed { .. } => failed += 1,
        }
    }

    finish_run(state, &run_key, Some(prompt_id), "success", imported, Some(&title), None)
        .map_err(|e| e.human().to_string())?;

    Ok(IngestResult {
        run_key,
        prompt_id: Some(prompt_id),
        created: true,
        media_ids,
        imported,
        duplicates,
        failed,
        already_captured: false,
        message: None,
    })
}

/// 标题（Kotlin `makeTitle`）：显式标题 → 保存节点的 filename_prefix → 文件名（去掉扩展名）。
fn make_title(req: &IngestRequest, parsed: &crate::workflow::graph_parse::Parsed, files: &[&IngestOutput]) -> String {
    let prefix = req
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| parsed.title_hint.clone())
        .or_else(|| {
            files.first().map(|f| {
                let stem = f.filename.rsplit_once('.').map(|(head, _)| head).unwrap_or(&f.filename);
                stem.trim_end_matches(['_', ' ', '-']).to_string()
            })
        });
    let base = prefix.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "ComfyUI 生成".to_string());
    let full = match parsed.seed {
        Some(seed) => format!("{base} · seed {seed}"),
        None => base,
    };
    take_chars(&full, 250)
}

/// 本地输出文件（Kotlin `resolveLocalOutput`）：`<outputDir>/<subfolder>/<filename>`。
fn resolve_local_output(output_dir: Option<&str>, item: &IngestOutput) -> Option<PathBuf> {
    let dir = output_dir?.trim();
    if dir.is_empty() {
        return None;
    }
    let mut path = PathBuf::from(dir);
    if let Some(subfolder) = item.subfolder.as_deref().filter(|s| !s.trim().is_empty()) {
        path.push(subfolder);
    }
    path.push(&item.filename);
    // 词法归一化（`..` 不进文件系统）：路径来自 ComfyUI 的返回值，不能直接信
    let normalized = crate::tools::policy::normalize_lexical(&path);
    normalized.is_file().then_some(normalized)
}

/// HTTP 下载兜底（Kotlin `downloadOutput`）：`GET /view?filename=&subfolder=&type=`。
async fn download_output(comfy_url: &str, item: &IngestOutput, state: &AppState) -> Option<PathBuf> {
    let mut query = format!("filename={}", url_encode(&item.filename));
    if let Some(subfolder) = item.subfolder.as_deref().filter(|s| !s.trim().is_empty()) {
        query.push_str(&format!("&subfolder={}", url_encode(subfolder)));
    }
    let type_field = item.type_field.clone().unwrap_or_else(|| "output".to_string());
    query.push_str(&format!("&type={}", url_encode(&type_field)));
    let url = format!("{comfy_url}/view?{query}");

    let tmp_dir = state.media_dir().parent().unwrap_or(state.media_dir()).join("tmp");
    std::fs::create_dir_all(&tmp_dir).ok()?;
    let ext = {
        let ext = crate::media::media_upload::extension_of(&item.filename);
        if ext.is_empty() { "bin".to_string() } else { ext }
    };
    let tmp = tmp_dir.join(format!("capture-{}.{ext}", random_suffix()));

    let bytes = crate::comfy::comfy_client::get_bytes(&url, 300).await.ok()?;
    std::fs::write(&tmp, &bytes).ok()?;
    Some(tmp)
}

/// 产物类型（Kotlin `kindFor`）：提交方声明 → 节点类型线索 → 扩展名。
fn kind_for(item: &IngestOutput) -> String {
    if let Some(kind) = item
        .kind
        .as_deref()
        .map(str::to_uppercase)
        .filter(|k| crate::media::media_upload::KINDS.contains(&k.as_str()))
    {
        return kind;
    }
    let node = item.node_type.as_deref().unwrap_or("").to_lowercase();
    let ext = crate::media::media_upload::extension_of(&item.filename);
    if node.contains("audio") && node.contains("save") {
        return "AUDIO".into();
    }
    if node.contains("previewaudio") {
        return "AUDIO".into();
    }
    if node.contains("videocombine") || node.contains("savevideo") || node.contains("savewebm") {
        return "VIDEO".into();
    }
    if node.contains("animated") {
        return "VIDEO".into();
    }
    if AUDIO_EXT.contains(&ext.as_str()) {
        return "AUDIO".into();
    }
    if VIDEO_EXT.contains(&ext.as_str()) {
        return "VIDEO".into();
    }
    if IMAGE_EXT.contains(&ext.as_str()) {
        return "IMAGE".into();
    }
    crate::media::media_upload::kind_of(&item.filename, None)
}

const IMAGE_EXT: [&str; 8] = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];
const VIDEO_EXT: [&str; 6] = ["mp4", "webm", "mov", "mkv", "avi", "m4v"];
const AUDIO_EXT: [&str; 7] = ["mp3", "wav", "flac", "ogg", "m4a", "aac", "opus"];

// ---------------------------------------------------------------------------
//  轮询
// ---------------------------------------------------------------------------

/// 跑一轮轮询：把 `/history` 里还没捕获的完成项收进来（Kotlin `pollOnce`）。
///
/// 返回 `CapturePollResult` 的 JSON：`{ok, checked, newRuns, newMedia, message?}`。
pub async fn poll_once(state: &AppState, config: &CaptureConfig) -> Value {
    let mut result = Map::new();
    result.insert("ok".into(), json!(false));
    result.insert("checked".into(), json!(0));
    result.insert("newRuns".into(), json!(0));
    result.insert("newMedia".into(), json!(0));

    let base = config.comfy_url.trim().trim_end_matches('/').to_string();
    let history = match crate::comfy::comfy_client::get_json(&format!("{base}/history?max_items=100")).await {
        Ok(value) => value,
        Err(message) => {
            result.insert("message".into(), json!(format!("连不上 ComfyUI（{base}）：{message}")));
            return Value::Object(result);
        }
    };
    let Some(entries) = history.as_object() else {
        result.insert("message".into(), json!("ComfyUI /history 返回格式异常"));
        return Value::Object(result);
    };

    let mut checked = 0i64;
    let mut new_runs = 0i64;
    let mut new_media = 0i64;
    // history 按提交顺序返回（旧 → 新），这里**从最新往回**处理
    for (run_key, entry) in entries.iter().rev() {
        if checked >= config.max_per_poll.max(1) {
            break;
        }
        let status_object = entry.get("status").and_then(Value::as_object);
        let completed = status_object
            .and_then(|s| s.get("completed"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let status_str = status_object
            .and_then(|s| s.get("status_str"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let has_outputs = entry
            .get("outputs")
            .and_then(Value::as_object)
            .map(|o| !o.is_empty())
            .unwrap_or(false);
        if !completed && status_str.as_deref() != Some("error") && !has_outputs {
            continue;
        }
        let claim = match begin_run(state, run_key, "ComfyUI", Some(entry), false) {
            Ok(claim) => claim,
            Err(_) => continue,
        };
        if !claim.claimed {
            continue;
        }
        checked += 1;
        let request = build_ingest_request(run_key, entry, config, status_str.as_deref() == Some("error"));
        let outcome = ingest_claimed(state, &request, config).await;
        if outcome.imported > 0 {
            new_runs += 1;
            new_media += outcome.imported;
        }
    }

    result.insert("ok".into(), json!(true));
    result.insert("checked".into(), json!(checked));
    result.insert("newRuns".into(), json!(new_runs));
    result.insert("newMedia".into(), json!(new_media));
    Value::Object(result)
}

// ---------------------------------------------------------------------------
//  小工具
// ---------------------------------------------------------------------------

/// 按**字符**截断（Kotlin 的 `String.take(n)` 是 UTF-16 码元；这里按 Unicode 标量，
/// 差异只在 emoji 上，而这种文本里没有 —— 但**别换成按字节**，那会把汉字切两半）。
fn take_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn push_unique(list: &mut Vec<String>, value: &str) {
    if !list.iter().any(|existing| existing == value) {
        list.push(value.to_string());
    }
}

/// 极简的百分号编码（Kotlin 用 `URLEncoder` + `+` → `%20`）。
fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn random_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:x}{:x}", std::process::id(), nanos)
}

#[cfg(test)]
mod tests;
