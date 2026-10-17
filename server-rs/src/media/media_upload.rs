//! 上传（P4 第 6 步）—— multipart 多文件入库。
//!
//! # 与 Kotlin `MediaRoutes` 的 `POST /api/media/upload` 逐条对齐
//!
//! | 行为 | 细节 |
//! | --- | --- |
//! | 字段名 | 文件用 **`files`**；其它字段 `promptId` / `title` / `kind` / `source` / `notes` / `tags`（逗号分隔） |
//! | 一个文件都没收到 | **400**「没有收到文件（字段名请用 files）」——不静默成功 |
//! | `promptId` 不存在 | **400**（且把已经落到临时目录的文件清掉） |
//! | 落盘名 | `<32 位 hex>.<ext>`（`newStoredName`，与 Kotlin 同一套命名） |
//! | 判重 | 按 **文件内容的 sha256**：命中已有产物 → 删掉刚落的那份、记进 `duplicates`（**不重复入库**） |
//! | 入库 | 一个文件一个事务：插行 + 标签 + `refresh_use_counts`；图片再补 `width/height` 与缩略图 |
//! | 单个文件失败 | **不整批失败**：记进 `failed`（`fileName` + 原因），其余继续 |
//! | 响应 | **201** + `{items, duplicates, failed, promptId}` |
//!
//! # 为什么先全部收进临时目录
//!
//! multipart 的字段与文件**顺序不保证**（文件名、`promptId` 哪个先来都可能）。
//! Kotlin 那边也是先收齐再处理 —— 这样"用 `promptId` 校验存在性"才能在**动库之前**做完，
//! 而不是写了一半个文件才发现 `promptId` 是错的。

use axum::extract::Multipart;
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::media::media;
use crate::web::models::MediaDto;
use crate::library::repo::{self, WriteError};
use crate::web::routes::{ApiError, AppState};

pub const KINDS: [&str; 3] = ["IMAGE", "VIDEO", "AUDIO"];

/// 扩展名 → MIME（与 Kotlin `MediaFiles.MIME` 同一张表）。
fn mime_table() -> &'static [(&'static str, &'static str)] {
    &[
        ("png", "image/png"), ("jpg", "image/jpeg"), ("jpeg", "image/jpeg"),
        ("webp", "image/webp"), ("gif", "image/gif"), ("bmp", "image/bmp"),
        ("avif", "image/avif"), ("tif", "image/tiff"), ("tiff", "image/tiff"),
        ("mp4", "video/mp4"), ("webm", "video/webm"), ("mov", "video/quicktime"),
        ("mkv", "video/x-matroska"), ("avi", "video/x-msvideo"), ("m4v", "video/x-m4v"),
        ("mp3", "audio/mpeg"), ("wav", "audio/wav"), ("flac", "audio/flac"),
        ("ogg", "audio/ogg"), ("m4a", "audio/mp4"), ("aac", "audio/aac"),
        ("opus", "audio/opus"), ("wma", "audio/x-ms-wma"),
    ]
}

pub fn extension_of(filename: &str) -> String {
    filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_lowercase())
        .unwrap_or_default()
        .chars()
        .take(12)
        .collect()
}

/// 客户端给的 MIME 优先（不等于 `application/octet-stream` 时），否则按扩展名查表。
pub fn detect_mime(filename: &str, provided: Option<&str>) -> String {
    if let Some(m) = provided.map(|m| m.trim()).filter(|m| !m.is_empty() && *m != "application/octet-stream") {
        return m.to_string();
    }
    let ext = extension_of(filename);
    mime_table()
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, m)| m.to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

pub fn kind_of(filename: &str, provided_mime: Option<&str>) -> String {
    let mime = detect_mime(filename, provided_mime);
    if mime.starts_with("image/") {
        "IMAGE".into()
    } else if mime.starts_with("video/") {
        "VIDEO".into()
    } else if mime.starts_with("audio/") {
        "AUDIO".into()
    } else {
        // 认不出来就当图片（与 Kotlin 的 `else -> "IMAGE"` 同义：宁可给它生成缩略图）
        "IMAGE".into()
    }
}

/// 落盘名：32 位 hex + 扩展名（与 Kotlin `Storage.newStoredName` 同一套）。
pub fn new_stored_name(original_name: &str) -> String {
    let ext = extension_of(original_name);
    let id = uuid_hex();
    if ext.is_empty() {
        id
    } else {
        format!("{id}.{ext}")
    }
}

/// 不引 `uuid` crate：这里只要"足够不会撞"的随机十六进制（32 位）。
fn uuid_hex() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(format!("{:?}", std::thread::current().id()).as_bytes());
    let digest = hasher.finalize();
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// 文件内容的 sha256（去重键 —— 与 Kotlin 的 SHA-256 判重同一个口径）。
pub fn sha256_of(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// 上传结果（与 Kotlin `UploadResultDto` 同形）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadResult {
    pub items: Vec<MediaDto>,
    pub duplicates: Vec<DuplicateInfo>,
    pub failed: Vec<FailedInfo>,
    /// Kotlin `UploadResultDto.promptId: Long? = null` → 请求里没带 promptId 时**键不出现**
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateInfo {
    pub file_name: String,
    pub existing_media_id: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedInfo {
    pub file_name: String,
    pub reason: String,
}

struct Incoming {
    original_name: String,
    tmp: PathBuf,
    mime: Option<String>,
    size: i64,
}

/// `POST /api/media/upload`。
pub async fn upload(state: AppState, mut multipart: Multipart) -> Result<(axum::http::StatusCode, axum::Json<UploadResult>), ApiError> {
    let mut prompt_id: Option<i64> = None;
    let mut title: Option<String> = None;
    let mut kind_override: Option<String> = None;
    let mut source: Option<String> = None;
    let mut notes: Option<String> = None;
    let mut tags: Vec<String> = Vec::new();
    let mut incoming: Vec<Incoming> = Vec::new();
    let mut failed: Vec<FailedInfo> = Vec::new();

    let tmp_dir = state.media_dir().parent().unwrap_or(Path::new(".")).join("tmp");
    std::fs::create_dir_all(&tmp_dir)
        .map_err(|e| ApiError::internal(format!("临时目录不可用：{e}")))?;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("上传解析失败：{e}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        let file_name = field.file_name().map(|f| f.to_string());
        match file_name {
            None => {
                // 普通表单字段
                let value = field.text().await.unwrap_or_default();
                match name.as_str() {
                    "promptId" => prompt_id = value.trim().parse::<i64>().ok(),
                    "title" => title = Some(value.trim().to_string()).filter(|t| !t.is_empty()),
                    "kind" => {
                        let upper = value.trim().to_uppercase();
                        kind_override = KINDS.contains(&upper.as_str()).then_some(upper);
                    }
                    "source" => source = Some(value.trim().to_string()).filter(|t| !t.is_empty()),
                    "notes" => notes = Some(value.trim().to_string()).filter(|t| !t.is_empty()),
                    "tags" => {
                        tags = value
                            .split(',')
                            .map(|t| t.trim().to_string())
                            .filter(|t| !t.is_empty())
                            .collect()
                    }
                    _ => {}
                }
            }
            Some(original) => {
                let original_name = if original.trim().is_empty() {
                    "upload.bin".to_string()
                } else {
                    original
                };
                let tmp = tmp_dir.join(format!("upload-{}.part", uuid_hex()));
                let mime = field.content_type().map(|m| m.to_string());
                let bytes = match field.bytes().await {
                    Ok(b) => b,
                    Err(e) => {
                        failed.push(FailedInfo {
                            file_name: original_name,
                            reason: format!("读取上传内容失败：{e}"),
                        });
                        continue;
                    }
                };
                let size = bytes.len() as i64;
                if let Err(e) = std::fs::write(&tmp, &bytes) {
                    failed.push(FailedInfo { file_name: original_name, reason: format!("写入临时文件失败：{e}") });
                    continue;
                }
                incoming.push(Incoming { original_name, tmp, mime, size });
            }
        }
    }

    if incoming.is_empty() {
        return Err(ApiError::bad_request("没有收到文件（字段名请用 files）"));
    }

    // promptId 校验放在**动库之前**；不通过就把临时文件清掉（别留一地垃圾）
    if let Some(pid) = prompt_id {
        let exists = state.with(|conn| {
            Ok(conn
                .query_row("SELECT id FROM prompts WHERE id = ?1", params![pid], |row| row.get::<_, i64>(0))
                .optional_present()?)
        })?;
        if exists.is_none() {
            incoming.iter().for_each(|i| {
                let _ = std::fs::remove_file(&i.tmp);
            });
            return Err(ApiError::bad_request(format!("promptId={pid} 对应的提示词不存在")));
        }
    }

    let mut created: Vec<MediaDto> = Vec::new();
    let mut duplicates: Vec<DuplicateInfo> = Vec::new();
    let media_dir = state.media_dir().to_path_buf();

    for item in incoming {
        let stored_name = new_stored_name(&item.original_name);
        let moved = media_dir.join(&stored_name);
        if let Err(e) = std::fs::rename(&item.tmp, &moved) {
            failed.push(FailedInfo { file_name: item.original_name, reason: format!("移动文件失败：{e}") });
            continue;
        }

        let result = (|| -> Result<Option<i64>, String> {
            let kind = kind_override.clone().unwrap_or_else(|| kind_of(&item.original_name, item.mime.as_deref()));
            let mime = detect_mime(&item.original_name, item.mime.as_deref());
            let sha = sha256_of(&moved).map_err(|e| format!("算 sha256 失败：{e}"))?;

            // 判重：内容相同就**不入库**（与 Kotlin 一致）
            let existing: Option<i64> = state
                .with(|conn| {
                    Ok(conn
                        .query_row("SELECT id FROM media_assets WHERE sha256 = ?1 LIMIT 1", params![sha], |row| {
                            row.get::<_, i64>(0)
                        })
                        .optional_present()?)
                })
                .map_err(|e| e.human().to_string())?;
            if let Some(existing_id) = existing {
                let _ = std::fs::remove_file(&moved);
                duplicates.push(DuplicateInfo { file_name: item.original_name.clone(), existing_media_id: existing_id });
                return Ok(None);
            }

            let dims = if kind == "IMAGE" { crate::media::thumbs::probe_image_size(&moved) } else { None };
            let new_title = title
                .clone()
                .unwrap_or_else(|| item.original_name.clone());

            let new_id = state
                .write(|tx| -> Result<i64, WriteError> {
                    tx.execute(
                        "INSERT INTO media_assets
                           (prompt_id, kind, title, original_name, stored_name, mime_type, size_bytes,
                            width, height, duration_ms, sha256, source, notes, favorite)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,NULL,?10,?11,?12,0)",
                        params![
                            prompt_id,
                            kind,
                            new_title,
                            item.original_name,
                            stored_name,
                            mime,
                            item.size,
                            dims.map(|d| d.0),
                            dims.map(|d| d.1),
                            sha,
                            source.clone().unwrap_or_else(|| "ComfyUI".to_string()),
                            notes.clone(),
                        ],
                    )?;
                    let id = tx.last_insert_rowid();
                    if !tags.is_empty() {
                        repo::add_media_tags(tx, id, &tags)?;
                    }
                    // 产物**集合**：手动上传的产物一般没有提示词，会按 kind 归到"孤儿那一组"
                    // （详情页也要能在同批之间左右切换）。判定见 `media/collection.rs`。
                    crate::media::collection::assign(tx, id)?;
                    repo::refresh_use_counts(tx)?;
                    Ok(id)
                })
                .map_err(|e| e.human().to_string())?;

            if kind == "IMAGE" {
                let thumb = crate::media::thumbs::thumb_path(&media_dir, new_id);
                let _ = crate::media::thumbs::write_thumbnail(&moved, &thumb);
            }
            Ok(Some(new_id))
        })();

        match result {
            Ok(Some(new_id)) => match media::get(&state, new_id) {
                Ok(dto) => created.push(dto),
                Err(e) => failed.push(FailedInfo { file_name: item.original_name, reason: e.human().to_string() }),
            },
            Ok(None) => {}
            Err(reason) => {
                // 单个文件失败不整批失败：清掉刚落的那份，记进 failed
                let _ = std::fs::remove_file(&moved);
                failed.push(FailedInfo { file_name: item.original_name, reason });
            }
        }
    }

    Ok((
        axum::http::StatusCode::CREATED,
        axum::Json(UploadResult { items: created, duplicates, failed, prompt_id }),
    ))
}

/// `rusqlite::OptionalExtension` 的小包装（省得每个文件都 import 一次）。
trait OptionalPresent<T> {
    fn optional_present(self) -> rusqlite::Result<Option<T>>;
}

impl<T> OptionalPresent<T> for rusqlite::Result<T> {
    fn optional_present(self) -> rusqlite::Result<Option<T>> {
        use rusqlite::OptionalExtension;
        self.optional()
    }
}

#[cfg(test)]
mod tests;
