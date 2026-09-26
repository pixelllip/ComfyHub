//! AI 附件（M3 / AIH-027 ~ AIH-031）—— Kotlin `AiAttachments.kt` + `AiRoutes` 那五条端点的移植。
//!
//! # 三层职责分得很清楚，改的时候别混
//!
//! | 层 | 这里 | 管什么 |
//! | --- | --- | --- |
//! | DTO | [`AttachmentDto`] | 对外的**客观事实**（名字 / 类型 / 大小 / 尺寸）。**不含存储路径** |
//! | 库 | [`insert`] / [`get`] / [`list`] / [`delete_row`] | 只有数据库那一层 |
//! | 文件 | [`save`] / [`file_of`] / [`thumbnail_of`] / [`delete`] | 落盘、缩略图、读出来做 base64 内联 |
//!
//! # 关键规矩（每条都有理由）
//!
//!  1. **类型判定只认签名**（[`crate::file_kind`]）：认不出来直接拒收，不做"未知即图片"的乐观回退
//!     —— 前端声明的 MIME 只是线索（AIH-027）。
//!  2. **原件与缩略图放在 `<storage>/ai-attachments` 与 `<storage>/ai-thumbs`**，
//!     与画廊产物（`<storage>/media`、`<storage>/thumbs`）**分开**存：
//!     两者的生命周期不同，混在一起以后清孤儿会互相误删。
//!  3. **上传失败绝不留半份**：临时文件在异常路径上一定删掉（库里有行、盘上没文件是另一种麻烦）。
//!  4. **一个都没成 → 400**（界面要能直接看到"为什么这张图发不上去"）；部分成功仍然 201，
//!     失败的逐条列在 `failed` 里（**不静默丢弃**）。
//!
//! ⚠ 与 Kotlin 的一处**诚实差距**：视频预览帧（Kotlin 走 Windows 缩略图管线抽第一帧）在 Rust 侧
//! **还没有**（与 `GET /api/media/{id}/poster` 是同一块缺口）。所以视频附件的 `/thumb` 现在回 **204**，
//! 界面按既有约定退化成文件图标（不是破图）—— **不假装**抽过帧。

use std::path::{Path, PathBuf};

use axum::extract::{Multipart, Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::file_kind::{self, FileKind};
use crate::iso_time::iso_time;
use crate::routes::{ApiError, AppState};

/// 单个附件的上传硬上限（Kotlin `AiAttachmentStore.MAX_UPLOAD_BYTES`）。
pub const MAX_UPLOAD_BYTES: u64 = 32 * 1024 * 1024;

/// 允许前端随预检提交的文件头大小（Kotlin `StrictIntake.MAX_HEAD_BYTES`）。
pub const MAX_HEAD_BYTES: usize = 4096;

/// 内联 base64 的**原始字节**上限（Kotlin `AiAttachmentStore.MAX_INLINE_BYTES`）。
///
/// base64 会把体积放大 1/3，而各网关的请求体上限普遍在 20MB 量级
/// （Anthropic 单图 5MB、OpenAI 单图 20MB）—— 8MB 原图 → ~10.7MB 请求体，
/// 是"能发出去"和"别把网关打挂"之间的平衡点。
pub const MAX_INLINE_BYTES: u64 = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
//  DTO
// ---------------------------------------------------------------------------

/// 一条附件（Kotlin `AiAttachmentDto`）。
///
/// ⚠ 可空字段一律 `skip_serializing_if`（Kotlin `explicitNulls = false`）——
/// 少了它界面上会多出一堆 `null` 键，`tests/shape_lint.rs` 也会红。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentDto {
    pub id: String,
    /// 原始文件名（界面显示用）
    pub name: String,
    /// image / video / audio / document / text
    pub kind: String,
    /// 输入模态，与 `kind` 同义；未知类型不允许入库，所以这里不会是 null
    pub modality: String,
    pub mime_type: String,
    pub size_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// ready / rejected / deleted
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

impl AttachmentDto {
    /// 界面上能不能显示成"图"（图片缩略图 / 视频预览帧）。
    pub fn has_preview(&self) -> bool {
        self.kind == "image" || self.kind == "video"
    }
}

/// 一次上传的结果：`items` 是入库成功的，`failed` 是逐个失败的原因（不静默丢弃）。
#[derive(Debug, Clone, Serialize)]
pub struct UploadResult {
    pub items: Vec<AttachmentDto>,
    pub failed: Vec<FailedInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedInfo {
    pub file_name: String,
    pub reason: String,
}

// ---------------------------------------------------------------------------
//  目录
// ---------------------------------------------------------------------------

/// `<storage>` —— 附件目录的基准（`media_dir` 的父目录）。
pub fn storage_root(state: &AppState) -> PathBuf {
    state
        .media_dir()
        .parent()
        .unwrap_or(state.media_dir())
        .to_path_buf()
}

pub fn attachment_dir(state: &AppState) -> PathBuf {
    storage_root(state).join("ai-attachments")
}

pub fn attachment_thumb_dir(state: &AppState) -> PathBuf {
    storage_root(state).join("ai-thumbs")
}

fn tmp_dir(state: &AppState) -> PathBuf {
    storage_root(state).join("tmp")
}

/// 上传中转文件（放在 `<storage>/tmp`，与画廊上传共用同一套约定）。
pub fn temp_file(state: &AppState) -> std::io::Result<PathBuf> {
    let dir = tmp_dir(state);
    std::fs::create_dir_all(&dir)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Ok(dir.join(format!("upload-{nanos}-{}.part", std::process::id())))
}

/// AI 附件原件的落盘名（与画廊产物同一套命名规则：32 位 hex + 原扩展名）。
fn new_stored_name(original_name: &str) -> String {
    let ext: String = original_name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_lowercase())
        .filter(|ext| !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_default();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mixed = nanos ^ ((std::process::id() as u128) << 96) ^ nanos.rotate_left(29);
    let hex = format!("{mixed:032x}");
    if ext.is_empty() {
        hex
    } else {
        format!("{hex}.{ext}")
    }
}

/// 防目录穿越地解析一个原件路径（找不到文件回 `None`：库里有行、盘上没文件时如实降级）。
pub fn resolve_attachment(state: &AppState, stored_name: &str) -> Option<PathBuf> {
    if stored_name.trim().is_empty() {
        return None;
    }
    let dir = attachment_dir(state);
    if stored_name.contains('/') || stored_name.contains('\\') || stored_name.contains("..") {
        return None;
    }
    let path = dir.join(stored_name);
    path.is_file().then_some(path)
}

pub fn thumb_path(state: &AppState, id: &str) -> PathBuf {
    attachment_thumb_dir(state).join(format!("{id}.jpg"))
}

/// 附件 id 只允许 UUID / 十六进制那种（防穿越：它会被直接拼进路径）。
///
/// ⚠ **查库时不拿它当错误**：Kotlin 的 `AiAttachmentRepo.get(id)` 对一个奇形怪状的 id
/// 只是"查不到"，于是调用方给的是 `附件不存在: xxx`。把非法 id 单独报成另一个错
/// （`非法的附件 id`）会让预检的逐条结论对不上号 —— 所以这里只在**拼路径**的地方用它。
fn safe_path_id(id: &str) -> Option<&str> {
    let trimmed = id.trim();
    let ok = !trimmed.is_empty()
        && trimmed.len() <= 64
        && trimmed.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then_some(trimmed)
}

// ---------------------------------------------------------------------------
//  库
// ---------------------------------------------------------------------------

pub fn insert(state: &AppState, dto: &AttachmentDto, stored_name: &str) -> Result<(), ApiError> {
    state.write(|conn| {
        conn.execute(
            "INSERT INTO ai_attachments \
             (id, original_name, stored_name, kind, mime_type, size_bytes, width, height, sha256, status) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                dto.id,
                dto.name,
                stored_name,
                dto.kind,
                dto.mime_type,
                dto.size_bytes,
                dto.width,
                dto.height,
                dto.sha256,
                dto.status
            ],
        )?;
        Ok(())
    })
}

fn row_to_dto(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttachmentDto> {
    let created: Option<String> = row.get("created_at")?;
    let kind: String = row.get::<_, Option<String>>("kind")?.unwrap_or_else(|| "document".to_string());
    Ok(AttachmentDto {
        id: row.get("id")?,
        name: row
            .get::<_, Option<String>>("original_name")?
            .unwrap_or_else(|| "附件".to_string()),
        modality: kind.clone(),
        kind,
        mime_type: row
            .get::<_, Option<String>>("mime_type")?
            .unwrap_or_else(|| "application/octet-stream".to_string()),
        size_bytes: row.get::<_, Option<i64>>("size_bytes")?.unwrap_or(0),
        width: row.get("width")?,
        height: row.get("height")?,
        sha256: row.get("sha256")?,
        status: row
            .get::<_, Option<String>>("status")?
            .unwrap_or_else(|| "ready".to_string()),
        created_at: iso_time(created.as_deref()),
    })
}

pub fn get(state: &AppState, id: &str) -> Result<Option<AttachmentDto>, ApiError> {
    // 非法 id 与"库里没有"一样回 None（见 `safe_path_id` 的注释）
    let Some(id) = safe_path_id(id) else { return Ok(None) };
    state.with(|conn| {
        Ok(conn
            .query_row("SELECT * FROM ai_attachments WHERE id = ?1", params![id], row_to_dto)
            .optional()?)
    })
}

/// 按 id 批量取，**保持传入顺序**（预检结果要跟界面的顺序一一对应）。
///
/// 找不到的 id 静默跳过 —— 调用方可以用"数量对不对"判断有没有已删除的附件。
pub fn list(state: &AppState, ids: &[String]) -> Result<Vec<AttachmentDto>, ApiError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut found: Vec<AttachmentDto> = Vec::new();
    for id in ids {
        // ⚠ 非法 id（非 ASCII / 太长 / 带斜杠）与"库里没有"一样**静默跳过**：
        // 这条路是预检用的，脏 id 不该把整次预检打成 500 —— 调用方用"数量对不对"判断有没有失效的附件。
        if let Ok(Some(dto)) = get(state, id) {
            found.push(dto);
        }
    }
    // 保持传入顺序（重复的 id 也保持重复，调用方自己 distinct）
    let mut out: Vec<AttachmentDto> = Vec::new();
    for id in ids {
        if let Some(dto) = found.iter().find(|d| &d.id == id) {
            out.push(dto.clone());
        }
    }
    Ok(out)
}

pub fn stored_name(state: &AppState, id: &str) -> Result<Option<String>, ApiError> {
    let Some(id) = safe_path_id(id) else { return Ok(None) };
    state.with(|conn| {
        Ok(conn
            .query_row("SELECT stored_name FROM ai_attachments WHERE id = ?1", params![id], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten())
    })
}

pub fn delete_row(state: &AppState, id: &str) -> Result<bool, ApiError> {
    // 参数化查询，id 原样交给 SQLite（查不到就是 0 行）
    state.write(|conn| Ok(conn.execute("DELETE FROM ai_attachments WHERE id = ?1", params![id.trim()])? > 0))
}

// ---------------------------------------------------------------------------
//  文件那一层
// ---------------------------------------------------------------------------

/// 把一个已落到临时文件的上传收进附件库。
///
/// 失败一律回 `400 UNSUPPORTED_CONTENT`（带人话）并**删掉临时文件**，绝不留半份。
pub fn save(
    state: &AppState,
    name: &str,
    tmp: &Path,
    declared_mime: Option<&str>,
) -> Result<AttachmentDto, ApiError> {
    let result = save_inner(state, name, tmp, declared_mime);
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

fn save_inner(
    state: &AppState,
    name: &str,
    tmp: &Path,
    declared_mime: Option<&str>,
) -> Result<AttachmentDto, ApiError> {
    let trimmed = name.trim();
    let file_name: String = if trimmed.is_empty() { "attachment.bin".to_string() } else { trimmed.to_string() }
        .chars()
        .take(255)
        .collect();
    let size = std::fs::metadata(tmp).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Err(ApiError::ai("UNSUPPORTED_CONTENT", format!("文件 {file_name} 是空的")));
    }
    if size > MAX_UPLOAD_BYTES {
        return Err(ApiError::ai(
            "UNSUPPORTED_CONTENT",
            format!("文件 {file_name} 超过单附件上限（{} MB）", MAX_UPLOAD_BYTES / 1024 / 1024),
        ));
    }
    let head = read_head(tmp, MAX_HEAD_BYTES);
    let detected = file_kind::detect(&head, &file_name, declared_mime);
    if detected.blocked() {
        return Err(ApiError::ai(
            "UNSUPPORTED_CONTENT",
            format!(
                "无法识别文件 {file_name} 的真实类型（签名与声明的 {} 不符），已拒绝上传",
                declared_mime.unwrap_or("未知 MIME")
            ),
        ));
    }

    let dir = attachment_dir(state);
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(format!("建附件目录失败：{e}")))?;
    let stored = new_stored_name(&file_name);
    let dest = dir.join(&stored);
    std::fs::rename(tmp, &dest)
        .or_else(|_| {
            // 跨盘 rename 会失败：退化成"复制 + 删源"
            std::fs::copy(tmp, &dest).map(|_| ()).and_then(|_| std::fs::remove_file(tmp))
        })
        .map_err(|e| ApiError::internal(format!("落盘失败：{e}")))?;

    let dimensions = if detected.kind == FileKind::Image {
        crate::thumbs::probe_image_size(&dest)
    } else {
        None
    };
    let sha = crate::media_upload::sha256_of(&dest).ok();
    let dto = AttachmentDto {
        id: uuid_like(),
        name: file_name,
        kind: detected.kind.wire().to_string(),
        modality: detected
            .kind
            .modality()
            .unwrap_or("document")
            .to_string(),
        mime_type: detected
            .mime_type
            .clone()
            .or_else(|| declared_mime.map(|m| m.to_string()))
            .unwrap_or_else(|| "application/octet-stream".to_string()),
        size_bytes: size as i64,
        width: dimensions.map(|(w, _)| w),
        height: dimensions.map(|(_, h)| h),
        sha256: sha,
        status: "ready".to_string(),
        created_at: None,
    };
    insert(state, &dto, &stored)?;
    // 回读一次：`created_at` 由库里的默认值给（与 Kotlin 的"插完再取"一致）
    get(state, &dto.id)?.ok_or_else(|| ApiError::internal("附件入库后读不回来"))
}

fn read_head(path: &Path, max: usize) -> Vec<u8> {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else { return Vec::new() };
    let mut buffer = vec![0u8; max];
    match file.read(&mut buffer) {
        Ok(read) => buffer.truncate(read),
        Err(_) => buffer.clear(),
    }
    buffer
}

/// 原件路径（找不到文件回 `None`）。
pub fn file_of(state: &AppState, id: &str) -> Result<Option<PathBuf>, ApiError> {
    let Some(stored) = stored_name(state, id)? else { return Ok(None) };
    Ok(resolve_attachment(state, &stored))
}

/// 缩略图 / 视频预览帧：`(路径, Content-Type)`；没有可看的图回 `None`（端点回 204）。
///
/// 生成是**惰性**的：上传时只落原件，第一次请求缩略图时才解码 ——
/// 上传路径不该为一张可能永远没人看的缩略图停下来。
///
/// 图片分支有**两级降级**（与画廊 `/thumb` 同一条规矩）：先试 JPEG 缩略图，
/// 解不了就把**原件当预览发出去**（带它自己的 MIME）——断在这里的话用户只会看到文件图标，
/// 而"图明明在那儿"。
pub fn thumbnail_of(state: &AppState, id: &str) -> Result<Option<(PathBuf, String)>, ApiError> {
    let Some(dto) = get(state, id)? else { return Ok(None) };
    let Some(src) = file_of(state, id)? else { return Ok(None) };
    match dto.kind.as_str() {
        "image" => {
            let dest = thumb_path(state, &dto.id);
            if dest.is_file() {
                return Ok(Some((dest, "image/jpeg".to_string())));
            }
            std::fs::create_dir_all(attachment_thumb_dir(state)).ok();
            match crate::thumbs::write_thumbnail(&src, &dest) {
                Ok(true) if dest.is_file() => Ok(Some((dest, "image/jpeg".to_string()))),
                _ => Ok(Some((src, dto.mime_type))),
            }
        }
        // ⚠ 视频预览帧还没搬（见文件头）：**回 204**，界面退化成文件图标。
        //    这里刻意不去"假装"抽一帧，也不把视频原件当图片发给 <img>。
        "video" => Ok(None),
        _ => Ok(None),
    }
}

pub fn delete(state: &AppState, id: &str) -> Result<bool, ApiError> {
    let stored = stored_name(state, id)?;
    let removed = delete_row(state, id)?;
    if let Some(stored) = stored {
        if let Some(path) = resolve_attachment(state, &stored) {
            let _ = std::fs::remove_file(path);
        }
    }
    // 缩略图只在 id 形状合法时拼路径（防穿越）
    if let Some(safe) = safe_path_id(id) {
        let _ = std::fs::remove_file(thumb_path(state, safe));
    }
    Ok(removed)
}

/// 读成"可以直接内联给模型"的附件（图片才需要 base64）。
///
/// 读不到文件或超过内联上限时回 `None` —— 调用方负责把它变成一条**明确说明**
/// （"这个附件没随本次请求发送"），而不是静默丢掉。
pub fn inline_base64(state: &AppState, dto: &AttachmentDto) -> Option<String> {
    let path = file_of(state, &dto.id).ok().flatten()?;
    let size = std::fs::metadata(&path).ok()?.len();
    if size == 0 || size > MAX_INLINE_BYTES {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    Some(base64_encode(&bytes))
}

/// 标准 base64（不引额外依赖：附件内联只用它一处，20 行足够）。
pub fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let cleaned: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if cleaned.len() % 4 != 0 {
        return None;
    }
    let value_of = |byte: u8| -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some((byte - b'A') as u32),
            b'a'..=b'z' => Some((byte - b'a' + 26) as u32),
            b'0'..=b'9' => Some((byte - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            b'=' => Some(0),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(cleaned.len() / 4 * 3);
    for chunk in cleaned.chunks(4) {
        let mut triple = 0u32;
        for byte in chunk {
            triple = (triple << 6) | value_of(*byte)?;
        }
        out.push((triple >> 16) as u8);
        if chunk[2] != b'=' {
            out.push((triple >> 8) as u8);
        }
        if chunk[3] != b'=' {
            out.push(triple as u8);
        }
    }
    Some(out)
}

fn uuid_like() -> String {
    // 附件 id 走的是**文件系统安全**的一串：UUID v4 形状（Kotlin 用 UUID.randomUUID()）
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mixed = nanos ^ (pid << 64) ^ nanos.rotate_left(23);
    let hex = format!("{mixed:032x}");
    format!("{}-{}-4{}-a{}-{}", &hex[0..8], &hex[8..12], &hex[13..16], &hex[17..20], &hex[20..32])
}

// ---------------------------------------------------------------------------
//  端点
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    // ⚠ 必须写 `axum::routing::get` 的全路径：本模块的仓储函数就叫 `get`
    // （与 Kotlin 同名同义），直接 `use axum::routing::get` 会被同名遮蔽。
    Router::new()
        .route("/api/ai/attachments", axum::routing::post(upload))
        .route(
            "/api/ai/attachments/{id}",
            axum::routing::get(get_one).delete(delete_one),
        )
        .route("/api/ai/attachments/{id}/file", axum::routing::get(get_file))
        .route("/api/ai/attachments/{id}/thumb", axum::routing::get(get_thumb))
        // 附件准入预检（AIH-029）：**纯计算**，不产生任何上游请求
        .route("/api/ai/preflight", axum::routing::post(preflight))
}

// ---------------------------------------------------------------------------
//  准入预检（AIH-029 / AIH-030）
// ---------------------------------------------------------------------------

/// `POST /api/ai/preflight` 的请求体（Kotlin `PreflightRequest`）。
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PreflightRequest {
    pub provider_id: String,
    pub model_id: String,
    /// 前端直接给的线索（可含文件头）；有 `attachmentIds` 时以库里的为准
    pub attachments: Vec<FactInput>,
    /// 已上传附件的 id（M3）：**服务端以库里的事实为准**，前端声明只是线索
    pub attachment_ids: Vec<String>,
}

/// 前端给的附件线索（Kotlin `AttachmentFactDto`）。
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FactInput {
    pub name: String,
    pub modality: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: Option<i64>,
    pub pixels: Option<i64>,
    /// 文件头（base64，最多 [`crate::strict_intake::MAX_HEAD_BYTES`] 字节）
    pub head_base64: Option<String>,
}

fn fact_of(dto: &AttachmentDto) -> crate::strict_intake::AttachmentFact {
    crate::strict_intake::AttachmentFact {
        name: dto.name.clone(),
        modality: Some(dto.modality.clone()),
        mime_type: dto.mime_type.clone(),
        size_bytes: dto.size_bytes,
        pixels: match (dto.width, dto.height) {
            (Some(w), Some(h)) => Some(w * h),
            _ => None,
        },
    }
}

/// `POST /api/ai/preflight`：逐个附件的准入结论（界面用它给缩略图打红框 / 显示原因）。
pub async fn preflight_json(state: &AppState, body: &PreflightRequest) -> Result<Value, ApiError> {
    let provider = crate::ai::require_provider_row(state, &body.provider_id)?;
    let model = crate::ai::list_models(state, &body.provider_id)?
        .into_iter()
        .find(|m| m.id == body.model_id)
        .ok_or_else(|| {
            ApiError::ai("UNKNOWN_MODEL", format!("模型不存在于目录中: {}", body.model_id))
        })?;

    // 与 Kotlin `attachmentIds.distinct()` 一样**保序去重**
    let mut ids: Vec<String> = Vec::new();
    for id in &body.attachment_ids {
        let trimmed = id.trim().to_string();
        if !trimmed.is_empty() && !ids.contains(&trimmed) {
            ids.push(trimmed);
        }
    }

    struct Item {
        fact: crate::strict_intake::AttachmentFact,
        resolver_blockers: Vec<String>,
        attachment_id: Option<String>,
    }
    let mut items: Vec<Item> = Vec::new();
    for id in &ids {
        match get(state, id)? {
            Some(dto) => items.push(Item {
                fact: fact_of(&dto),
                resolver_blockers: Vec::new(),
                attachment_id: Some(id.clone()),
            }),
            None => items.push(Item {
                fact: crate::strict_intake::AttachmentFact {
                    name: id.clone(),
                    modality: None,
                    mime_type: "application/octet-stream".to_string(),
                    size_bytes: 0,
                    pixels: None,
                },
                resolver_blockers: vec![format!("附件不存在或已被删除：{id}")],
                attachment_id: Some(id.clone()),
            }),
        }
    }
    for input in &body.attachments {
        let head = input.head_base64.as_deref().and_then(base64_decode);
        if let Some(bytes) = head.as_ref() {
            if bytes.len() > crate::strict_intake::MAX_HEAD_BYTES {
                return Err(ApiError::bad_request(format!(
                    "文件头太大（{} 字节 > {}）",
                    bytes.len(),
                    crate::strict_intake::MAX_HEAD_BYTES
                )));
            }
        }
        let name = if input.name.trim().is_empty() { "附件" } else { input.name.trim() };
        let mime = input
            .mime_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let resolved = crate::strict_intake::resolve(
            name,
            input.modality.as_deref(),
            &mime,
            input.size_bytes.unwrap_or(0),
            input.pixels,
            head.as_deref(),
        );
        items.push(Item {
            resolver_blockers: resolved.blockers,
            fact: resolved.fact,
            attachment_id: None,
        });
    }

    let mut out_items: Vec<Value> = Vec::new();
    let mut all_blockers: Vec<String> = Vec::new();
    let mut allowed_all = true;
    for (index, item) in items.iter().enumerate() {
        // 严格分类器都在这一层跑：前端谎报 image 骗不过准入（AIH-027）
        let transports = crate::strict_intake::adapter_transports(
            &provider.api,
            item.fact.modality.as_deref().unwrap_or_default(),
        );
        let admission = crate::strict_intake::evaluate(&model, &item.fact, &transports, index as i64);
        // 文件头判定失败的阻断理由必须一起带出来（AIH-027/030）
        let mut blockers = item.resolver_blockers.clone();
        blockers.extend(admission.blockers);
        if !blockers.is_empty() {
            allowed_all = false;
        }
        all_blockers.extend(blockers.iter().cloned());
        let mut entry = Map::new();
        entry.insert("index".into(), Value::Number((index as i64).into()));
        entry.insert("name".into(), Value::String(item.fact.name.clone()));
        entry.insert("allowed".into(), Value::Bool(blockers.is_empty()));
        entry.insert("blockers".into(), Value::Array(blockers.into_iter().map(Value::String).collect()));
        if let Some(id) = item.attachment_id.as_ref() {
            entry.insert("attachmentId".into(), Value::String(id.clone()));
        }
        out_items.push(Value::Object(entry));
    }

    let mut out = Map::new();
    out.insert("allowed".into(), Value::Bool(allowed_all));
    out.insert(
        "blockers".into(),
        Value::Array(all_blockers.into_iter().map(Value::String).collect()),
    );
    out.insert("items".into(), Value::Array(out_items));
    Ok(Value::Object(out))
}

async fn preflight(
    State(state): State<AppState>,
    Json(body): Json<PreflightRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(preflight_json(&state, &body).await?))
}

const FIELD_HINT: &str = "没有收到文件（表单字段名请用 files）";

/// `POST /api/ai/attachments`（multipart，字段名 `files`）。
async fn upload(State(state): State<AppState>, mut multipart: Multipart) -> Result<Response, ApiError> {
    let mut incoming: Vec<(String, PathBuf, Option<String>)> = Vec::new();
    let mut failed: Vec<FailedInfo> = Vec::new();

    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field
                    .file_name()
                    .map(|n| n.to_string())
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| "attachment.bin".to_string());
                let declared = field.content_type().map(|m| m.to_string());
                let tmp = temp_file(&state).map_err(|e| ApiError::internal(format!("建临时文件失败：{e}")))?;
                match field.bytes().await {
                    Ok(bytes) => {
                        if bytes.len() as u64 > MAX_UPLOAD_BYTES {
                            let _ = std::fs::remove_file(&tmp);
                            failed.push(FailedInfo {
                                file_name: name.clone(),
                                reason: format!("文件 {name} 超过单附件上限（{} MB）", MAX_UPLOAD_BYTES / 1024 / 1024),
                            });
                            continue;
                        }
                        if let Err(e) = std::fs::write(&tmp, &bytes) {
                            let _ = std::fs::remove_file(&tmp);
                            failed.push(FailedInfo { file_name: name, reason: format!("写临时文件失败：{e}") });
                            continue;
                        }
                        incoming.push((name, tmp, declared));
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&tmp);
                        failed.push(FailedInfo { file_name: name, reason: format!("读取上传内容失败：{e}") });
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                for (_, tmp, _) in incoming.iter() {
                    let _ = std::fs::remove_file(tmp);
                }
                return Err(ApiError::ai("CONFIG_ERROR", format!("附件上传解析失败：{e}")));
            }
        }
    }

    if incoming.is_empty() && failed.is_empty() {
        return Err(ApiError::ai("CONFIG_ERROR", FIELD_HINT));
    }

    let mut items: Vec<AttachmentDto> = Vec::new();
    for (name, tmp, declared) in incoming {
        match save(&state, &name, &tmp, declared.as_deref()) {
            Ok(dto) => items.push(dto),
            Err(error) => {
                // 一个都没成 → 400（Kotlin 同）：界面要能直接看到"为什么这张图发不上去"
                failed.push(FailedInfo {
                    file_name: name.clone(),
                    reason: error.human().to_string(),
                });
            }
        }
    }
    if items.is_empty() {
        let message = if failed.is_empty() {
            "附件入库失败".to_string()
        } else {
            failed
                .iter()
                .map(|f| format!("{}：{}", f.file_name, f.reason))
                .collect::<Vec<_>>()
                .join("；")
        };
        return Err(ApiError::ai("UNSUPPORTED_CONTENT", message));
    }
    Ok((StatusCode::CREATED, Json(UploadResult { items, failed })).into_response())
}

async fn get_one(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<AttachmentDto>, ApiError> {
    let dto = get(&state, &id)?
        .ok_or_else(|| ApiError::ai("CONFIG_ERROR", format!("附件不存在: {id}")))?;
    Ok(Json(dto))
}

async fn get_file(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let dto = get(&state, &id)?
        .ok_or_else(|| ApiError::ai("CONFIG_ERROR", format!("附件不存在: {id}")))?;
    let path = file_of(&state, &id)?
        .ok_or_else(|| ApiError::ai("CONFIG_ERROR", format!("附件文件已丢失: {}", dto.name)))?;
    crate::media::serve_path(
        &path,
        dto.sha256.as_deref(),
        &dto.mime_type,
        &dto.name,
        &headers,
    )
    .await
}

/// 缩略图 / 视频预览帧（**同一张接口**）；没有可看的图回 **204**。
async fn get_thumb(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, ApiError> {
    let _ = get(&state, &id)?.ok_or_else(|| ApiError::ai("CONFIG_ERROR", format!("附件不存在: {id}")))?;
    let thumb = tokio::task::spawn_blocking({
        let state = state.clone();
        let id = id.clone();
        move || thumbnail_of(&state, &id)
    })
    .await
    .map_err(|e| ApiError::internal(format!("缩略图任务失败：{e}")))??;
    let Some((path, content_type)) = thumb else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    let mut response = crate::media::serve_path(&path, None, &content_type, &format!("thumb-{id}"), &HeaderMap::new())
        .await?;
    response.headers_mut().insert(
        "x-thumbnail",
        axum::http::HeaderValue::from_static("1"),
    );
    Ok(response)
}

async fn delete_one(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Value>, ApiError> {
    let deleted = delete(&state, &id)?;
    if !deleted {
        return Err(ApiError::ai("CONFIG_ERROR", format!("附件不存在: {id}")));
    }
    let mut out = Map::new();
    out.insert("deleted".into(), Value::Bool(true));
    out.insert("id".into(), Value::String(id));
    Ok(Json(Value::Object(out)))
}



// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn state(label: &str) -> (AppState, PathBuf) {
        let dir = std::env::temp_dir().join(format!("comfyhub-att-{label}-{}", uuid_like()));
        std::fs::create_dir_all(dir.join("media")).unwrap();
        let conn = crate::db::open_memory().unwrap();
        let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql");
        let schema = std::fs::read_to_string(&schema_path).unwrap();
        conn.execute_batch(&schema).unwrap();
        let state = AppState::new(conn, ":memory:", dir.join("media"));
        (state, dir)
    }

    fn png_bytes() -> Vec<u8> {
        let mut image = image::RgbImage::new(6, 4);
        for pixel in image.pixels_mut() {
            *pixel = image::Rgb([9, 200, 30]);
        }
        let mut buffer = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut buffer, image::ImageFormat::Png)
            .unwrap();
        buffer.into_inner()
    }

    #[test]
    fn 图片附件入库_尺寸与缩略图都对() {
        let (state, dir) = state("png");
        let tmp = temp_file(&state).unwrap();
        std::fs::write(&tmp, png_bytes()).unwrap();
        let dto = save(&state, "参考图.png", &tmp, Some("image/png")).unwrap();
        assert_eq!(dto.kind, "image");
        assert_eq!(dto.modality, "image");
        assert_eq!(dto.mime_type, "image/png");
        assert_eq!((dto.width, dto.height), (Some(6), Some(4)), "要探测图片尺寸");
        assert!(dto.sha256.is_some());
        assert!(dto.created_at.is_some(), "创建时间由库里给");
        assert!(dto.has_preview());
        assert!(!tmp.exists(), "临时文件要被移走");

        // 回读一次形状（键集合照 Kotlin：可空键不出现）
        let json = serde_json::to_value(&dto).unwrap();
        assert!(json.as_object().unwrap().contains_key("mimeType"));
        assert!(json.as_object().unwrap().contains_key("sizeBytes"));
        assert!(!json.as_object().unwrap().contains_key("hasPreview"), "计算属性不序列化");

        // 惰性缩略图
        let (thumb, mime) = thumbnail_of(&state, &dto.id).unwrap().expect("图片要有缩略图");
        assert!(thumb.is_file());
        assert_eq!(mime, "image/jpeg");

        // 原件能读回来做内联
        let encoded = inline_base64(&state, &dto).expect("8MB 以内要能内联");
        assert!(encoded.starts_with("iVBORw0KGgo"), "PNG 的 base64 前缀");
        assert_eq!(base64_decode(&encoded).unwrap(), std::fs::read(file_of(&state, &dto.id).unwrap().unwrap()).unwrap());

        // 删除连原件与缩略图一起清
        assert!(delete(&state, &dto.id).unwrap());
        assert!(get(&state, &dto.id).unwrap().is_none());
        assert!(!thumb.exists(), "缩略图也要删掉");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn 认不出来的类型一律拒收且不留半份() {
        let (state, dir) = state("reject");
        let tmp = temp_file(&state).unwrap();
        std::fs::write(&tmp, [0x00, 0x01, 0x02, 0x03, 0xFF]).unwrap();
        let error = save(&state, "evil.png", &tmp, Some("image/png")).expect_err("必须拒");
        assert_eq!(error.error, "UNSUPPORTED_CONTENT");
        assert!(error.human().contains("无法识别文件 evil.png"), "{}", error.human());
        assert!(!tmp.exists(), "被拒的临时文件必须删掉");
        // 空文件同样拒
        let tmp = temp_file(&state).unwrap();
        std::fs::write(&tmp, b"").unwrap();
        let error = save(&state, "empty.txt", &tmp, None).expect_err("空文件必须拒");
        assert!(error.human().contains("是空的"), "{}", error.human());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn 文本附件入库但没有预览() {
        let (state, dir) = state("text");
        let tmp = temp_file(&state).unwrap();
        std::fs::write(&tmp, b"# hello\nworld\n").unwrap();
        let dto = save(&state, "notes.md", &tmp, Some("text/markdown")).unwrap();
        assert_eq!(dto.kind, "text");
        assert_eq!(dto.mime_type, "text/markdown");
        assert!(!dto.has_preview());
        assert!(thumbnail_of(&state, &dto.id).unwrap().is_none(), "文本没有画面");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn 视频附件回_204_而不是假装抽过帧() {
        let (state, dir) = state("video");
        let tmp = temp_file(&state).unwrap();
        let mut bytes = vec![0, 0, 0, 0x20];
        bytes.extend_from_slice(b"ftypisom");
        bytes.extend_from_slice(&[0; 8]);
        std::fs::write(&tmp, &bytes).unwrap();
        let dto = save(&state, "clip.mp4", &tmp, Some("video/mp4")).unwrap();
        assert_eq!(dto.kind, "video");
        assert!(dto.has_preview(), "界面会去要封面");
        assert!(
            thumbnail_of(&state, &dto.id).unwrap().is_none(),
            "预览帧还没搬：如实回 null（端点 204），不假装"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn 按_id_批量取要保持顺序() {
        let (state, dir) = state("list");
        let mut ids = Vec::new();
        for (index, name) in ["a.md", "b.md", "c.md"].iter().enumerate() {
            let tmp = temp_file(&state).unwrap();
            std::fs::write(&tmp, format!("# doc {index}").as_bytes()).unwrap();
            ids.push(save(&state, name, &tmp, None).unwrap().id);
        }
        let reordered = vec![ids[2].clone(), ids[0].clone(), "不存在".to_string()];
        let found = list(&state, &reordered).unwrap();
        assert_eq!(found.len(), 2, "找不到的 id 静默跳过");
        assert_eq!(found[0].id, ids[2], "顺序必须跟传入一致");
        assert_eq!(found[1].id, ids[0]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn 非法_id_与目录穿越都挡住() {
        let (state, dir) = state("safe");
        // 非法 id 与"查不到"同一条路（Kotlin 同）：**不是**另一个错误
        assert!(get(&state, "../../etc/passwd").unwrap().is_none());
        assert!(stored_name(&state, "../../etc/passwd").unwrap().is_none());
        assert!(delete(&state, "../../etc/passwd").unwrap() == false);
        assert!(resolve_attachment(&state, "../media/x.png").is_none());
        assert!(resolve_attachment(&state, "sub/x.png").is_none());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn base64_编解码往返() {
        for case in [b"".to_vec(), b"a".to_vec(), b"ab".to_vec(), b"abc".to_vec(), vec![0u8, 255, 128, 7]] {
            let encoded = base64_encode(&case);
            assert_eq!(base64_decode(&encoded).unwrap(), case, "{encoded}");
        }
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert!(base64_decode("not base64!!").is_none());
    }
}
