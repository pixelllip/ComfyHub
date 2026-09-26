//! 产物端点（P4 第 3 步）：列表 / 详情 / **原件文件流**（Range + ETag）。
//!
//! # 为什么"文件流"是这一批的重点
//!
//! Ktor 那边靠 `respondFile` 拿 PartialContent 支持（视频拖动进度条要用 Range）。
//! Rust 侧不复用现成静态文件服务（`tower-http` 的 `ServeFile` 不支持 ETag，
//! 而我们的 ETag 是**现成的**：`media_assets.sha256` 本来就是内容哈希，
//! 内容寻址 + `immutable` 正是总纲 §5.3 要的那套缓存语义）。
//! 所以这里自己写：`If-None-Match` 命中 → **304**（省一整条视频的流量）；
//! `Range: bytes=a-b` → **206 + Content-Range**；否则 200 全量。
//!
//! # 目录穿越
//!
//! `stored_name` 来自库，但仍然**必须**过 [`resolve_media`]：它把路径 normalize 之后
//! 要求它还在 `media/` 下面（与 Kotlin `Storage.resolveMedia` 同一条防线）——
//! 库被人手改一下就能读任意文件，这种事不该靠"库是可信的"来兜。
//!
//! # 还没搬的
//!
//! `upload` / `patch` / `delete` / `thumb`（要图像解码器）/ `poster`（要 ffmpeg 抽帧）
//! 一律 **501**，不拿空数组假装成功。

use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use std::path::{Path as FsPath, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::models::{media_from_row, MediaDto, Paged};
use crate::routes::{ApiError, AppState};

const FROM: &str = "FROM media_assets m LEFT JOIN prompts p ON p.id = m.prompt_id";

/// 产物的列清单（与 Kotlin `MediaRepo.COLS` 逐列一致，别加别减）。
const COLS: &str = "m.id, m.prompt_id, m.kind, m.title, m.original_name, m.stored_name, m.mime_type,
     m.size_bytes, m.width, m.height, m.duration_ms, m.sha256, m.source, m.favorite, m.notes,
     m.nsfw, m.created_at, m.updated_at, (m.workflow_json IS NOT NULL) AS has_workflow,
     p.title AS prompt_title, p.positive_prompt AS prompt_positive,
     p.negative_prompt AS prompt_negative, p.checkpoint, p.seed";

#[derive(Debug, Deserialize, Default)]
pub struct MediaQuery {
    q: Option<String>,
    tags: Option<String>,
    #[serde(rename = "tagMode")]
    tag_mode: Option<String>,
    kind: Option<String>,
    #[serde(rename = "promptId")]
    prompt_id: Option<i64>,
    favorite: Option<bool>,
    untagged: Option<bool>,
    sort: Option<String>,
    page: Option<i64>,
    size: Option<i64>,
}

/// 列表 / 搜索（`GET /api/media`）。
pub fn search(state: &AppState, q: &MediaQuery) -> Result<Paged<MediaDto>, ApiError> {
    let page = q.page.unwrap_or(1).max(1);
    let size = q.size.unwrap_or(24).clamp(1, 200);
    let offset = (page - 1) * size;
    let tags = parse_tag_param(q.tags.as_deref());
    let tag_mode = q.tag_mode.clone().unwrap_or_else(|| "any".to_string());

    state.with(|conn| {
        let mut where_sql = String::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let mut clauses: Vec<String> = Vec::new();

        if let Some(keyword) = q.q.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
            // 按空白拆词，每个词都要命中（AND）——与 Kotlin 一致（中文场景用 LIKE，不用 FTS）
            for term in keyword.split_whitespace().take(8) {
                clauses.push(
                    "(m.title LIKE ? OR m.original_name LIKE ? OR m.notes LIKE ?
                      OR p.title LIKE ? OR p.positive_prompt LIKE ? OR p.negative_prompt LIKE ?)"
                        .to_string(),
                );
                let like = format!("%{term}%");
                for _ in 0..6 {
                    args.push(Box::new(like.clone()));
                }
            }
        }

        if !tags.is_empty() {
            let marks = vec!["?"; tags.len()].join(",");
            if tag_mode == "all" {
                // 产物自己的标签 ∪ 它关联提示词的标签，必须**全部**命中
                clauses.push(format!(
                    "m.id IN (
                       SELECT x.media_id FROM (
                         SELECT mt.media_id AS media_id, t.normalized AS n
                           FROM media_tags mt JOIN tags t ON t.id = mt.tag_id
                          WHERE t.normalized IN ({marks})
                         UNION
                         SELECT m2.id AS media_id, t2.normalized AS n
                           FROM media_assets m2
                           JOIN prompt_tags pt ON pt.prompt_id = m2.prompt_id
                           JOIN tags t2 ON t2.id = pt.tag_id
                          WHERE t2.normalized IN ({marks})
                       ) x GROUP BY x.media_id HAVING COUNT(DISTINCT x.n) = {}
                     )",
                    tags.len()
                ));
                for _ in 0..2 {
                    for tag in &tags {
                        args.push(Box::new(tag.clone()));
                    }
                }
            } else {
                clauses.push(format!(
                    "(EXISTS (SELECT 1 FROM media_tags mt JOIN tags t ON t.id = mt.tag_id
                               WHERE mt.media_id = m.id AND t.normalized IN ({marks}))
                      OR EXISTS (SELECT 1 FROM prompt_tags pt JOIN tags t ON t.id = pt.tag_id
                               WHERE pt.prompt_id = m.prompt_id AND t.normalized IN ({marks})))"
                ));
                for _ in 0..2 {
                    for tag in &tags {
                        args.push(Box::new(tag.clone()));
                    }
                }
            }
        }

        if let Some(kind) = q.kind.as_ref().map(|k| k.trim().to_uppercase()).filter(|k| !k.is_empty() && k != "ALL")
        {
            clauses.push("m.kind = ?".to_string());
            args.push(Box::new(kind));
        }
        if let Some(prompt_id) = q.prompt_id {
            clauses.push("m.prompt_id = ?".to_string());
            args.push(Box::new(prompt_id));
        }
        if q.favorite == Some(true) {
            clauses.push("m.favorite = 1".to_string());
        }
        if q.untagged == Some(true) {
            clauses.push("m.prompt_id IS NULL".to_string());
        }
        if !clauses.is_empty() {
            where_sql = format!("WHERE {}", clauses.join(" AND "));
        }

        let total: i64 = {
            let sql = format!("SELECT COUNT(*) {FROM} {where_sql}");
            let mut stmt = conn.prepare(&sql)?;
            let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
            stmt.query_row(refs.as_slice(), |row| row.get(0))?
        };

        let order = match q.sort.as_deref() {
            Some("oldest") => "m.created_at ASC",
            Some("updated") => "m.updated_at DESC",
            Some("name") => "m.original_name ASC",
            Some("largest") => "m.size_bytes DESC",
            Some("favorite") => "m.favorite DESC, m.created_at DESC",
            _ => "m.created_at DESC",
        };
        let sql = format!("SELECT {COLS} {FROM} {where_sql} ORDER BY {order} LIMIT ? OFFSET ?");
        let mut stmt = conn.prepare(&sql)?;
        {
            let mut all: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
            let size_box = size as i64;
            let offset_box = offset;
            all.push(&size_box);
            all.push(&offset_box);
            let rows = stmt.query_map(all.as_slice(), |row| media_from_row(row))?;
            let mut items = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            for item in items.iter_mut() {
                item.prompt_tags = merged_tags(conn, item)?;
            }
            Ok(Paged::new(items, total, page, size))
        }
    })
}

pub fn get(state: &AppState, id: i64) -> Result<MediaDto, ApiError> {
    state
        .with(|conn| {
            let media = conn
                .query_row(&format!("SELECT {COLS} {FROM} WHERE m.id = ?1"), params![id], |row| {
                    media_from_row(row)
                })
                .optional()?;
            match media {
                Some(mut m) => {
                    m.prompt_tags = merged_tags(conn, &m)?;
                    Ok(Some(m))
                }
                None => Ok(None),
            }
        })?
        .ok_or_else(|| ApiError::not_found(format!("产物 {id} 不存在")))
}

/// 产物自身的标签 + 它关联提示词的标签（去重合并、按名字排序）——与 Kotlin
/// `MediaRepo.attachOwnTags` 同义（画廊里点开/筛选时看到的标签集合要一致）。
fn merged_tags(conn: &rusqlite::Connection, media: &MediaDto) -> rusqlite::Result<Vec<String>> {
    let mut own: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT t.name FROM media_tags mt JOIN tags t ON t.id = mt.tag_id
              WHERE mt.media_id = ?1 ORDER BY t.name",
        )?;
        let rows = stmt.query_map(params![media.id], |row| row.get::<_, String>(0))?;
        let collected = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        collected
    };
    if let Some(prompt_id) = media.prompt_id {
        let mut stmt = conn.prepare(
            "SELECT t.name FROM prompt_tags pt JOIN tags t ON t.id = pt.tag_id
              WHERE pt.prompt_id = ?1 ORDER BY t.name",
        )?;
        let from_prompt = stmt
            .query_map(params![prompt_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        own.extend(from_prompt);
    }
    own.sort();
    own.dedup();
    Ok(own)
}

/// 标签过滤参数：逗号分隔、统一小写去空格（与 Kotlin `TagRepo.parseTagParam` 同义）。
fn parse_tag_param(raw: Option<&str>) -> Vec<String> {
    // ⚠ 不能用 `Vec::dedup()`：它只去**相邻**重复，而 `赛博,国风,赛博` 这种中间隔了一个的
    // 会漏掉（Kotlin 那边是 `distinct()`，语义是全局去重、保序）。单测盯着这条。
    let mut out: Vec<String> = Vec::new();
    for part in raw.unwrap_or("").split(',') {
        let tag = part.trim().to_lowercase();
        if !tag.is_empty() && !out.contains(&tag) {
            out.push(tag);
        }
    }
    out
}

// ---------------------------------------------------------------------------
//  原件文件流
// ---------------------------------------------------------------------------

/// 把 `stored_name` 解析成磁盘路径，**顺带挡住目录穿越**（与 Kotlin `Storage.resolveMedia` 同义）。
pub fn resolve_media(media_dir: &FsPath, stored_name: &str) -> Option<PathBuf> {
    if stored_name.trim().is_empty() {
        return None;
    }
    // 名字里带分隔符的一律拒绝（normalize 之后还要再比一次，双保险）
    let candidate = media_dir.join(stored_name);
    let normalized = normalize_path(&candidate)?;
    if !normalized.starts_with(media_dir) {
        return None;
    }
    if !normalized.is_file() {
        return None;
    }
    Some(normalized)
}

/// 纯词法 normalize（不碰文件系统：文件不存在时也要能判"越界了"）。
fn normalize_path(path: &FsPath) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

/// `GET /api/media/{id}/file` —— 原件，支持 Range 与 ETag。
pub async fn media_file(state: AppState, id: i64, headers: HeaderMap) -> Result<Response, ApiError> {
    let media = get(&state, id)?;
    let path = resolve_media(state.media_dir(), &media.stored_name)
        .ok_or_else(|| ApiError::not_found(format!("文件已丢失: {}", media.stored_name)))?;
    let mime = media
        .mime_type
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    serve_path(&path, media.sha256.as_deref(), &mime, &media.original_name, &headers).await
}

/// 把磁盘上的一个文件流出去：**ETag（内容寻址）+ `If-None-Match` 304 + 单段 Range 206**。
///
/// 抽成公共函数是为了让**附件原件**（`GET /api/ai/attachments/{id}/file`）与画廊产物**共用同一份实现**：
/// 视频拖动进度条靠的就是 Range，两处各写一份必然漂移（一处支持 Range、另一处不支持，
/// 表现是"画廊里的视频能拖、聊天里的拖不动"）。
///
/// `etag` 给内容 sha256；没有就退回 `len-mtime`（与画廊那条路同一个口径）。
pub async fn serve_path(
    path: &std::path::Path,
    etag: Option<&str>,
    mime: &str,
    name: &str,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| ApiError::internal(format!("读取文件失败：{e}")))?;
    let len = meta.len();
    let etag = match etag.filter(|s| !s.is_empty()) {
        Some(sha) => format!("\"{sha}\""),
        None => format!(
            "\"len{}-{:x}\"",
            len,
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ),
    };

    // 命中 ETag → 304（内容寻址 + immutable：这一条能省掉整段视频的流量）
    if let Some(inm) = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) {
        if inm.split(',').any(|t| t.trim() == etag || t.trim() == "*") {
            return Ok(Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .header(header::ETAG, etag)
                .body(Body::empty())
                .unwrap());
        }
    }

    let mime = if mime.trim().is_empty() { "application/octet-stream" } else { mime };
    let disposition = inline_disposition(name);

    // Range：视频拖动进度条就靠它
    if let Some(raw) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        if let Some((start, end)) = parse_range(raw, len) {
            let mut file = tokio::fs::File::open(path)
                .await
                .map_err(|e| ApiError::internal(format!("打开文件失败：{e}")))?;
            file.seek(std::io::SeekFrom::Start(start))
                .await
                .map_err(|e| ApiError::internal(format!("定位失败：{e}")))?;
            let take = end - start + 1;
            let stream = ReaderStream::new(file.take(take));
            return Ok(Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, mime)
                .header(header::CONTENT_LENGTH, take.to_string())
                .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::ETAG, etag)
                .header(header::CONTENT_DISPOSITION, disposition)
                .body(Body::from_stream(stream))
                .unwrap());
        }
    }

    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| ApiError::internal(format!("打开文件失败：{e}")))?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, len.to_string())
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ETAG, etag)
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(Body::from_stream(ReaderStream::new(file)))
        .unwrap())
}

/// 解析 `Range: bytes=a-b`（**只支持单段**：多段 Range 的响应体是 multipart，
/// 播放器其实不用它；宁可不支持，也不要返回一个格式不对的 206）。
fn parse_range(raw: &str, len: u64) -> Option<(u64, u64)> {
    let spec = raw.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start_raw, end_raw) = spec.split_once('-')?;
    if len == 0 {
        return None;
    }
    let (start, end) = if start_raw.trim().is_empty() {
        // `bytes=-500`：最后 500 字节
        let suffix: u64 = end_raw.trim().parse().ok()?;
        let take = suffix.min(len);
        (len - take, len - 1)
    } else {
        let start: u64 = start_raw.trim().parse().ok()?;
        let end = if end_raw.trim().is_empty() {
            len - 1
        } else {
            end_raw.trim().parse::<u64>().ok()?.min(len - 1)
        };
        (start, end)
    };
    if start > end || start >= len {
        return None;
    }
    Some((start, end))
}

/// `Content-Disposition: inline; filename="…"` —— 非 ASCII 名字额外给 `filename*`。
fn inline_disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| if c.is_ascii() && c != '"' && c != '\\' { c } else { '_' })
        .collect();
    let encoded: String = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("inline; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

/// 给"还没搬"的媒体端点用的统一 501。
pub fn not_migrated(what: &str) -> ApiError {
    ApiError::not_implemented(format!("{what} 还没搬到 Rust（灰度中，先走 Kotlin 的 8080）"))
}

/// 小工具：`HeaderValue` 的构造失败不该 panic（那是 500 的另一种写法）。
#[allow(dead_code)]
pub fn header_value(raw: &str) -> Option<HeaderValue> {
    HeaderValue::from_str(raw).ok()
}

/// 便于路由层直接返回 JSON 的错误（保持与读端点一致的形状）。
pub fn json_error(error: ApiError) -> Response {
    error.into_response()
}

/// 详情端点的返回类型别名（省得路由层写一长串）。
pub type MediaJson = Json<MediaDto>;

/// 列表端点的返回类型别名。
pub type MediaPageJson = Json<Paged<MediaDto>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parsing_covers_the_three_shapes() {
        assert_eq!(parse_range("bytes=0-499", 1000), Some((0, 499)));
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_range("bytes=-200", 1000), Some((800, 999)));
        // 越界 / 多段 / 畸形一律不当 Range（回 200 全量，而不是错格式的 206）
        assert_eq!(parse_range("bytes=1000-2000", 1000), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
        assert_eq!(parse_range("bytes=-0", 0), None);
    }

    #[test]
    fn path_traversal_is_rejected() {
        let dir = std::path::Path::new("D:/x/storage/media");
        assert!(normalize_path(&dir.join("../secret.txt")).is_some());
        let resolved = normalize_path(&dir.join("../secret.txt")).unwrap();
        assert!(!resolved.starts_with(dir), "越界必须被 starts_with 拦住");
        assert!(resolve_media(dir, "../secret.txt").is_none());
        assert!(resolve_media(dir, "").is_none());
    }

    #[test]
    fn inline_disposition_carries_a_utf8_fallback() {
        let value = inline_disposition("赛博朋克.png");
        assert!(value.contains("filename=\"______.png\"") || value.contains("filename=\""));
        assert!(value.contains("filename*=UTF-8''"));
    }

    #[test]
    fn tag_param_is_normalized_and_deduped() {
        assert_eq!(parse_tag_param(Some(" 赛博 , 国风,赛博")), vec!["赛博", "国风"]);
        assert!(parse_tag_param(None).is_empty());
    }
}
// ---------------------------------------------------------------------------
//  缩略图（P4 第 5 步）
// ---------------------------------------------------------------------------

/// `GET /api/media/{id}/thumb` —— 图片缩略图（生成后缓存；**失败回退原图**）。
///
/// 与 Kotlin `MediaRoutes` 的 `/thumb` 分支逐条对齐：
///  1. **非图片 → 204**（前端据此不放缩略图，而不是显示破图）；
///  2. 缩略图不存在就现生成（`<storage>/thumbs/<id>.jpg`）；生成器**不放大**、铺深灰底；
///  3. 生成失败（没有解码器 / 文件坏了）→ **回退原图**（宁可给大图，也不要破图或 500）；
///  4. 命中缓存时带 `X-Thumbnail: 1`（与 Kotlin 同一个头，前端/e2e 都认它）。
pub async fn media_thumb(state: AppState, id: i64) -> Result<Response, ApiError> {
    let media = get(&state, id)?;
    if media.kind != "IMAGE" {
        return Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .unwrap());
    }
    let source = resolve_media(state.media_dir(), &media.stored_name)
        .ok_or_else(|| ApiError::not_found("文件已丢失"))?;

    let thumb = crate::thumbs::thumb_path(state.media_dir(), id);
    let mut from_cache = true;
    if !thumb.is_file() {
        from_cache = false;
        // 生成失败不算错：下面回退原图（与 Kotlin 的 `if (!writeThumbnail(...))` 分支一致）
        let _ = crate::thumbs::write_thumbnail(&source, &thumb);
    }

    let hit_thumb = thumb.is_file();
    let (path, mime, name) = if hit_thumb {
        (thumb, "image/jpeg".to_string(), format!("thumb-{id}.jpg"))
    } else {
        let mime = media
            .mime_type
            .clone()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        (source, mime, media.original_name.clone())
    };

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| ApiError::internal(format!("读取缩略图失败：{e}")))?;
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .header(header::CONTENT_DISPOSITION, inline_disposition(&name));
    if hit_thumb {
        // 命中缓存 = 1（这是"这次没现生成"的意思，与 Kotlin 的 `X-Thumbnail: 1` 同义）
        builder = builder.header("X-Thumbnail", if from_cache { "1" } else { "0" });
    }
    Ok(builder.body(Body::from(bytes)).unwrap())
}
/// `GET /api/prompts/{id}/media` —— 某个提示词下的全部产物（按创建时间倒序）。
///
/// ⚠ 与 Kotlin 一致：**提示词不存在也回 200 + `[]`**（不做存在性检查）。
/// 这不是"忘了"，而是那条路由本来就没查 —— 详情页拿空列表渲染，比弹一个 404 更合适。
pub fn list_by_prompt(state: &AppState, prompt_id: i64) -> Result<Vec<MediaDto>, ApiError> {
    state.with(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} {FROM} WHERE m.prompt_id = ?1 ORDER BY m.created_at DESC"
        ))?;
        let rows = stmt.query_map(params![prompt_id], |row| media_from_row(row))?;
        let mut items = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        for item in items.iter_mut() {
            item.prompt_tags = merged_tags(conn, item)?;
        }
        Ok(items)
    })
}

/// `GET /api/media/{id}/prompt` —— 这条产物关联的**完整提示词**。
///
/// 三种结果：
///  - 产物不存在 → 404；
///  - 产物没关联提示词（`prompt_id IS NULL`）→ **204**（Dart 那边 `mediaPrompt` 就是把 204 当 null）；
///  - 有关联但那条提示词**已经不在了**（悬挂引用）→ **404**，说明是"关联的提示词已被删除"。
///
/// ⚠ 这里与 Kotlin 有一处**有意的行为差异**：Kotlin 那条路由用的是
/// `PromptRepo.get(pid) ?: ApiError("关联的提示词已被删除")` —— 也就是**200 + `{"error":…}`**。
/// 前端 `Prompt.fromJson` 会把这个 body 解析成一条空提示词（弹窗里显示空字段），
/// 而不是报错。我们回 404：Dart 的 `mediaPrompt` 对非 2xx 是抛异常（界面显示失败），
/// 至少不会假装"拿到了一条提示词"。**切流时这条要在验收清单上过一遍。**
pub fn prompt_of_media(state: &AppState, media_id: i64) -> Result<Option<crate::models::PromptDto>, ApiError> {
    let media = get(state, media_id)?;
    let Some(prompt_id) = media.prompt_id else {
        return Ok(None);
    };
    state
        .with(|conn| {
            let prompt = conn
                .query_row(
                    "SELECT * FROM prompts WHERE id = ?1",
                    params![prompt_id],
                    |row| crate::models::prompt_from_row(row),
                )
                .optional()?;
            match prompt {
                Some(mut p) => {
                    p.tags = crate::routes::tags_of(conn, p.id)?;
                    p.media_count = crate::routes::media_count_of(conn, p.id)?;
                    Ok(Some(Some(p)))
                }
                None => Ok(Some(None)),
            }
        })?
        .flatten()
        .map_or_else(
            || Err(ApiError::not_found("关联的提示词已被删除")),
            |p| Ok(Some(p)),
        )
}