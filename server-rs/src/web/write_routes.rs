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
    extract::State,
    http::StatusCode,
    routing::{delete, patch, post, put},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};

use crate::library::repo::{self, WriteError};
use crate::web::inputs::{
    Deleted, FavoriteInput, FavoriteResult, PromptInput, TagInput, TagNamesInput,
};
use crate::web::models::{prompt_from_row, tag_from_row, PromptDto, TagDto};
use crate::web::routes::{ApiError, AppState, Id, IdPair};

/// 写端点的路由表（**不在这里 `with_state`**：要和读端点那份 merge 之后再统一上状态，
/// 否则两份 `Router` 的状态类型对不上 —— axum 的 `merge` 要求两侧同状态）。
pub fn write_router() -> Router<AppState> {
    Router::new()
        .route("/api/tags", post(create_tag))
        .route("/api/tags/{id}", put(update_tag).delete(delete_tag))
        .route("/api/prompts", post(create_prompt))
        .route(
            "/api/prompts/{id}",
            put(update_prompt).delete(delete_prompt),
        )
        .route("/api/prompts/{id}/favorite", post(favorite))
        .route("/api/prompts/{id}/duplicate", post(duplicate))
        .route("/api/prompts/{id}/tags", post(add_tags))
        .route("/api/prompts/{id}/tags/{tag_id}", delete(remove_tag))
        .route("/api/media/{id}", patch(patch_media).delete(delete_media))
        // 产物标签（2026-11-10）：手动加 / 删。整体替换走 `PATCH /api/media/{id}`
        // 的 `tags`；这两条是"只加一条 / 只删一条"的细粒度入口。
        .route("/api/media/{id}/tags", post(add_media_tags))
        .route("/api/media/{id}/tags/{tag_id}", delete(remove_media_tag))
        // 给库里已有的提示词与产物补自动标签（标签页的「补标签」按钮）。
        .route("/api/tags/auto-apply", post(auto_apply_tags))
        // 手动集合（2026-10-18）：建一个集合并把选中的产物一起移进去。
        // 读那一半（`GET /api/media/collections`）在读端点表里。
        .route("/api/media/collections", post(create_collection))
        // 改名（2026-11-07）：展开详情里点「重命名集合」；改名同时把它转成 manual，
        // 免得「重新自动整理」下次把名字连同集合一起解散。
        .route("/api/media/collections/{id}", patch(rename_collection))
        // 「重新自动整理」：把 `origin='auto'` 的集合全解散再按当前规则重分。
        // ⚠ 与上面那条是**两个互相独立**的静态路径（不是同一路径的不同方法，
        // 也不是动态段），所以两条都要写出来 —— 少了任何一条，对应操作都是 404/405。
        .route("/api/media/collections/regroup", post(regroup_collections))
}

/// `POST /api/media/collections/regroup` 的响应体：`{"regrouped": 12}`。
///
/// 键名是**动词的过去式**（与 `{"deleted": id}` 同一套风格）：`regrouped` 是这次
/// 分进组的产物条数，口径与启动回填的返回值一致（孤儿 / 已移出 / 正文空白都不算）。
#[derive(Debug, serde::Serialize)]
pub struct RegroupResult {
    pub regrouped: usize,
}

/// `POST /api/media/collections/regroup`：**重新自动整理**（2026-10-18）。
///
/// 语义全在 [`crate::media::collection::regroup_auto`] 里，这里只负责"过事务 + 回 JSON"：
/// 解散 auto、重分、清空集合是**一个整体**，中间崩一次就会留下"成员被摘下来了、
/// 新组还没建"的半截状态（产物暂时不属于任何集合，且不报错）。
///
/// ⚠ 写端点必须注册在 `write_router()` 里（`state.write` 用的是 `BEGIN IMMEDIATE`）；
/// 读到只读路由表里会没有事务，与红线 R2 冲突。
async fn regroup_collections(
    State(state): State<AppState>,
) -> Result<Json<RegroupResult>, ApiError> {
    // `regroup_auto` 收 `&Connection`，`Transaction` 靠 `Deref` 递过去（与 `create_manual` 同一条路）。
    let regrouped = state.write(|tx| crate::media::collection::regroup_auto(tx))?;
    Ok(Json(RegroupResult { regrouped }))
}

/// `POST /api/media/collections` 的请求体。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCollectionInput {
    pub media_ids: Vec<i64>,
    /// 集合名字，可空（界面会催一句，但不强制）。
    #[serde(default)]
    pub title: Option<String>,
}

/// `POST /api/media/collections`：**新建手动集合**（201 + 新集合摘要）。
///
/// 一次调用同时做三件事，所以必须在一个事务里：建集合行、把选中产物挂进去、
/// 收拾被搬空的老集合。中间崩一次就会留下"集合建了、成员没进去"的空壳。
///
/// ⚠ 拒绝的情况（400）与拒绝的**理由**：
/// * 选中的产物 kind 不一致（"一个集合只能装一种"）；
/// * 一条都没选。
async fn create_collection(
    State(state): State<AppState>,
    Json(body): Json<CreateCollectionInput>,
) -> Result<(StatusCode, Json<crate::web::models::MediaCollectionDto>), ApiError> {
    let created = state.write(|tx| {
        crate::media::collection::create_manual(tx, &body.media_ids, body.title.as_deref())
    })?;
    // 直接按 id 取摘要：默认名 / 成员数都由后端算好，别让界面再猜一遍。
    let summary = state.with(|conn| {
        crate::media::collection::summary(conn, created).map_err(crate::web::routes::as_sql_error)
    })?;
    Ok((StatusCode::CREATED, Json(summary.into())))
}

/// `PATCH /api/media/collections/{id}` 的请求体。
#[derive(Debug, serde::Deserialize)]
pub struct RenameCollectionInput {
    pub title: String,
}

/// `PATCH /api/media/collections/{id}`：给集合改名（2026-11-07）。
///
/// 空名字回 400（界面上不允许把名字清回"这一组"）；改名同时把 `origin` 翻成
/// `manual`（见 `collection::rename` 的注释）。
async fn rename_collection(
    State(state): State<AppState>,
    Id(id): Id,
    Json(body): Json<RenameCollectionInput>,
) -> Result<Json<crate::web::models::MediaCollectionDto>, ApiError> {
    let summary = state.write(|tx| crate::media::collection::rename(tx, id, &body.title))?;
    Ok(Json(summary.into()))
}

/// `PATCH /api/media/{id}`：改标题 / 关联提示词 / 收藏 / 备注 / NSFW 标记。
async fn patch_media(
    State(state): State<AppState>,
    Id(id): Id,
    Json(body): Json<crate::web::inputs::MediaUpdate>,
) -> Result<Json<crate::web::models::MediaDto>, ApiError> {
    Ok(Json(crate::media::media_write::update(&state, id, &body)?))
}

/// `DELETE /api/media/{id}`：删库 + 删磁盘文件（回 `{"deleted": id}`）。
async fn delete_media(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<Json<Deleted>, ApiError> {
    crate::media::media_write::delete(&state, id)?;
    Ok(Json(Deleted { deleted: id }))
}

/// 产物是否存在（标签子路由用它把"不存在"转换为 404，而不是让外键返回 500）。
fn media_exists(state: &AppState, id: i64) -> Result<bool, ApiError> {
    state.with(|conn| {
        Ok(conn
            .query_row(
                "SELECT id FROM media_assets WHERE id = ?1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    })
}

/// `POST /api/media/{id}/tags`：给产物**追加**标签（不删已有的）。
async fn add_media_tags(
    State(state): State<AppState>,
    Id(id): Id,
    Json(body): Json<TagNamesInput>,
) -> Result<Json<crate::web::models::MediaDto>, ApiError> {
    if !media_exists(&state, id)? {
        return Err(ApiError::not_found(format!("产物 {id} 不存在")));
    }
    state.write(|tx| {
        repo::append_media_tags(tx, id, &body.tags)?;
        repo::refresh_use_counts(tx)
    })?;
    Ok(Json(crate::media::media::get(&state, id)?))
}

/// `DELETE /api/media/{id}/tags/{tag_id}`：只解除这一条关联，标签本身留在词表里。
async fn remove_media_tag(
    State(state): State<AppState>,
    IdPair(id, tag_id): IdPair,
) -> Result<Json<crate::web::models::MediaDto>, ApiError> {
    if !media_exists(&state, id)? {
        return Err(ApiError::not_found(format!("产物 {id} 不存在")));
    }
    state.write(|tx| {
        repo::remove_media_tag(tx, id, tag_id)?;
        repo::refresh_use_counts(tx)
    })?;
    Ok(Json(crate::media::media::get(&state, id)?))
}

/// `POST /api/tags/auto-apply`：给库里已有的提示词与产物补一遍自动标签。
async fn auto_apply_tags(
    State(state): State<AppState>,
) -> Result<Json<crate::library::auto_tag::AutoTagReport>, ApiError> {
    Ok(Json(crate::library::auto_tag::apply_existing(&state)?))
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
            conn.query_row(
                "SELECT * FROM tags WHERE id = ?1",
                params![id],
                tag_from_row,
            )
            .optional()
        })?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("标签 {id} 不存在")))
}

fn read_prompt(state: &AppState, id: i64) -> Result<Json<PromptDto>, ApiError> {
    state
        .with(|conn| {
            let prompt = conn
                .query_row(
                    "SELECT * FROM prompts WHERE id = ?1",
                    params![id],
                    prompt_from_row,
                )
                .optional()?;
            match prompt {
                Some(mut p) => {
                    p.tags = crate::web::routes::tags_of(conn, p.id)?;
                    p.media_count = crate::web::routes::media_count_matching(conn, p.id)?;
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
        let id = repo::ensure_tag(
            tx,
            &input.name,
            input.category.as_deref(),
            input.color.as_deref(),
        )?
        .ok_or_else(|| WriteError::Validation("标签名不能为空".into()))?;
        repo::update_tag(tx, id, &input)?;
        Ok(id)
    })?;
    let dto = read_tag(&state, id)?;
    Ok((StatusCode::CREATED, dto))
}

async fn update_tag(
    State(state): State<AppState>,
    Id(id): Id,
    Json(input): Json<TagInput>,
) -> Result<Json<TagDto>, ApiError> {
    let ok = state.write(|tx| repo::update_tag(tx, id, &input))?;
    if !ok {
        return Err(ApiError::not_found(format!("标签 {id} 不存在")));
    }
    read_tag(&state, id)
}

async fn delete_tag(State(state): State<AppState>, Id(id): Id) -> Result<Json<Deleted>, ApiError> {
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
    Id(id): Id,
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
    Id(id): Id,
) -> Result<Json<Deleted>, ApiError> {
    let ok = state.write(|tx| repo::delete_prompt(tx, id))?;
    if !ok {
        return Err(ApiError::not_found(format!("提示词 {id} 不存在")));
    }
    Ok(Json(Deleted { deleted: id }))
}

/// 收藏 / 取消收藏。
///
/// ⚠ 响应是 `{"id", "favorite"}` 两个键（Kotlin 侧出现过"混类型 map 使序列化抛 500"的问题）。
async fn favorite(
    State(state): State<AppState>,
    Id(id): Id,
    Json(body): Json<FavoriteInput>,
) -> Result<Json<FavoriteResult>, ApiError> {
    let ok = state.write(|tx| repo::set_favorite(tx, id, body.favorite))?;
    if !ok {
        return Err(ApiError::not_found(format!("提示词 {id} 不存在")));
    }
    Ok(Json(FavoriteResult {
        id,
        favorite: body.favorite,
    }))
}

async fn duplicate(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<(StatusCode, Json<PromptDto>), ApiError> {
    // 先把源读齐（DTO + 标签），再在**一个写事务里**建副本
    let source = state.with(|conn| {
        let prompt = conn
            .query_row(
                "SELECT * FROM prompts WHERE id = ?1",
                params![id],
                prompt_from_row,
            )
            .optional()?;
        let Some(mut p) = prompt else { return Ok(None) };
        p.tags = crate::web::routes::tags_of(conn, p.id)?;
        p.media_count = crate::web::routes::media_count_matching(conn, p.id)?;
        Ok(Some(p))
    })?;
    let Some(prompt) = source else {
        return Err(ApiError::not_found(format!("提示词 {id} 不存在")));
    };
    let tags = prompt.tags.clone();
    let new_id = state.write(|tx| {
        repo::duplicate_prompt(
            tx,
            id,
            &repo::PromptWithTags {
                prompt: prompt.clone(),
                tags: tags.clone(),
            },
        )
    })?;
    let dto = read_prompt(&state, new_id)?;
    Ok((StatusCode::CREATED, dto))
}

async fn add_tags(
    State(state): State<AppState>,
    Id(id): Id,
    Json(body): Json<TagNamesInput>,
) -> Result<Json<PromptDto>, ApiError> {
    let exists = state.with(|conn| {
        Ok(conn
            .query_row("SELECT id FROM prompts WHERE id = ?1", params![id], |row| {
                row.get::<_, i64>(0)
            })
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
    IdPair(id, tag_id): IdPair,
) -> Result<Json<PromptDto>, ApiError> {
    let exists = state.with(|conn| {
        Ok(conn
            .query_row("SELECT id FROM prompts WHERE id = ?1", params![id], |row| {
                row.get::<_, i64>(0)
            })
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
