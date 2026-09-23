//! 写端点的路由装配（P4 第 2 步）。
//!
//! # 与读端点同一套形状纪律
//!
//! 路径、请求体字段、响应体字段都照抄 Kotlin 侧（`TagRoutes.kt` / `PromptRoutes.kt`）。
//! 特别是几处**容易写错**的：
//!
//! | 端点 | 状态码 / 响应 | 出处 |
//! | --- | --- | --- |
//! | `POST /api/tags` | **201** + `TagDto` | `TagRoutes.kt:46` |
//! | `PUT /api/tags/{id}` | 200 + `TagDto`；不存在 404；重名 400 | `TagRoutes.kt:53` |
//! | `DELETE /api/tags/{id}` | 200 + `{"deleted": id}` | `TagRoutes.kt:62` |
//! | `POST /api/prompts` | **201** + `PromptDto` | `PromptRoutes.kt:52` |
//! | `POST /api/prompts/{id}/favorite` | 200 + `{"id", "favorite"}`（**两个键**，见 `inputs.rs` 注释） | `PromptRoutes.kt:93` |
//! | `POST /api/prompts/{id}/duplicate` | **201** + 新 `PromptDto` | `PromptRoutes.kt:81` |
//! | `DELETE /api/prompts/{id}` | 200 + `{"deleted": id}` | `PromptRoutes.kt:74` |
//!
//! # 写路径必须过事务
//!
//! 每个写端点都是 `state.write(...)` → `db::tx_immediate`（**R2**）。理由不只是并发：
//! "建提示词 + 建标签 + 重算 use_count" 这三步**必须在同一个事务里**，
//! 否则中间崩一次就会留下"标签建了、关联没建、计数还是旧的"这种脏状态。
//! 顺带：同一个事务也让 `_sync_log` 的触发器行与业务行**一起提交**（同步不会看到半截）。

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};

use crate::inputs::{Deleted, FavoriteInput, FavoriteResult, PromptInput, TagInput, TagNamesInput};
use crate::models::{prompt_from_row, tag_from_row, PromptDto, TagDto};
use crate::repo::{self, WriteError};
use crate::routes::{ApiError, AppState};

/// 写端点的路由表（**不在这里 `with_state`**：要和读端点那份 merge 之后再统一上状态，
/// 否则两份 `Router` 的状态类型对不上 —— axum 的 `merge` 要求两侧同状态）。
pub fn write_router() -> Router<AppState> {
    Router::new()
        .route("/api/tags", post(create_tag))
        .route("/api/tags/{id}", put(update_tag).delete(delete_tag))
        .route("/api/prompts", post(create_prompt))
        .route("/api/prompts/{id}", put(update_prompt).delete(delete_prompt))
        .route("/api/prompts/{id}/favorite", post(favorite))
        .route("/api/prompts/{id}/duplicate", post(duplicate))
        .route("/api/prompts/{id}/tags", post(add_tags))
        .route("/api/prompts/{id}/tags/{tag_id}", delete(remove_tag))
        .route("/api/media/{id}", patch(patch_media).delete(delete_media))
        .route("/api/prompts/{id}/media", get(list_media_stub))
}

/// `PATCH /api/media/{id}`：改标题 / 关联提示词 / 收藏 / 备注 / NSFW 标记。
async fn patch_media(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<crate::inputs::MediaUpdate>,
) -> Result<Json<crate::models::MediaDto>, ApiError> {
    Ok(Json(crate::media_write::update(&state, id, &body)?))
}

/// `DELETE /api/media/{id}`：删库 + 删磁盘文件（回 `{"deleted": id}`）。
async fn delete_media(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Deleted>, ApiError> {
    crate::media_write::delete(&state, id)?;
    Ok(Json(Deleted { deleted: id }))
}

/// 产物列表还没搬（`/api/media` 属于后面几批）；**明确 501**，不要给个空数组假装成功。
async fn list_media_stub(Path(_id): Path<i64>) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_implemented("产物端点还没搬到 Rust（灰度中，先走 Kotlin 的 8080）"))
}

impl From<WriteError> for ApiError {
    fn from(e: WriteError) -> Self {
        match e {
            WriteError::Validation(message) => ApiError::bad_request(message),
            WriteError::Conflict(message) => ApiError::bad_request(message),
            WriteError::NotFound => ApiError::not_found("目标不存在"),
            WriteError::Db(err) => ApiError::internal(format!("数据库错误：{err}")),
        }
    }
}

fn read_tag(state: &AppState, id: i64) -> Result<Json<TagDto>, ApiError> {
    state
        .with(|conn| {
            conn.query_row("SELECT * FROM tags WHERE id = ?1", params![id], |row| tag_from_row(row))
                .optional()
        })?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("标签 {id} 不存在")))
}

fn read_prompt(state: &AppState, id: i64) -> Result<Json<PromptDto>, ApiError> {
    state
        .with(|conn| {
            let prompt = conn
                .query_row("SELECT * FROM prompts WHERE id = ?1", params![id], |row| prompt_from_row(row))
                .optional()?;
            match prompt {
                Some(mut p) => {
                    p.tags = crate::routes::tags_of(conn, p.id)?;
                    p.media_count = crate::routes::media_count_of(conn, p.id)?;
                    Ok(Some(p))
                }
                None => Ok(None),
            }
        })?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("提示词 {id} 不存在")))
}

// ---------------------------------------------------------------------------
//  标签
// ---------------------------------------------------------------------------

async fn create_tag(
    State(state): State<AppState>,
    Json(input): Json<TagInput>,
) -> Result<(StatusCode, Json<TagDto>), ApiError> {
    let id = state.write(|tx| {
        let id = repo::ensure_tag(tx, &input.name, input.category.as_deref(), input.color.as_deref())?
            .ok_or_else(|| WriteError::Validation("标签名不能为空".into()))?;
        repo::update_tag(tx, id, &input)?;
        Ok(id)
    })?;
    let dto = read_tag(&state, id)?;
    Ok((StatusCode::CREATED, dto))
}

async fn update_tag(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<TagInput>,
) -> Result<Json<TagDto>, ApiError> {
    let ok = state.write(|tx| repo::update_tag(tx, id, &input))?;
    if !ok {
        return Err(ApiError::not_found(format!("标签 {id} 不存在")));
    }
    read_tag(&state, id)
}

async fn delete_tag(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Deleted>, ApiError> {
    let ok = state.write(|tx| repo::delete_tag(tx, id))?;
    if !ok {
        return Err(ApiError::not_found(format!("标签 {id} 不存在")));
    }
    Ok(Json(Deleted { deleted: id }))
}

// ---------------------------------------------------------------------------
//  提示词
// ---------------------------------------------------------------------------

async fn create_prompt(
    State(state): State<AppState>,
    Json(input): Json<PromptInput>,
) -> Result<(StatusCode, Json<PromptDto>), ApiError> {
    let id = state.write(|tx| repo::create_prompt(tx, &input))?;
    let dto = read_prompt(&state, id)?;
    Ok((StatusCode::CREATED, dto))
}

async fn update_prompt(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<PromptInput>,
) -> Result<Json<PromptDto>, ApiError> {
    let ok = state.write(|tx| repo::update_prompt(tx, id, &input))?;
    if !ok {
        return Err(ApiError::not_found(format!("提示词 {id} 不存在")));
    }
    read_prompt(&state, id)
}

async fn delete_prompt(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Deleted>, ApiError> {
    let ok = state.write(|tx| repo::delete_prompt(tx, id))?;
    if !ok {
        return Err(ApiError::not_found(format!("提示词 {id} 不存在")));
    }
    Ok(Json(Deleted { deleted: id }))
}

/// 收藏 / 取消收藏。
///
/// ⚠ 响应是 `{"id", "favorite"}` 两个键（Kotlin 侧这里踩过"混类型 map 让序列化抛 500"的坑）。
async fn favorite(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<FavoriteInput>,
) -> Result<Json<FavoriteResult>, ApiError> {
    let ok = state.write(|tx| repo::set_favorite(tx, id, body.favorite))?;
    if !ok {
        return Err(ApiError::not_found(format!("提示词 {id} 不存在")));
    }
    Ok(Json(FavoriteResult { id, favorite: body.favorite }))
}

async fn duplicate(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<(StatusCode, Json<PromptDto>), ApiError> {
    // 先把源读齐（DTO + 标签），再在**一个写事务里**建副本
    let source = state.with(|conn| {
        let prompt = conn
            .query_row("SELECT * FROM prompts WHERE id = ?1", params![id], |row| prompt_from_row(row))
            .optional()?;
        let Some(mut p) = prompt else { return Ok(None) };
        p.tags = crate::routes::tags_of(conn, p.id)?;
        p.media_count = crate::routes::media_count_of(conn, p.id)?;
        Ok(Some(p))
    })?;
    let Some(prompt) = source else {
        return Err(ApiError::not_found(format!("提示词 {id} 不存在")));
    };
    let tags = prompt.tags.clone();
    let new_id = state.write(|tx| {
        repo::duplicate_prompt(tx, id, &repo::PromptWithTags { prompt: prompt.clone(), tags: tags.clone() })
    })?;
    let dto = read_prompt(&state, new_id)?;
    Ok((StatusCode::CREATED, dto))
}

async fn add_tags(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<TagNamesInput>,
) -> Result<Json<PromptDto>, ApiError> {
    let exists = state.with(|conn| {
        Ok(conn
            .query_row("SELECT id FROM prompts WHERE id = ?1", params![id], |row| row.get::<_, i64>(0))
            .optional()?
            .is_some())
    })?;
    if !exists {
        return Err(ApiError::not_found(format!("提示词 {id} 不存在")));
    }
    state.write(|tx| {
        repo::add_prompt_tags(tx, id, &body.tags)?;
        repo::refresh_use_counts(tx)
    })?;
    read_prompt(&state, id)
}

async fn remove_tag(
    State(state): State<AppState>,
    Path((id, tag_id)): Path<(i64, i64)>,
) -> Result<Json<PromptDto>, ApiError> {
    let exists = state.with(|conn| {
        Ok(conn
            .query_row("SELECT id FROM prompts WHERE id = ?1", params![id], |row| row.get::<_, i64>(0))
            .optional()?
            .is_some())
    })?;
    if !exists {
        return Err(ApiError::not_found(format!("提示词 {id} 不存在")));
    }
    state.write(|tx| {
        repo::remove_prompt_tag(tx, id, tag_id)?;
        repo::refresh_use_counts(tx)
    })?;
    read_prompt(&state, id)
}
