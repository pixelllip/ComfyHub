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
//!  1. **类型判定只认签名**（[`crate::media::file_kind`]）：认不出来直接拒收，不做"未知即图片"的乐观回退
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

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use axum::extract::{Multipart, Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::infra::iso_time::iso_time;
use crate::media::file_kind::{self, FileKind};
use crate::web::routes::{ApiError, AppState};

/// 单个附件的上传硬上限（Kotlin `AiAttachmentStore.MAX_UPLOAD_BYTES`）。
pub const MAX_UPLOAD_BYTES: u64 = 32 * 1024 * 1024;

/// `POST /api/ai/attachments` 整个请求体的上限（**不是**单文件上限，见下）。
///
/// ⚠ 这个常量是**补一个真实漏洞**（2026-09-27 用户报"上传不了图片附件"）：
/// axum 的 `DefaultBodyLimit` 默认只有 **2MB**，而 Kotlin 那边是
/// `call.receiveMultipart(formFieldLimit = MAX_UPLOAD_BYTES)` —— **32MB**。
/// 搬过来时漏了这一层，于是"选一张手机拍的照片（4MB）→ 传不上去"：
/// 请求体还没进到 `upload()` 就被截断，客户端拿到的是
/// `附件上传解析失败：Error parsing multipart/form-data request`（curl）/
/// `SocketException ... errno = 10053`（Dart 的 chunked 发送被中途掐断）。
///
/// 为什么取 `MAX_UPLOAD_BYTES` 的若干倍而不是等于它：`DefaultBodyLimit` 管的是
/// **整个请求体**（所有字段加起来），而 App 的选文件对话框允许**一次选多张**。
/// 单文件仍然由 `MAX_UPLOAD_BYTES` 逐条判定（超了给的是那句能看懂的人话）。
///
/// ⚠ 为什么不能直接照搬 Kotlin 的 4GB（`/api/media/upload` 那档）：Rust 侧
/// `field.bytes()` 会把**整个字段读进内存**，Kotlin 是流式落盘 ——
/// 上限给到 GB 级等于给自己埋一个 OOM。要真正对齐得先把落盘改成流式（未做）。
pub const MAX_BODY_BYTES: usize = (MAX_UPLOAD_BYTES as usize) * 4;

/// 允许前端随预检提交的文件头大小（Kotlin `StrictIntake.MAX_HEAD_BYTES`）。
pub const MAX_HEAD_BYTES: usize = 4096;

/// 内联 base64 的**原始字节**上限（Kotlin `AiAttachmentStore.MAX_INLINE_BYTES`）。
///
/// base64 会把体积放大 1/3，而各网关的请求体上限普遍在 20MB 量级
/// （Anthropic 单图 5MB、OpenAI 单图 20MB）—— 8MB 原图 → ~10.7MB 请求体，
/// 是"能发出去"和"别把网关打挂"之间的平衡点。
pub const MAX_INLINE_BYTES: u64 = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
//  附件缓存（2026-11-04）：裁图 / 搜图 / 上传都落成附件，不能无限长
// ---------------------------------------------------------------------------
//
// 三条纪律：
//  1. **被引用的永远不删**：消息块里的 `attachment_id`、提问卡的 `image_url` /
//     `option_images_json` 都算引用 —— 删了历史里的图就变成破图。
//  2. **没被引用的留 7 天**：草稿（SharedPreferences 里的附件 id）不在库里，
//     所以不能"一没引用就删"；7 天是给用户回来继续那条草稿的余量。
//  3. **总量 2GB 封顶，只淘汰"没被引用且超过 1 小时"的旧图**：1 小时的宽限是
//     防"搜图刚返回 id、AI 还没把它写进选项"那一瞬间被 GC 抢走。
/// 未被引用的附件保留天数（草稿宽限）。
pub const GC_GRACE_DAYS: i64 = 7;
/// 附件缓存总预算（2 GB）。超出后淘汰**未被引用**的旧图到 80%。
pub const MAX_CACHE_BYTES: i64 = 2 * 1024 * 1024 * 1024;
/// 超预算淘汰的最小年龄（秒）：防与"刚搜索完、还没写进提问"抢同一个附件。
const GC_OVER_CAP_MIN_AGE_SECONDS: i64 = 3600;
/// 孤儿文件（盘上有、库里没有）的最小年龄：上传是"先落盘再入库"，别删到半成品。
const GC_ORPHAN_MIN_AGE_SECONDS: i64 = 3600;

/// 附件缓存统计（`GET /api/ai/attachments/stats`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheStats {
    pub count: i64,
    pub bytes: i64,
    pub max_bytes: i64,
    pub grace_days: i64,
    /// 按来源分组：upload / crop / search（老数据是 upload）。
    pub by_origin: Vec<OriginStats>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginStats {
    pub origin: String,
    pub count: i64,
    pub bytes: i64,
}

/// 一次 GC 的结果（`POST /api/ai/attachments/gc`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GcReport {
    pub removed: i64,
    pub freed_bytes: i64,
    pub kept_referenced: i64,
    pub orphan_files: i64,
    pub remaining: i64,
    pub remaining_bytes: i64,
}

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
        .filter(|ext| {
            !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric())
        })
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
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then_some(trimmed)
}

// ---------------------------------------------------------------------------
//  库
// ---------------------------------------------------------------------------

pub fn insert(
    state: &AppState,
    dto: &AttachmentDto,
    stored_name: &str,
    origin: &str,
) -> Result<(), ApiError> {
    state.write(|conn| {
        conn.execute(
            "INSERT INTO ai_attachments \
             (id, original_name, stored_name, kind, mime_type, size_bytes, width, height, sha256,
              status, origin) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
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
                dto.status,
                origin
            ],
        )?;
        Ok(())
    })
}

fn row_to_dto(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttachmentDto> {
    let created: Option<String> = row.get("created_at")?;
    let kind: String = row
        .get::<_, Option<String>>("kind")?
        .unwrap_or_else(|| "document".to_string());
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
    let Some(id) = safe_path_id(id) else {
        return Ok(None);
    };
    state.with(|conn| {
        conn.query_row(
            "SELECT * FROM ai_attachments WHERE id = ?1",
            params![id],
            row_to_dto,
        )
        .optional()
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
    let Some(id) = safe_path_id(id) else {
        return Ok(None);
    };
    state.with(|conn| {
        Ok(conn
            .query_row(
                "SELECT stored_name FROM ai_attachments WHERE id = ?1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    })
}

pub fn delete_row(state: &AppState, id: &str) -> Result<bool, ApiError> {
    // 参数化查询，id 原样交给 SQLite（查不到就是 0 行）
    state.write(|conn| {
        Ok(conn.execute(
            "DELETE FROM ai_attachments WHERE id = ?1",
            params![id.trim()],
        )? > 0)
    })
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
    save_with_origin(state, name, tmp, declared_mime, "upload")
}

/// 与 [`save`] 同一条链，只是把来源记下来（upload / crop / search）。
///
/// 来源只用于缓存统计与 GC：**用户上传的图**哪怕一时没被引用也留得久一点，
/// 裁图 / 搜图这类"过程图"可以更早清掉。
pub fn save_with_origin(
    state: &AppState,
    name: &str,
    tmp: &Path,
    declared_mime: Option<&str>,
    origin: &str,
) -> Result<AttachmentDto, ApiError> {
    let result = save_inner(state, name, tmp, declared_mime, origin);
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
    origin: &str,
) -> Result<AttachmentDto, ApiError> {
    let trimmed = name.trim();
    let file_name: String = if trimmed.is_empty() {
        "attachment.bin".to_string()
    } else {
        trimmed.to_string()
    }
    .chars()
    .take(255)
    .collect();
    let size = std::fs::metadata(tmp).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Err(ApiError::ai(
            "UNSUPPORTED_CONTENT",
            format!("文件 {file_name} 是空的"),
        ));
    }
    if size > MAX_UPLOAD_BYTES {
        return Err(ApiError::ai(
            "UNSUPPORTED_CONTENT",
            format!(
                "文件 {file_name} 超过单附件上限（{} MB）",
                MAX_UPLOAD_BYTES / 1024 / 1024
            ),
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
    std::fs::create_dir_all(&dir)
        .map_err(|e| ApiError::internal(format!("建附件目录失败：{e}")))?;
    let stored = new_stored_name(&file_name);
    let dest = dir.join(&stored);
    std::fs::rename(tmp, &dest)
        .or_else(|_| {
            // 跨盘 rename 会失败：退化成"复制 + 删源"
            std::fs::copy(tmp, &dest)
                .map(|_| ())
                .and_then(|_| std::fs::remove_file(tmp))
        })
        .map_err(|e| ApiError::internal(format!("落盘失败：{e}")))?;

    let dimensions = if detected.kind == FileKind::Image {
        crate::media::thumbs::probe_image_size(&dest)
    } else {
        None
    };
    let sha = crate::media::media_upload::sha256_of(&dest).ok();
    let dto = AttachmentDto {
        id: uuid_like(),
        name: file_name,
        kind: detected.kind.wire().to_string(),
        modality: detected.kind.modality().unwrap_or("document").to_string(),
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
    insert(state, &dto, &stored, origin)?;
    // 回读一次：`created_at` 由库里的默认值给（与 Kotlin 的"插完再取"一致）
    get(state, &dto.id)?.ok_or_else(|| ApiError::internal("附件入库后读不回来"))
}

/// 附件的 HTTP 原址（相对后端根）。
///
/// 与 [`router`] 里那条路由**必须同源**：提问卡里的裁图、选项示意图都存这个地址，
/// 前端拿到之后只补 `baseUrl`，不再自己拼一道路径。
pub fn file_url(id: &str) -> String {
    format!("/api/ai/attachments/{id}/file")
}

/// 归一化裁剪区（`"x,y,w,h"`，0~1，左上角原点）。
///
/// 只认逗号分隔的 4 个浮点；解析失败 / 非有限值 / 退化成 0 面积一律 `None`
/// （调用方报参数错，**不要**悄悄当成整张图 —— 那样用户看到的裁图与问题对不上）。
pub fn parse_region(raw: &str) -> Option<(f64, f64, f64, f64)> {
    let parts: Vec<f64> = raw
        .split(',')
        .map(|part| part.trim().parse::<f64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if parts.len() != 4 || parts.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let (x, y, w, h) = (parts[0], parts[1], parts[2], parts[3]);
    let x = x.clamp(0.0, 1.0);
    let y = y.clamp(0.0, 1.0);
    let w = w.clamp(0.0, 1.0 - x);
    let h = h.clamp(0.0, 1.0 - y);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some((x, y, w, h))
}

/// 从一张**图片附件**上裁一块，存成一条新的附件（提问卡 / 选项示意图用）。
///
/// 为什么裁图要在后端：模型只给归一化框，前端不碰解码；裁出来的图必须是一条
/// **真附件**（有 id、有缩略图、能进历史消息），否则重开 App 那张图就没了。
///
/// 参数校验与失败都返回人话（模型据此改参数），不静默退化成整张图。
pub fn crop_image(
    state: &AppState,
    source_id: &str,
    region: &str,
    display_name: &str,
) -> Result<AttachmentDto, ApiError> {
    let Some((x, y, w, h)) = parse_region(region) else {
        return Err(ApiError::ai(
            "INVALID_ARGUMENT",
            format!("裁剪区域必须是 \"x,y,w,h\" 四个 0~1 的数：{region}"),
        ));
    };
    let source = get(state, source_id)?.ok_or_else(|| {
        ApiError::ai(
            "INVALID_ARGUMENT",
            format!("要裁图的附件不存在：{source_id}"),
        )
    })?;
    if source.kind != "image" {
        return Err(ApiError::ai(
            "INVALID_ARGUMENT",
            format!("只能从图片附件上裁图（{} 是 {}）", source.name, source.kind),
        ));
    }
    let src = file_of(state, source_id)?
        .ok_or_else(|| ApiError::ai("CONFIG_ERROR", format!("附件文件已丢失：{}", source.name)))?;
    let image = image::open(&src)
        .map_err(|e| ApiError::ai("UNSUPPORTED_CONTENT", format!("这张图打不开：{e}")))?;
    let (iw, ih) = (image.width() as f64, image.height() as f64);
    if iw < 1.0 || ih < 1.0 {
        return Err(ApiError::ai("UNSUPPORTED_CONTENT", "这张图没有可裁的像素"));
    }
    let left = (x * iw).floor().clamp(0.0, iw - 1.0) as u32;
    let top = (y * ih).floor().clamp(0.0, ih - 1.0) as u32;
    let right = ((x + w) * iw).ceil().clamp((left + 1) as f64, iw) as u32;
    let bottom = ((y + h) * ih).ceil().clamp((top + 1) as f64, ih) as u32;
    let crop = image.crop_imm(left, top, right - left, bottom - top);
    let tmp = temp_file(state).map_err(|e| ApiError::internal(format!("建临时文件失败：{e}")))?;
    if let Err(e) = crop.save_with_format(&tmp, image::ImageFormat::Png) {
        let _ = std::fs::remove_file(&tmp);
        return Err(ApiError::internal(format!("裁图落盘失败：{e}")));
    }
    let name = if display_name.trim().is_empty() {
        format!("{}（局部）.png", source.name)
    } else {
        display_name.trim().to_string()
    };
    save_with_origin(state, &name, &tmp, Some("image/png"), "crop")
}

/// 把一段**已经在内存里**的字节收进附件库（下载来的参考图用）。
///
/// 与上传那条路走**同一个** [`save`]：类型判定、尺寸探测、sha256、临时文件清理
/// 全都复用，不为"网络下载"另开一套落盘逻辑。
pub fn save_bytes(
    state: &AppState,
    name: &str,
    bytes: &[u8],
    declared_mime: Option<&str>,
    origin: &str,
) -> Result<AttachmentDto, ApiError> {
    let tmp = temp_file(state).map_err(|e| ApiError::internal(format!("建临时文件失败：{e}")))?;
    if let Err(e) = std::fs::write(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(ApiError::internal(format!("写临时文件失败：{e}")));
    }
    save_with_origin(state, name, &tmp, declared_mime, origin)
}

fn read_head(path: &Path, max: usize) -> Vec<u8> {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut buffer = vec![0u8; max];
    match file.read(&mut buffer) {
        Ok(read) => buffer.truncate(read),
        Err(_) => buffer.clear(),
    }
    buffer
}

/// 原件路径（找不到文件回 `None`）。
pub fn file_of(state: &AppState, id: &str) -> Result<Option<PathBuf>, ApiError> {
    let Some(stored) = stored_name(state, id)? else {
        return Ok(None);
    };
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
    let Some(dto) = get(state, id)? else {
        return Ok(None);
    };
    let Some(src) = file_of(state, id)? else {
        return Ok(None);
    };
    match dto.kind.as_str() {
        "image" => {
            let dest = thumb_path(state, &dto.id);
            if dest.is_file() {
                return Ok(Some((dest, "image/jpeg".to_string())));
            }
            std::fs::create_dir_all(attachment_thumb_dir(state)).ok();
            match crate::media::thumbs::write_thumbnail(&src, &dest) {
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

// ---------------------------------------------------------------------------
//  附件缓存：统计 + GC
// ---------------------------------------------------------------------------

/// 从附件地址里抠出 id（`/api/ai/attachments/<id>/file`，允许绝对地址）。
///
/// 只认我们自己的路径：`image_url` / `option_images_json` 里也可能有公网示例图，
/// 那些不是本机附件，不该进引用集合。
fn attachment_id_from_url(url: &str) -> Option<String> {
    let marker = "/api/ai/attachments/";
    let index = url.find(marker)?;
    let rest = &url[index + marker.len()..];
    let id = rest.split('/').next()?.trim();
    if id.is_empty() || id.len() > 64 || id.contains("..") {
        return None;
    }
    Some(id.to_string())
}

/// `option_images_json` 里的地址（字符串或 `{url}` 对象都认）。
fn option_image_urls(raw: Option<&str>) -> Vec<String> {
    let Some(text) = raw.map(str::trim).filter(|t| !t.is_empty()) else {
        return Vec::new();
    };
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| match item {
            Value::String(url) => Some(url),
            Value::Object(map) => map.get("url").and_then(Value::as_str).map(str::to_string),
            _ => None,
        })
        .collect()
}

/// 现在**还被历史引用**的附件 id。
///
/// 两处真源：消息块里的 `attachment_id`（用户上传）与提问卡里的 `image_url` /
/// `option_images_json`（裁图 / 选项示意图）。工具结果 json_payload 里出现过、
/// 但没进任何提问的参考图**故意不算引用** —— 那种图没在界面上出现过，可以清。
fn referenced_ids(state: &AppState) -> Result<HashSet<String>, ApiError> {
    state.with(|conn| {
        let mut out = HashSet::new();
        let mut parts = conn.prepare(
            "SELECT attachment_id FROM ai_message_parts
              WHERE attachment_id IS NOT NULL AND attachment_id <> ''",
        )?;
        let rows = parts.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            out.insert(row?);
        }
        let mut questions =
            conn.prepare("SELECT image_url, option_images_json FROM ai_questions")?;
        let rows = questions.query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })?;
        for row in rows {
            let (image, options) = row?;
            if let Some(url) = image.as_deref() {
                if let Some(id) = attachment_id_from_url(url) {
                    out.insert(id);
                }
            }
            for url in option_image_urls(options.as_deref()) {
                if let Some(id) = attachment_id_from_url(&url) {
                    out.insert(id);
                }
            }
        }
        Ok(out)
    })
}

/// 附件缓存统计（数量 / 字节 / 按来源分组）。
pub fn cache_stats(state: &AppState) -> Result<CacheStats, ApiError> {
    state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT origin, COUNT(*), COALESCE(SUM(size_bytes), 0)
               FROM ai_attachments GROUP BY origin ORDER BY origin",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(OriginStats {
                origin: row
                    .get::<_, Option<String>>(0)?
                    .unwrap_or_else(|| "upload".into()),
                count: row.get(1)?,
                bytes: row.get(2)?,
            })
        })?;
        let mut by_origin = Vec::new();
        let mut count = 0i64;
        let mut bytes = 0i64;
        for row in rows {
            let row = row?;
            count += row.count;
            bytes += row.bytes;
            by_origin.push(row);
        }
        Ok(CacheStats {
            count,
            bytes,
            max_bytes: MAX_CACHE_BYTES,
            grace_days: GC_GRACE_DAYS,
            by_origin,
        })
    })
}

/// 清掉**没被引用**的旧附件（启动时跑一次；界面上的「清理未引用」也走它）。
///
/// 顺序：① 超 7 天且没引用 → 删；② 总量超 2GB → 从最老的开始删没引用且超 1 小时的，
/// 直到 80%；③ 盘上多出来的孤儿文件（上传中断 / 入库失败留下的）也一并收掉。
/// 被引用的**一个都不删** —— 历史里的图比省几 MB 重要。
pub fn collect_garbage(state: &AppState) -> Result<GcReport, ApiError> {
    let referenced = referenced_ids(state)?;
    struct Row {
        id: String,
        size: i64,
        old: bool,
        stale: bool,
    }
    let rows: Vec<Row> = state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, size_bytes,
                    CASE WHEN datetime(created_at) < datetime('now', ?1) THEN 1 ELSE 0 END,
                    CASE WHEN datetime(created_at) < datetime('now', ?2) THEN 1 ELSE 0 END
               FROM ai_attachments ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![
                format!("-{GC_GRACE_DAYS} days"),
                format!("-{GC_OVER_CAP_MIN_AGE_SECONDS} seconds")
            ],
            |row| {
                Ok(Row {
                    id: row.get(0)?,
                    size: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    old: row.get::<_, i64>(2)? != 0,
                    stale: row.get::<_, i64>(3)? != 0,
                })
            },
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })?;

    let kept_referenced = rows
        .iter()
        .filter(|row| referenced.contains(&row.id))
        .count() as i64;
    let mut total: i64 = rows.iter().map(|row| row.size).sum();
    let mut removed = 0i64;
    let mut freed_bytes = 0i64;
    let mut deleted: HashSet<String> = HashSet::new();

    // ① 过了宽限、又没人引用的
    for row in rows.iter() {
        if referenced.contains(&row.id) || !row.old || deleted.contains(&row.id) {
            continue;
        }
        if let Ok(true) = delete(state, &row.id) {
            removed += 1;
            freed_bytes += row.size;
            total -= row.size;
            deleted.insert(row.id.clone());
        }
    }
    // ② 总量超预算：只淘汰没引用、且至少 1 小时前的（防抢刚搜回来的图）
    if total > MAX_CACHE_BYTES {
        let target = MAX_CACHE_BYTES / 10 * 8;
        for row in rows.iter() {
            if total <= target {
                break;
            }
            if referenced.contains(&row.id) || !row.stale || deleted.contains(&row.id) {
                continue;
            }
            if let Ok(true) = delete(state, &row.id) {
                removed += 1;
                freed_bytes += row.size;
                total -= row.size;
                deleted.insert(row.id.clone());
            }
        }
    }

    let orphan_files = sweep_orphan_files(state).unwrap_or(0);
    let stats = cache_stats(state)?;
    Ok(GcReport {
        removed,
        freed_bytes,
        kept_referenced,
        orphan_files,
        remaining: stats.count,
        remaining_bytes: stats.bytes,
    })
}

/// 删掉盘上"库里没有"的附件 / 缩略图；只删至少 1 小时前的，避开正在上传的临时状态。
fn sweep_orphan_files(state: &AppState) -> Result<i64, ApiError> {
    let stored: HashSet<String> = state.with(|conn| {
        let mut stmt = conn.prepare("SELECT stored_name FROM ai_attachments")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<HashSet<_>>>()
    })?;
    let ids: HashSet<String> = state.with(|conn| {
        let mut stmt = conn.prepare("SELECT id FROM ai_attachments")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<HashSet<_>>>()
    })?;

    let mut removed = 0i64;
    let dir = attachment_dir(state);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if stored.contains(&name) || !old_enough(&path, GC_ORPHAN_MIN_AGE_SECONDS) {
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    let thumb_dir = attachment_thumb_dir(state);
    if let Ok(entries) = std::fs::read_dir(&thumb_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let id = name.strip_suffix(".jpg").unwrap_or(&name);
            if ids.contains(id) || !old_enough(&path, GC_ORPHAN_MIN_AGE_SECONDS) {
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

fn old_enough(path: &Path, seconds: i64) -> bool {
    let Ok(modified) = std::fs::metadata(path).and_then(|m| m.modified()) else {
        return false;
    };
    match std::time::SystemTime::now().duration_since(modified) {
        Ok(age) => age.as_secs() >= seconds.max(0) as u64,
        Err(_) => false,
    }
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
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
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
    if !cleaned.len().is_multiple_of(4) {
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
    format!(
        "{}-{}-4{}-a{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

// ---------------------------------------------------------------------------
//  端点
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    // ⚠ 必须写 `axum::routing::get` 的全路径：本模块的仓储函数就叫 `get`
    // （与 Kotlin 同名同义），直接 `use axum::routing::get` 会被同名遮蔽。
    Router::new()
        .route("/api/ai/attachments", axum::routing::post(upload))
        // 缓存统计 / 手动回收（2026-11-04）：必须在 `{id}` 之前/旁边声明，
        // matchit 的静态段优先于参数段，但两条都留着才不会 404。
        .route(
            "/api/ai/attachments/stats",
            axum::routing::get(cache_stats_endpoint),
        )
        .route(
            "/api/ai/attachments/gc",
            axum::routing::post(cache_gc_endpoint),
        )
        .route(
            "/api/ai/attachments/{id}",
            axum::routing::get(get_one).delete(delete_one),
        )
        .route(
            "/api/ai/attachments/{id}/file",
            axum::routing::get(get_file),
        )
        .route(
            "/api/ai/attachments/{id}/thumb",
            axum::routing::get(get_thumb),
        )
        // 附件准入预检（AIH-029）：**纯计算**，不产生任何上游请求
        .route("/api/ai/preflight", axum::routing::post(preflight))
        // 上传这一条要放开 axum 的默认 2MB 请求体上限（默认值会让真实照片传不进来，
        // 见 MAX_BODY_BYTES 的注释）。**只挂在这一条路由上**，别扩到全局。
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
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
    /// 文件头（base64，最多 [`crate::media::strict_intake::MAX_HEAD_BYTES`] 字节）
    pub head_base64: Option<String>,
}

fn fact_of(dto: &AttachmentDto) -> crate::media::strict_intake::AttachmentFact {
    crate::media::strict_intake::AttachmentFact {
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
    let provider = crate::ai::ai::require_provider_row(state, &body.provider_id)?;
    let model = crate::ai::ai::list_models(state, &body.provider_id)?
        .into_iter()
        .find(|m| m.id == body.model_id)
        .ok_or_else(|| {
            ApiError::ai(
                "UNKNOWN_MODEL",
                format!("模型不存在于目录中: {}", body.model_id),
            )
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
        fact: crate::media::strict_intake::AttachmentFact,
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
                fact: crate::media::strict_intake::AttachmentFact {
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
            if bytes.len() > crate::media::strict_intake::MAX_HEAD_BYTES {
                return Err(ApiError::bad_request(format!(
                    "文件头太大（{} 字节 > {}）",
                    bytes.len(),
                    crate::media::strict_intake::MAX_HEAD_BYTES
                )));
            }
        }
        let name = if input.name.trim().is_empty() {
            "附件"
        } else {
            input.name.trim()
        };
        let mime = input
            .mime_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let resolved = crate::media::strict_intake::resolve(
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
        let transports = crate::media::strict_intake::adapter_transports(
            &provider.api,
            item.fact.modality.as_deref().unwrap_or_default(),
        );
        let admission =
            crate::media::strict_intake::evaluate(&model, &item.fact, &transports, index as i64);
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
        entry.insert(
            "blockers".into(),
            Value::Array(blockers.into_iter().map(Value::String).collect()),
        );
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
async fn upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
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
                let tmp = temp_file(&state)
                    .map_err(|e| ApiError::internal(format!("建临时文件失败：{e}")))?;
                match field.bytes().await {
                    Ok(bytes) => {
                        if bytes.len() as u64 > MAX_UPLOAD_BYTES {
                            let _ = std::fs::remove_file(&tmp);
                            failed.push(FailedInfo {
                                file_name: name.clone(),
                                reason: format!(
                                    "文件 {name} 超过单附件上限（{} MB）",
                                    MAX_UPLOAD_BYTES / 1024 / 1024
                                ),
                            });
                            continue;
                        }
                        if let Err(e) = std::fs::write(&tmp, &bytes) {
                            let _ = std::fs::remove_file(&tmp);
                            failed.push(FailedInfo {
                                file_name: name,
                                reason: format!("写临时文件失败：{e}"),
                            });
                            continue;
                        }
                        incoming.push((name, tmp, declared));
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&tmp);
                        failed.push(FailedInfo {
                            file_name: name,
                            reason: format!("读取上传内容失败：{e}"),
                        });
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                for (_, tmp, _) in incoming.iter() {
                    let _ = std::fs::remove_file(tmp);
                }
                // 请求体撞上上限（`DefaultBodyLimit`，见 MAX_BODY_BYTES）时 axum 给的是
                // 一句 `Error parsing multipart/form-data request` —— 谁也看不出是"传太大了"。
                // 这里换成能照着做的人话（单文件上限仍然由上面逐条判定）。
                if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    return Err(ApiError::ai(
                        "UNSUPPORTED_CONTENT",
                        format!(
                            "这次上传的总内容超过 {} MB 上限（单张最大 {} MB）—— 少选几个文件再试",
                            MAX_BODY_BYTES / 1024 / 1024,
                            MAX_UPLOAD_BYTES / 1024 / 1024
                        ),
                    ));
                }
                return Err(ApiError::ai(
                    "CONFIG_ERROR",
                    format!("附件上传解析失败：{e}"),
                ));
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

/// `GET /api/ai/attachments/stats`：缓存有多少、按来源分。
async fn cache_stats_endpoint(State(state): State<AppState>) -> Result<Json<CacheStats>, ApiError> {
    Ok(Json(cache_stats(&state)?))
}

/// `POST /api/ai/attachments/gc`：手动清一次没被引用的旧附件。
async fn cache_gc_endpoint(State(state): State<AppState>) -> Result<Json<GcReport>, ApiError> {
    Ok(Json(collect_garbage(&state)?))
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
    crate::media::media::serve_path(
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
    let _ = get(&state, &id)?
        .ok_or_else(|| ApiError::ai("CONFIG_ERROR", format!("附件不存在: {id}")))?;
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
    let mut response = crate::media::media::serve_path(
        &path,
        None,
        &content_type,
        &format!("thumb-{id}"),
        &HeaderMap::new(),
    )
    .await?;
    response
        .headers_mut()
        .insert("x-thumbnail", axum::http::HeaderValue::from_static("1"));
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
mod tests;
