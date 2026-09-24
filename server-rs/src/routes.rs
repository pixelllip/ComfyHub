//! P4 灰度第一批端点：`/api/health`、`/api/tags`、`/api/prompts`（**只读**）。
//!
//! # 为什么从这三个开始
//!
//! 总纲 §5.1 的灰度顺序是"先非 AI 端点（tags / prompts）"。理由是它们**只读、无副作用、
//! 无上游调用** —— 灰度期间"Kotlin 与 Rust 同时在跑"，读端点错了最多显示不对，
//! 不会造成数据损坏；而写端点（`POST /api/prompts` 等）要等读端点稳定、
//! 且 `/api/sync/push` 那样的幂等语义也铺好之后再搬。
//!
//! # 一条硬纪律：**形状一个字都不改**
//!
//! Flutter 一行不改是 P4 的验收前提，所以这里的路径、查询参数、JSON 字段名都照抄
//! Kotlin 侧（`PromptRoutes.kt` / `TagRoutes.kt` / `Repo` 的 `search`）：
//!
//! | 端点 | 查询参数 |
//! | --- | --- |
//! | `GET /api/tags` | `q` / `category` / `sort=popular\|name` / `limit` |
//! | `GET /api/tags/categories` | — |
//! | `GET /api/prompts` | `q` / `kind` / `favorite` / `sort=newest\|oldest\|title` / `page` / `size` |
//! | `GET /api/prompts/{id}` | — |

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::json;
use std::sync::{Arc, Mutex};

use crate::db;
use crate::models::{prompt_from_row, tag_from_row, Paged, PromptDto, TagDto};

/// 共享状态：一条被 `Mutex` 护住的连接。
///
/// ⚠ 这是**灰度期的权宜之计**，不是目标形态：总纲 §5.1 要求"写走 `tokio-rusqlite` 的专属线程
/// （写天然串行化，彻底不会 `SQLITE_BUSY`），读走一小池只读连接"。
/// 现在只有读端点，先用一条连接把形状与形状用例铺出来；写端点落地时再换。
#[derive(Clone)]
pub struct AppState {
    pub conn: Arc<Mutex<Connection>>,
    pub db_path: String,
    /// 产物原件目录（`<storage>/media`）——文件流端点要它。
    media_dir: Arc<std::path::PathBuf>,
}

impl AppState {
    pub fn new(conn: Connection, db_path: impl Into<String>, media_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: db_path.into(),
            media_dir: Arc::new(media_dir.into()),
        }
    }

    pub fn media_dir(&self) -> &std::path::Path {
        self.media_dir.as_path()
    }

    pub fn with<F, T>(&self, f: F) -> Result<T, ApiError>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T>,
    {
        let guard = self.conn.lock().map_err(|_| ApiError::internal("连接锁已中毒"))?;
        f(&guard).map_err(ApiError::from)
    }

    /// 写路径：**一律过 `BEGIN IMMEDIATE`**（红线 R2）。
    ///
    /// 事务纪律只有 `db::tx_immediate` 一处实现 —— 端点里别自己拼 `BEGIN`/`COMMIT`。
    pub fn write<F, T>(&self, f: F) -> Result<T, ApiError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, crate::repo::WriteError>,
    {
        let mut guard = self.conn.lock().map_err(|_| ApiError::internal("连接锁已中毒"))?;
        db::tx_immediate(&mut guard, f).map_err(ApiError::from)
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/tags", get(list_tags))
        .route("/api/tags/categories", get(list_tag_categories))
        .route("/api/prompts", get(list_prompts))
        .route("/api/prompts/{id}", get(get_prompt))
        .route("/api/stats", get(get_stats))
        // 产物（P4 第 3 步）：列表 / 详情 / **原件文件流**（Range + ETag）
        .route("/api/media", get(list_media))
        .route("/api/media/{id}", get(get_media))
        .route("/api/media/{id}/file", get(media_file))
        // 还没搬的：缩略图要图像解码器、封面要 ffmpeg 抽帧、上传要 multipart —— 一律 501
        .route("/api/media/{id}/thumb", get(media_thumb))
        .route("/api/media/{id}/poster", get(media_poster))
        .route("/api/media/{id}/prompt", get(media_prompt))
        .route("/api/prompts/{id}/media", get(prompt_media))
        .route("/api/media/upload", axum::routing::post(media_upload))
        // 捕获配置（本地那一半：读写 app_settings 里的一行 JSON）
        .route("/api/capture/config", get(capture_config).put(put_capture_config))
        // 工作流原文（前端按需拉：详情页的「查看工作流」弹窗）
        .route("/api/prompts/{id}/workflow", get(prompt_workflow))
        .route("/api/media/{id}/workflow", get(media_workflow))
        // API 格式节点图（"当时真正跑的东西"：comfy_submit 与端到端自测都读它，R10）
        .route("/api/prompts/{id}/api-graph", get(prompt_api_graph))
        .merge(crate::write_routes::write_router())
        .with_state(state)
}

// ---------------------------------------------------------------------------
//  产物（列表 / 详情 / 文件流）
// ---------------------------------------------------------------------------

async fn list_media(
    State(state): State<AppState>,
    Query(q): Query<crate::media::MediaQuery>,
) -> Result<Json<Paged<crate::models::MediaDto>>, ApiError> {
    Ok(Json(crate::media::search(&state, &q)?))
}

async fn get_media(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<crate::models::MediaDto>, ApiError> {
    Ok(Json(crate::media::get(&state, id)?))
}

async fn media_file(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    crate::media::media_file(state, id, headers).await
}

/// 顶部那几个数字（`GET /api/stats`）。
async fn get_stats(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::media_write::stats(&state)?))
}

async fn media_thumb(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::response::Response, ApiError> {
    crate::media::media_thumb(state, id).await
}

async fn media_poster(Path(_id): Path<i64>) -> Result<Json<serde_json::Value>, ApiError> {
    Err(crate::media::not_migrated("视频封面"))
}

/// `GET /api/media/{id}/prompt`：204 = 这条产物没关联提示词（Dart 的 `mediaPrompt` 把 204 当 null）。
async fn media_prompt(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::response::Response, ApiError> {
    match crate::media::prompt_of_media(&state, id)? {
        None => Ok(axum::response::Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(axum::body::Body::empty())
            .unwrap()),
        Some(prompt) => Ok(Json(prompt).into_response()),
    }
}

/// `GET /api/prompts/{id}/media`：这条提示词下的全部产物。
async fn prompt_media(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<crate::models::MediaDto>>, ApiError> {
    Ok(Json(crate::media::list_by_prompt(&state, id)?))
}

/// `GET /api/capture/config`
async fn capture_config(State(state): State<AppState>) -> Result<Json<crate::capture::CaptureConfig>, ApiError> {
    Ok(Json(crate::capture::get_config(&state)?))
}

/// `PUT /api/capture/config`（整体覆盖 + 归一化，与 Kotlin 同语义）
async fn put_capture_config(
    State(state): State<AppState>,
    Json(body): Json<crate::capture::CaptureConfig>,
) -> Result<Json<crate::capture::CaptureConfig>, ApiError> {
    Ok(Json(crate::capture::put_config(&state, body)?))
}

/// `GET /api/prompts/{id}/workflow` —— 有就原样吐 `application/json`，没有就 **204**。
async fn prompt_workflow(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::response::Response, ApiError> {
    workflow_response(crate::capture::prompt_workflow(&state, id)?)
}

/// `GET /api/media/{id}/workflow`
async fn media_workflow(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::response::Response, ApiError> {
    workflow_response(crate::capture::media_workflow(&state, id)?)
}

/// `GET /api/prompts/{id}/api-graph` —— **API 格式**节点图（提交给 ComfyUI 的那一份）。
///
/// 与 `/workflow` 的三分（404 / 204 / 200）形状相同，但取数链条完全不同：
/// 这里是 `source_ref → capture_runs.raw → HistoryEntry`（见 `capture::prompt_api_graph`）。
async fn prompt_api_graph(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::response::Response, ApiError> {
    match crate::capture::prompt_api_graph(&state, id)? {
        // 没有 API 图 → 204：调用方据此如实说"这条跑不了"，而不是当成"提示词不存在"
        None => Ok(axum::response::Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(axum::body::Body::empty())
            .unwrap()),
        Some(graph) => Ok(axum::response::Response::builder()
            .status(StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(graph.to_string()))
            .unwrap()),
    }
}

/// 工作流原文的响应：**原样文本**（不重新序列化：可能有几百 KB，前端按原样塞进弹窗）
fn workflow_response(text: Option<String>) -> Result<axum::response::Response, ApiError> {
    match text {
        None => Ok(axum::response::Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(axum::body::Body::empty())
            .unwrap()),
        Some(json) => Ok(axum::response::Response::builder()
            .status(StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(json))
            .unwrap()),
    }
}

async fn media_upload(
    State(state): State<AppState>,
    multipart: axum::extract::Multipart,
) -> Result<(axum::http::StatusCode, Json<crate::media_upload::UploadResult>), ApiError> {
    crate::media_upload::upload(state, multipart).await
}

// ---------------------------------------------------------------------------
//  错误
// ---------------------------------------------------------------------------

/// 统一错误响应（形状对齐 Kotlin 侧的 `ApiError`：`{error, message}`）。
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub error: String,
    pub message: String,
}

impl ApiError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self { status: StatusCode::NOT_FOUND, error: "NOT_FOUND".into(), message: message.into() }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, error: "INTERNAL".into(), message: message.into() }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, error: "BAD_REQUEST".into(), message: message.into() }
    }

    /// 还没搬到 Rust 的端点：**明确 501**，不要拿空数组假装成功（那会让界面以为"真的没有"）。
    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self { status: StatusCode::NOT_IMPLEMENTED, error: "NOT_IMPLEMENTED".into(), message: message.into() }
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        Self::internal(format!("数据库错误：{e}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.error, "message": self.message }))).into_response()
    }
}

// ---------------------------------------------------------------------------
//  /api/health
// ---------------------------------------------------------------------------

async fn health(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let (engine, schema) = state.with(|conn| Ok((db::engine_info(conn)?, db::schema_version(conn)?)))?;
    Ok(Json(json!({
        "status": "ok",
        "impl": "rust",
        "db": state.db_path,
        "schemaVersion": schema,
        "engine": engine,
    })))
}

// ---------------------------------------------------------------------------
//  /api/tags
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct TagQuery {
    q: Option<String>,
    category: Option<String>,
    sort: Option<String>,
    limit: Option<i64>,
}

async fn list_tags(
    State(state): State<AppState>,
    Query(q): Query<TagQuery>,
) -> Result<Json<Vec<TagDto>>, ApiError> {
    let limit = q.limit.unwrap_or(500).clamp(1, 2000);
    let keyword = q.q.as_ref().map(|s| format!("%{}%", s.trim()));
    let tags = state.with(|conn| {
        // 排序跟 Kotlin `TagRepo.list` 一致：popular = 使用次数降序，其次按名字
        let order = match q.sort.as_deref() {
            Some("name") => "name COLLATE NOCASE ASC",
            _ => "use_count DESC, name COLLATE NOCASE ASC",
        };
        let sql = format!(
            "SELECT * FROM tags
              WHERE (?1 IS NULL OR name LIKE ?1 OR normalized LIKE ?1)
                AND (?2 IS NULL OR category = ?2)
              ORDER BY {order} LIMIT ?3"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![keyword, q.category, limit], |row| tag_from_row(row))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })?;
    Ok(Json(tags))
}

async fn list_tag_categories(State(state): State<AppState>) -> Result<Json<Vec<String>>, ApiError> {
    let categories = state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT category FROM tags WHERE category IS NOT NULL AND category <> '' ORDER BY category",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })?;
    Ok(Json(categories))
}

// ---------------------------------------------------------------------------
//  /api/prompts
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct PromptQuery {
    q: Option<String>,
    kind: Option<String>,
    favorite: Option<bool>,
    sort: Option<String>,
    page: Option<i64>,
    size: Option<i64>,
}

async fn list_prompts(
    State(state): State<AppState>,
    Query(q): Query<PromptQuery>,
) -> Result<Json<Paged<PromptDto>>, ApiError> {
    let page = q.page.unwrap_or(1).max(1);
    let size = q.size.unwrap_or(20).clamp(1, 200);
    let offset = (page - 1) * size;
    let keyword = q.q.as_ref().map(|s| format!("%{}%", s.trim()));
    let favorite = q.favorite.map(|f| if f { 1_i64 } else { 0_i64 });

    let paged = state.with(|conn| {
        let where_sql = "WHERE (?1 IS NULL OR title LIKE ?1 OR positive_prompt LIKE ?1 OR negative_prompt LIKE ?1)
                           AND (?2 IS NULL OR kind = ?2)
                           AND (?3 IS NULL OR favorite = ?3)";
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM prompts {where_sql}"),
            rusqlite::params![keyword, q.kind, favorite],
            |row| row.get(0),
        )?;
        let order = match q.sort.as_deref() {
            Some("oldest") => "created_at ASC, id ASC",
            Some("title") => "title COLLATE NOCASE ASC, id ASC",
            _ => "created_at DESC, id DESC",
        };
        let sql = format!("SELECT * FROM prompts {where_sql} ORDER BY {order} LIMIT ?4 OFFSET ?5");
        let mut stmt = conn.prepare(&sql)?;
        let mut items = stmt
            .query_map(rusqlite::params![keyword, q.kind, favorite, size, offset], |row| {
                prompt_from_row(row)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for item in items.iter_mut() {
            item.tags = tags_of(conn, item.id)?;
            item.media_count = media_count_of(conn, item.id)?;
        }
        Ok(Paged::new(items, total, page, size))
    })?;
    Ok(Json(paged))
}

async fn get_prompt(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<PromptDto>, ApiError> {
    let prompt = state.with(|conn| {
        let mut stmt = conn.prepare("SELECT * FROM prompts WHERE id = ?1")?;
        let mut rows = stmt.query_map(rusqlite::params![id], |row| prompt_from_row(row))?;
        match rows.next() {
            Some(row) => {
                let mut prompt = row?;
                prompt.tags = tags_of(conn, prompt.id)?;
                prompt.media_count = media_count_of(conn, prompt.id)?;
                Ok(Some(prompt))
            }
            None => Ok(None),
        }
    })?;
    prompt
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("提示词 {id} 不存在")))
}

/// 一个提示词的标签（按名字排序，与 Kotlin 侧一致）。
pub(crate) fn tags_of(conn: &Connection, prompt_id: i64) -> rusqlite::Result<Vec<TagDto>> {
    let mut stmt = conn.prepare(
        "SELECT t.* FROM tags t
           JOIN prompt_tags pt ON pt.tag_id = t.id
          WHERE pt.prompt_id = ?1
          ORDER BY t.name COLLATE NOCASE ASC",
    )?;
    let rows = stmt.query_map(rusqlite::params![prompt_id], |row| tag_from_row(row))?;
    rows.collect()
}

pub(crate) fn media_count_of(conn: &Connection, prompt_id: i64) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM media_assets WHERE prompt_id = ?1",
        rusqlite::params![prompt_id],
        |row| row.get(0),
    )
}
