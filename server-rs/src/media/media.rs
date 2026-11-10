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

use crate::web::models::{media_from_row, MediaDto, Paged};
use crate::web::routes::{ApiError, AppState};

const FROM: &str = "FROM media_assets m LEFT JOIN prompts p ON p.id = m.prompt_id";

/// 产物的列清单（与 Kotlin `MediaRepo.COLS` 逐列一致，别加别减）。
const COLS: &str =
    "m.id, m.prompt_id, m.kind, m.title, m.original_name, m.stored_name, m.mime_type,
     m.size_bytes, m.width, m.height, m.duration_ms, m.sha256, m.source, m.favorite, m.notes,
     m.nsfw, m.created_at, m.updated_at, (m.workflow_json IS NOT NULL) AS has_workflow,
     p.title AS prompt_title, p.positive_prompt AS prompt_positive,
     p.negative_prompt AS prompt_negative, p.checkpoint, p.seed,
     m.collection_id AS collection_id,
     -- `MAX(1, …)`：孤儿产物（collection_id IS NULL）的 COUNT 会是 0，
     -- 但语义上它是「只有自己一个」→ 1。界面据此决定要不要叠放 / 显示角标，
     -- 回 0 会让没进组的产物显示成「0 张的组」。
     MAX(1, (SELECT COUNT(*) FROM media_assets x WHERE x.collection_id = m.collection_id))
       AS collection_size,
     -- 集合的**头三张**成员 id（按 created_at 升序，逗号分隔）—— 叠放封面用。
     -- 放在 SQL 里算，是为了让画廊**一次请求就拿到封面**：
     -- 否则每个集合格子都要再发一次列成员的请求（一页 24 格 = 24 次）。
     -- 没进组时子查询为 NULL → 空串（Dart 侧 split 出来是空数组）。
     COALESCE((SELECT GROUP_CONCAT(x.id) FROM (
                 SELECT id FROM media_assets y
                  WHERE y.collection_id = m.collection_id
                  ORDER BY y.created_at ASC, y.id ASC LIMIT 3
               ) x), '') AS collection_preview";

/// 列表排序用的 `ORDER BY` 片段（**不许直接拼用户输入**，只认这几档）。
fn order_clause(sort: Option<&str>) -> &'static str {
    // `created_at` 只到毫秒：同一批入库（一次运行里几张图、或一次轮询收几条）很容易撞在
    // 同一个毫秒上。**必须带 id 兜底**，否则同毫秒的几条在翻页 / 刷新之间顺序不稳定，
    // 用户看到的是"图在乱跳"。（2026-11-09 查入库顺序时一起补的。）
    match sort {
        Some("oldest") => "m.created_at ASC, m.id ASC",
        Some("updated") => "m.updated_at DESC, m.id DESC",
        Some("name") => "m.original_name ASC, m.id ASC",
        Some("largest") => "m.size_bytes DESC, m.id DESC",
        Some("favorite") => "m.favorite DESC, m.created_at DESC, m.id DESC",
        _ => "m.created_at DESC, m.id DESC",
    }
}

/// `GET /api/media` 的查询参数。
///
/// ⚠ **布尔项一律收成字符串再自己判**（[`bool_param`]），不用 `Option<bool>`：
/// axum 的 `Query` 用的是严格 bool 解析，**只认 `true` / `false`** —— 发 `1` 会
/// **整个请求 400**。而 Dart 那边（以及本仓库的 `untagged: '1'` 写法）发的是 `1`。
/// 实测后果（2026-10-11）：画廊点一下「仅收藏」就变成"加载失败 HTTP 400" ——
/// 这个筛选**一直是坏的**，而且报的是个看不出原因的 400。
/// 收成字符串之后：认 `1/0/true/false/yes/no/on/off`，认不出来**当没传**（绝不 400）。
#[derive(Debug, Deserialize, Default)]
pub struct MediaQuery {
    q: Option<String>,
    tags: Option<String>,
    #[serde(rename = "tagMode")]
    tag_mode: Option<String>,
    kind: Option<String>,
    #[serde(rename = "promptId")]
    prompt_id: Option<i64>,
    /// 只看某一个集合的成员（详情页的"同批"用得上）。
    #[serde(rename = "collectionId")]
    collection_id: Option<i64>,
    /// **折叠集合**（`collapse=1`）：一个集合只留一条（按 `created_at` 最早那条当代表）。
    /// 画廊用它把"同一批微调"显示成一个格子（带叠放封面与角标）。
    collapse: Option<String>,
    favorite: Option<String>,
    untagged: Option<String>,
    sort: Option<String>,
    page: Option<i64>,
    size: Option<i64>,
}

impl MediaQuery {
    fn collapse(&self) -> bool {
        bool_param(self.collapse.as_deref())
    }

    fn favorite(&self) -> bool {
        bool_param(self.favorite.as_deref())
    }

    fn untagged(&self) -> bool {
        bool_param(self.untagged.as_deref())
    }
}

/// 宽松的布尔参数：`1/0/true/false/yes/no/on/off` 都认，其余**当没传**。
///
/// **绝不报错** —— 一个筛选参数写错不该让整个列表变成 400（那正是这个 bug 的形状：
/// 用户看到的是"加载失败 HTTP 400"，完全猜不到是"仅收藏"那个开关干的）。
///
/// `pub(crate)`：`PromptQuery`（提示词 / 预设列表）也吃同一份判据 ——
/// 两处各写一遍的话，"产物列表收 1、提示词列表不收"这种漂移不会有用例变红。
pub(crate) fn bool_param(raw: Option<&str>) -> bool {
    matches!(
        raw.map(|s| s.trim().to_ascii_lowercase()).as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
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

        if let Some(kind) = q
            .kind
            .as_ref()
            .map(|k| k.trim().to_uppercase())
            .filter(|k| !k.is_empty() && k != "ALL")
        {
            clauses.push("m.kind = ?".to_string());
            args.push(Box::new(kind));
        }
        if let Some(prompt_id) = q.prompt_id {
            clauses.push("m.prompt_id = ?".to_string());
            args.push(Box::new(prompt_id));
        }
        if let Some(collection_id) = q.collection_id {
            clauses.push("m.collection_id = ?".to_string());
            args.push(Box::new(collection_id));
        }
        if q.favorite() {
            clauses.push("m.favorite = 1".to_string());
        }
        if q.untagged() {
            clauses.push("m.prompt_id IS NULL".to_string());
        }
        if !clauses.is_empty() {
            where_sql = format!("WHERE {}", clauses.join(" AND "));
        }

        let order = order_clause(q.sort.as_deref());

        // 折叠（`collapse=1`）：一个集合只留**一个代表**，画廊据此把"同一批微调"
        // 显示成一个格子（叠放封面 + 角标）。
        //
        // 代表 = 按 `created_at` 升序的第一条。用 `NOT EXISTS(更早的兄弟)` 表达，
        // **不用 `MIN(id)`** —— 代表必须与 `ORDER BY … ASC` 的第一条严格一致，
        // 否则会出现"折叠后总数对、但某条代表被漏掉"（总数与列表对不上）。
        //
        // ⚠⚠ 这里的写法踩过一次（2026-10-11，上线前真库上量出来的）：
        // 曾经写成"未分组的 **或** 不是任何集合成员"，那等于**把所有集合成员全排除掉** ——
        // 25 条产物折叠后回 21（只有孤儿），**4 个集合的格子在画廊里根本不出现**，
        // 而且 `pages=1` 所以翻页也补不回来。用户看到的是"这几张图没了"。
        // 正确语义是"**未分组的** ∪ **每个集合里最早的那条**"。
        //
        // ⚠ 代价（刻意取舍，别当 bug）：**搜索 / 标签筛选只认代表那条**。
        // 否则会出现"搜到了这一组的第 3 张，但那一组在列表里根本不显示"——
        // 用户看到的是"搜索结果少了几组，而且看不出为什么"。
        // 折叠视图下用户看到的就是代表，所以按代表筛是一致的。
        let collapse = q.collapse();
        let representative_only = if where_sql.is_empty() {
            "WHERE (m.collection_id IS NULL OR NOT EXISTS (
                   SELECT 1 FROM media_assets x
                    WHERE x.collection_id = m.collection_id
                      AND (x.created_at < m.created_at
                           OR (x.created_at = m.created_at AND x.id < m.id))))"
                .to_string()
        } else {
            format!(
                "{where_sql} AND (m.collection_id IS NULL OR NOT EXISTS (
                   SELECT 1 FROM media_assets x
                    WHERE x.collection_id = m.collection_id
                      AND (x.created_at < m.created_at
                           OR (x.created_at = m.created_at AND x.id < m.id))))"
            )
        };
        let total_where = if collapse {
            &representative_only
        } else {
            &where_sql
        };
        let total: i64 = {
            let sql = format!("SELECT COUNT(*) {FROM} {total_where}");
            let mut stmt = conn.prepare(&sql)?;
            let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
            stmt.query_row(refs.as_slice(), |row| row.get(0))?
        };

        // 列表本身也要只留代表 —— 否则"总共 12 组"配上 40 个格子。
        let list_where = if collapse {
            &representative_only
        } else {
            &where_sql
        };
        let sql = format!("SELECT {COLS} {FROM} {list_where} ORDER BY {order} LIMIT ? OFFSET ?");
        let mut stmt = conn.prepare(&sql)?;
        {
            let mut all: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
            let size_box = size;
            let offset_box = offset;
            all.push(&size_box);
            all.push(&offset_box);
            let rows = stmt.query_map(all.as_slice(), media_from_row)?;
            let mut items = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            for item in items.iter_mut() {
                fill_tags(conn, item)?;
            }
            Ok(Paged::new(items, total, page, size))
        }
    })
}

pub fn get(state: &AppState, id: i64) -> Result<MediaDto, ApiError> {
    state
        .with(|conn| {
            let media = conn
                .query_row(
                    &format!("SELECT {COLS} {FROM} WHERE m.id = ?1"),
                    params![id],
                    media_from_row,
                )
                .optional()?;
            match media {
                Some(mut m) => {
                    fill_tags(conn, &mut m)?;
                    Ok(Some(m))
                }
                None => Ok(None),
            }
        })?
        .ok_or_else(|| ApiError::not_found(format!("产物 {id} 不存在")))
}

/// 填两组标签：`own_tags` = 产物自身的；`prompt_tags` = 自身 ∪ 关联提示词的。
///
/// 与 Kotlin `MediaRepo.attachOwnTags` 同义（画廊里点开/筛选时看到的标签集合要一致）。
/// 编辑弹窗用 `own_tags`（只改产物自己的），筛选与展示仍用合并后的 `prompt_tags`。
fn fill_tags(conn: &rusqlite::Connection, media: &mut MediaDto) -> rusqlite::Result<()> {
    let mut merged: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT t.name FROM media_tags mt JOIN tags t ON t.id = mt.tag_id
              WHERE mt.media_id = ?1 ORDER BY t.name",
        )?;
        let rows = stmt.query_map(params![media.id], |row| row.get::<_, String>(0))?;

        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    media.own_tags = merged.clone();
    if let Some(prompt_id) = media.prompt_id {
        let mut stmt = conn.prepare(
            "SELECT t.name FROM prompt_tags pt JOIN tags t ON t.id = pt.tag_id
              WHERE pt.prompt_id = ?1 ORDER BY t.name",
        )?;
        let from_prompt = stmt
            .query_map(params![prompt_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        merged.extend(from_prompt);
    }
    merged.sort();
    merged.dedup();
    media.prompt_tags = merged;
    Ok(())
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
pub async fn media_file(
    state: AppState,
    id: i64,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let media = get(&state, id)?;
    let path = resolve_media(state.media_dir(), &media.stored_name)
        .ok_or_else(|| ApiError::not_found(format!("文件已丢失: {}", media.stored_name)))?;
    let mime = media
        .mime_type
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    serve_path(
        &path,
        media.sha256.as_deref(),
        &mime,
        &media.original_name,
        &headers,
    )
    .await
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
    if let Some(inm) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    {
        if inm.split(',').any(|t| t.trim() == etag || t.trim() == "*") {
            return Ok(Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .header(header::ETAG, etag)
                .body(Body::empty())
                .unwrap());
        }
    }

    let mime = if mime.trim().is_empty() {
        "application/octet-stream"
    } else {
        mime
    };
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
        .map(|c| {
            if c.is_ascii() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
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
    ApiError::not_implemented(format!(
        "{what} 还没搬到 Rust（灰度中，先走 Kotlin 的 8080）"
    ))
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

    let thumb = crate::media::thumbs::thumb_path(state.media_dir(), id);
    let mut from_cache = true;
    if !thumb.is_file() {
        from_cache = false;
        // 生成失败不算错：下面回退原图（与 Kotlin 的 `if (!writeThumbnail(...))` 分支一致）
        let _ = crate::media::thumbs::write_thumbnail(&source, &thumb);
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
/// 按**一组 id** 取产物（`GET /api/presets/{id}/media` 用）。
///
/// 三条：
///  1. **顺序跟着传进来的 id 顺序**（调用方给的是按时间升序的"关联产物"，界面照它排）；
///     所以这里在 Rust 侧重排，而不是靠 SQL 的 `IN`（`IN` 不保证顺序）。
///  2. 空列表直接回空，**不拼一条 `IN ()` 的非法 SQL**。
///  3. 查不到的 id 静默跳过（产物可能刚好被删了）—— 不报错、也不编一条假数据。
pub fn list_by_ids(state: &AppState, ids: &[i64]) -> Result<Vec<MediaDto>, ApiError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    state.with(|conn| {
        let marks = vec!["?"; ids.len()].join(",");
        let sql = format!("SELECT {COLS} {FROM} WHERE m.id IN ({marks})");
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), media_from_row)?;
        let mut found = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        for item in found.iter_mut() {
            fill_tags(conn, item)?;
        }
        // 按调用方给的顺序重排（`IN` 出来的顺序是 SQLite 自己的）
        let mut out: Vec<MediaDto> = Vec::with_capacity(found.len());
        for id in ids {
            if let Some(pos) = found.iter().position(|m| m.id == *id) {
                out.push(found.swap_remove(pos));
            }
        }
        Ok(out)
    })
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
        let rows = stmt.query_map(params![prompt_id], media_from_row)?;
        let mut items = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        for item in items.iter_mut() {
            fill_tags(conn, item)?;
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
pub fn prompt_of_media(
    state: &AppState,
    media_id: i64,
) -> Result<Option<crate::web::models::PromptDto>, ApiError> {
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
                    crate::web::models::prompt_from_row,
                )
                .optional()?;
            match prompt {
                Some(mut p) => {
                    p.tags = crate::web::routes::tags_of(conn, p.id)?;
                    p.media_count = crate::web::routes::media_count_matching(conn, p.id)?;
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

#[cfg(test)]
mod tests;
