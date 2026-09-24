//! P4 灰度端点：业务读/写（`health` / `tags` / `prompts` / `media` / `stats` / 捕获配置）、
//! 同步 4 条、以及 `/api/ai/*` 的**只读前两批**（`providers` / 模型目录）。
//!
//! 各批次清单见 `src/main.rs` 的文件头表（那里是**诚实清单**：没搬的一律 501）。
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
    extract::{FromRequestParts, Path, Query, State},
    http::{request::Parts, StatusCode},
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

    /// 项目根（工具权限与 ComfyUI 探测的基准，AGENTS §7）。
    ///
    /// 与 Kotlin `Config.resolveProjectRoot` **同一套算法**：环境变量 →
    /// `<storage>` 的父目录 → 兜底到 storage 自己。发布包/源码树两种布局都认
    /// （`<根>\.db\comfyhub.db` + `<根>\storage\media`）。
    pub fn project_root(&self) -> std::path::PathBuf {
        if let Ok(raw) = std::env::var("COMFYHUB_PROJECT_ROOT") {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return std::path::PathBuf::from(trimmed);
            }
        }
        let media = self.media_dir();
        let storage = media.parent().unwrap_or(media);
        let is_storage = storage
            .file_name()
            .map(|n| n.to_string_lossy().eq_ignore_ascii_case("storage"))
            .unwrap_or(false);
        if is_storage {
            storage.parent().unwrap_or(storage).to_path_buf()
        } else {
            storage.to_path_buf()
        }
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

/// 路径里的数字 id（`/api/prompts/{id}`、`/api/media/{id}` …）。
///
/// # 为什么不用 `Path<i64>`
///
/// 变量不是数字时 axum 自己回的是 **400 + 纯文本**
/// （`Invalid URL: Cannot parse "abc" to a i64`）—— Dart 的 `jsonDecode` 直接失败，
/// 界面只显示 "HTTP 400"。Kotlin 那边是 `call.requireId() ?: respondBadRequest("非法的 id")`
/// → `{"error":"非法的 id"}`（实测）。
///
/// 这个包装把"路径参数解析失败"也塞回**同一个信封**（`{error, detail?}`）：
/// **Rust 侧任何一个 4xx 都是 JSON**，客户端永远有 `error` 可以给人看。
/// 用法：`Id(id): Id`（元组模式直接解出 `i64`，处理函数里该怎么写还怎么写）。
pub struct Id(pub i64);

impl<S> FromRequestParts<S> for Id
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let raw = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::bad_request("非法的 id"))?;
        raw.0
            .trim()
            .parse::<i64>()
            .map(Id)
            .map_err(|_| ApiError::bad_request("非法的 id"))
    }
}

/// 路径里的**两个**数字 id（`/api/prompts/{id}/tags/{tagId}`）。
///
/// 与 [`Id`] 同理：`Path<(i64, i64)>` 解析失败时是纯文本 400，这里同样塞回信封。
pub struct IdPair(pub i64, pub i64);

impl<S> FromRequestParts<S> for IdPair
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let raw = Path::<(String, String)>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::bad_request("非法的 id"))?;
        let parse = |s: &str| s.trim().parse::<i64>().map_err(|_| ApiError::bad_request("非法的 id"));
        Ok(IdPair(parse(&raw.0 .0)?, parse(&raw.0 .1)?))
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
        // AI 端点第一批（P4 第 16 步）：provider 清单 —— **不含任何密钥**（AIH-012）
        .route("/api/ai/providers", get(ai_providers))
        // 模型目录（P4 第 20 步 = Task 2）：能力声明的唯一真源（AIH-011），附件/思考强度都读它
        .route("/api/ai/providers/{id}/models", get(ai_provider_models))
        // 会话与消息的**读**侧（P4 第 21 步）：AI 工作台打开第一件事就是这三条
        .route("/api/ai/conversations", get(ai_conversations).post(create_conversation))
        .route(
            "/api/ai/conversations/{id}",
            get(ai_conversation).patch(patch_conversation).delete(delete_conversation),
        )
        .route(
            "/api/ai/conversations/{id}/messages",
            get(ai_conversation_messages).post(append_message),
        )
        // run-kit 工件（P4 第 22 步）：手机 harness 的**全部规则类输入**（R12/R13 的第一次重建）
        .route("/api/ai/run-kit", get(ai_run_kit))
        .route("/api/ai/run-kit/status", get(ai_run_kit_status))
        // 跨端待办总览（P4 第 25 步，M1 定的契约）：另一台设备刚连上来时一次补齐所有"在等人"的卡片
        .route("/api/ai/pending", get(ai_pending))
        // 本机 ComfyUI 已保存的工作流文件（只读，用户 bug ⑤）—— **不能少任何一条目录来源**
        .route("/api/capture/workflows", get(capture_workflows))
        // 探测 ComfyUI 位置（只读）+ 用户点「使用这个目录」才写配置
        .route("/api/capture/locate", get(capture_locate))
        .route("/api/capture/locate/apply", axum::routing::post(capture_locate_apply))
        // 工作流原文（前端按需拉：详情页的「查看工作流」弹窗）
        .route("/api/prompts/{id}/workflow", get(prompt_workflow))
        .route("/api/media/{id}/workflow", get(media_workflow))
        // API 格式节点图（"当时真正跑的东西"：comfy_submit 与端到端自测都读它，R10）
        .route("/api/prompts/{id}/api-graph", get(prompt_api_graph))
        // 同步 API（P4 第 10 步）：手机连上来的第一件事（契约 §1）
        .route("/api/sync/manifest", get(sync_manifest))
        .route("/api/sync/changes", get(sync_changes))
        .route("/api/sync/push", axum::routing::post(sync_push))
        .route("/api/sync/bootstrap", get(sync_bootstrap))
        .merge(crate::write_routes::write_router())
        // 配对闸门（P4 第 23 步）：`/api/auth*` + **密钥下行** + 令牌校验中间件。
        // ⚠ 顺序要紧：`.layer()` 只作用于**它之前**加进来的路由，所以这里必须排在
        // 所有 `.route()` / `.merge()` 之后（排在前面就会漏掉一半端点，而且不报错）。
        .merge(crate::auth::router())
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::auth::gate))
        // 未知路径也要给**同样形状**的错误体（默认 fallback 是空 body，界面只会显示 "HTTP 404"）
        .fallback(fallback_route)
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
    Id(id): Id,
) -> Result<Json<crate::models::MediaDto>, ApiError> {
    Ok(Json(crate::media::get(&state, id)?))
}

async fn media_file(
    State(state): State<AppState>,
    Id(id): Id,
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
    Id(id): Id,
) -> Result<axum::response::Response, ApiError> {
    crate::media::media_thumb(state, id).await
}

async fn media_poster(Id(_id): Id) -> Result<Json<serde_json::Value>, ApiError> {
    Err(crate::media::not_migrated("视频封面"))
}

/// `GET /api/media/{id}/prompt`：204 = 这条产物没关联提示词（Dart 的 `mediaPrompt` 把 204 当 null）。
async fn media_prompt(
    State(state): State<AppState>,
    Id(id): Id,
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
    Id(id): Id,
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

/// `GET /api/capture/workflows?q=&limit=` —— 本机 ComfyUI 已保存的工作流文件（**只读**）。
///
/// 用户 bug ⑤：`prompts` 库只收"捕获过的运行"与"手动读进来的文件"，所以首次使用时库里是空的，
/// 而用户机器上早就存着工作流 —— 这个端点把它们摆出来（不写库、不动用户的文件）。
async fn capture_workflows(
    State(state): State<AppState>,
    Query(query): Query<crate::comfy_dirs::WorkflowQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::comfy_dirs::workflows_json(&state, query)?))
}

/// `GET /api/ai/providers` —— provider 清单（不含密钥，AIH-012）。
async fn ai_providers(State(state): State<AppState>) -> Result<Json<Vec<crate::ai::AiProviderDto>>, ApiError> {
    Ok(Json(crate::ai::list_providers(&state)?))
}

/// `GET /api/ai/providers/{id}/models` —— 模型目录（P4 第 20 步 = Task 2）。
///
/// provider 不存在 → **400 `{error:"CONFIG_ERROR", detail:"Provider 不存在: xx"}`**
/// （不是 404：客户端按 `error` 分支、按 `detail` 给人看，形状必须与 Kotlin 的
/// `AiException` 一致 —— Task 1 那个"界面显示机器码"的 bug 就是这么来的）。
async fn ai_provider_models(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
) -> Result<Json<Vec<crate::ai::AiModelDto>>, ApiError> {
    Ok(Json(crate::ai::list_models(&state, &provider_id)?))
}

#[derive(Debug, Deserialize, Default)]
struct ConversationQuery {
    /// Kotlin 只认 `1` 与 `true`（不区分大小写），别的值一律当 false。
    #[serde(rename = "includeArchived")]
    include_archived: Option<String>,
}

/// `GET /api/ai/conversations?includeArchived=1|true`。
async fn ai_conversations(
    State(state): State<AppState>,
    Query(q): Query<ConversationQuery>,
) -> Result<Json<Vec<crate::conversations::AiConversationDto>>, ApiError> {
    let include = q
        .include_archived
        .as_deref()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    Ok(Json(crate::conversations::list(&state, include, None)?))
}

/// `GET /api/ai/conversations/{id}`（不存在 → 400 CONFIG_ERROR + detail）。
async fn ai_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::conversations::AiConversationDto>, ApiError> {
    Ok(Json(crate::conversations::get(&state, &id)?))
}

/// `GET /api/ai/conversations/{id}/messages` —— 消息 + **有序块**（工具卡 / 思考段全靠它）。
async fn ai_conversation_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<crate::conversations::AiMessageDto>>, ApiError> {
    Ok(Json(crate::conversations::list_messages(&state, &id)?))
}

/// `GET /api/ai/run-kit` 的查询参数（手机按它选好的模型取环境头；两个都可选）。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RunKitQuery {
    provider_id: Option<String>,
    model_id: Option<String>,
}

/// `GET /api/ai/run-kit` —— **手机 harness 的全部"规则类"输入**（P4 第 22 步，R13）。
///
/// `?providerId=&modelId=` 缺省时用桌面第一个启用的 Provider 与它的第一个启用模型；
/// 指定了但库里没有 / 没启用 → **400 `CONFIG_ERROR`**（不静默回落 —— 那会让手机拿着
/// 一份"提示词里写的模型和它要用的不一样"的 kit 还不自知）。
async fn ai_run_kit(
    State(state): State<AppState>,
    Query(q): Query<RunKitQuery>,
) -> Result<Json<crate::run_kit::RunKitDto>, ApiError> {
    Ok(Json(crate::run_kit::build(
        &state,
        q.provider_id.as_deref(),
        q.model_id.as_deref(),
    )?))
}

/// `GET /api/ai/run-kit/status` —— `{current, revoked}`（手机跑之前先问一句吊销）。
async fn ai_run_kit_status(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::run_kit::status(&state)?))
}

/// `GET /api/ai/pending` —— **跨端待办总览**（P4 第 25 步，M1 定的契约）。
///
/// 另一台设备（手机 / 第二个窗口 / 刚重启的桌面 App）没经历那条 SSE 事件流，
/// 靠这一个接口把所有"在等人"的卡片一次补齐：AI 的提问 + 需要批准的工具调用。
async fn ai_pending(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(crate::pending::pending(&state)?))
}

/// `GET /api/capture/locate` —— 探测 ComfyUI 装在哪（用户"其他建议"第 3 条）。
///
/// **只读**：探测结果不会自动生效，用户点「使用这个目录」才会写进配置（下一个端点）。
async fn capture_locate(
    State(state): State<AppState>,
) -> Result<Json<crate::comfy_dirs::ComfyLocation>, ApiError> {
    Ok(Json(crate::comfy_dirs::locate_json(&state)?))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocateApplyRequest {
    output_dir: Option<String>,
}

/// `POST /api/capture/locate/apply` —— 把探测到的输出目录写进配置。
///
/// 两条准入照抄 Kotlin：目录**不能为空**、**必须真的存在**（都要 400 说清楚），
/// 不然会把一个打不开的路径写进配置，之后"捕获一条也收不到"而用户不知道为什么。
async fn capture_locate_apply(
    State(state): State<AppState>,
    Json(body): Json<LocateApplyRequest>,
) -> Result<Json<crate::capture::CaptureConfig>, ApiError> {
    let dir = body.output_dir.unwrap_or_default().trim().to_string();
    if dir.is_empty() {
        return Err(ApiError::bad_request("outputDir 不能为空"));
    }
    if !std::path::Path::new(&dir).is_dir() {
        return Err(ApiError::bad_request(format!("目录不存在：{dir}")));
    }
    let mut current = crate::capture::capture_config(&state)?;
    current.output_dir = Some(dir);
    Ok(Json(crate::capture::put_config(&state, current)?))
}

/// `GET /api/prompts/{id}/workflow` —— 有就原样吐 `application/json`，没有就 **204**。
async fn prompt_workflow(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<axum::response::Response, ApiError> {
    workflow_response(crate::capture::prompt_workflow(&state, id)?)
}

/// `GET /api/media/{id}/workflow`
async fn media_workflow(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<axum::response::Response, ApiError> {
    workflow_response(crate::capture::media_workflow(&state, id)?)
}

/// `GET /api/prompts/{id}/api-graph` —— **API 格式**节点图（提交给 ComfyUI 的那一份）。
///
/// 与 `/workflow` 的三分（404 / 204 / 200）形状相同，但取数链条完全不同：
/// 这里是 `source_ref → capture_runs.raw → HistoryEntry`（见 `capture::prompt_api_graph`）。
async fn prompt_api_graph(
    State(state): State<AppState>,
    Id(id): Id,
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

/// `GET /api/sync/manifest` —— 手机连上来的第一件事（🔒 `docs/sync-api-contract.md` §1）。
async fn sync_manifest(
    State(state): State<AppState>,
) -> Result<Json<crate::sync::SyncManifest>, ApiError> {
    Ok(Json(crate::sync::manifest(&state)?))
}

/// `GET /api/sync/changes?since=<cursor>&limit=500` —— 增量（契约 §2）。
///
/// ⚠ 游标落在保留窗口之外时**必须**回 `410 Gone`（见 `ApiError::gone`）。
async fn sync_changes(
    State(state): State<AppState>,
    Query(q): Query<SyncChangesQuery>,
) -> Result<Json<crate::sync::SyncChanges>, ApiError> {
    Ok(Json(crate::sync::changes(&state, q.since.unwrap_or(0), q.limit.unwrap_or(500))?))
}

#[derive(Debug, Deserialize)]
struct SyncChangesQuery {
    since: Option<i64>,
    limit: Option<i64>,
}

/// `POST /api/sync/push` —— 上行（契约 §4）。幂等 + 服务端赢 LWW。
async fn sync_push(
    State(state): State<AppState>,
    Json(body): Json<crate::sync::PushRequest>,
) -> Result<Json<crate::sync::PushResult>, ApiError> {
    Ok(Json(crate::sync::push(&state, body)?))
}

/// `GET /api/sync/bootstrap?table=<表>&page=N` —— 首次全量（契约 §3）。
///
/// ⚠ `table` 是**实现时补的必填参数**（契约 §3 的留白，理由写在 `sync::bootstrap` 的注释里）。
async fn sync_bootstrap(
    State(state): State<AppState>,
    Query(q): Query<BootstrapQuery>,
) -> Result<Json<crate::sync::BootstrapPage>, ApiError> {
    let table = q
        .table
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "缺 table 参数；合法值：{}",
                crate::sync::SYNCED.join(", ")
            ))
        })?;
    Ok(Json(crate::sync::bootstrap(
        &state,
        table,
        q.page.unwrap_or(1),
        q.page_size.unwrap_or(200),
    )?))
}

#[derive(Debug, Deserialize)]
struct BootstrapQuery {
    table: Option<String>,
    page: Option<i64>,
    #[serde(rename = "pageSize")]
    page_size: Option<i64>,
}

// ---------------------------------------------------------------------------
//  会话与消息的**写**侧（P4 第 24 步）
// ---------------------------------------------------------------------------
//
// 为什么这一批重要：手机的 M4 上行只推它**自己产生的行**（`/api/sync/push`），
// 而"桌面在手机那条会话上接着聊"必须由桌面自己写 —— 写侧不搬，指向 Rust 后端时
// 能看见历史却**动不了它**（界面会拿着 404/501 一脸茫然）。

/// `POST /api/ai/conversations` → **201 Created**（Kotlin 也是 201）。
async fn create_conversation(
    State(state): State<AppState>,
    Json(body): Json<crate::conversations::ConversationCreate>,
) -> Result<(StatusCode, Json<crate::conversations::AiConversationDto>), ApiError> {
    Ok((StatusCode::CREATED, Json(crate::conversations::create(&state, &body)?)))
}

/// `PATCH /api/ai/conversations/{id}` —— 改标题 / 换模型 / 归档（缺的字段一律不改）。
async fn patch_conversation(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<crate::conversations::ConversationPatch>,
) -> Result<Json<crate::conversations::AiConversationDto>, ApiError> {
    Ok(Json(crate::conversations::patch(&state, &id, &body)?))
}

/// `DELETE /api/ai/conversations/{id}` —— 消息与块靠外键级联（R1 是它的前提）。
async fn delete_conversation(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::conversations::delete(&state, &id)?;
    Ok(Json(serde_json::json!({ "deleted": true, "id": id })))
}

/// `POST /api/ai/conversations/{id}/messages` → **201 Created** + 落库后的那条消息。
async fn append_message(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<crate::conversations::MessageAppend>,
) -> Result<(StatusCode, Json<crate::conversations::AiMessageDto>), ApiError> {
    Ok((StatusCode::CREATED, Json(crate::conversations::append_message(&state, &id, &body)?)))
}

// ---------------------------------------------------------------------------
//  错误
// ---------------------------------------------------------------------------

/// 统一错误响应（形状**逐字对齐 Kotlin 侧的 `ApiError`**：`{error, detail?}`）。
///
/// # 两种形态（Kotlin `Application.kt` 的 StatusPages 就是这么分的）
///
/// | 来源 | `error` | `detail` | 例子 |
/// | --- | --- | --- | --- |
/// | 普通错误（`respondError` / `respondNotFound` / `respondBadRequest`） | **人话** | 不出现 | `{"error":"非法的 id"}` |
/// | AI 领域错误（`AiException`，AIH-024） | **稳定码** | 人话 | `{"error":"CONFIG_ERROR","detail":"会话不存在: 9"}` |
///
/// # ⚠ 为什么键名是 `error` 而不是 `message`（2026-09-24 对齐）
///
/// Dart 侧 `ApiClient._decode`（`lib/core/api_client.dart:50`）读的是 `error` 当人话、
/// `detail` 当附加说明；`AiApiClient._decode` 反过来**先读 `detail` 再读 `error`**。
/// 早先这里写的是 `{error:"NOT_FOUND", message:"提示词 999 不存在"}` ——
/// 那两个客户端都读不到人话，界面显示的是 `NOT_FOUND` 这个机器码。
///
/// # ⚠ `detail == None` 时**整键省略**（不是 `"detail": null`）
///
/// Kotlin 的 `AppJson` 是 `explicitNulls = false`（`server/src/main/kotlin/com/comfyhub/Json.kt:12`），
/// 实测：`GET /api/prompts/abc` → `{"error":"非法的 id"}`，**没有 `detail` 键**。
/// 见 `tests/shape_lint.rs` —— 那里有一条"镜像 DTO 不许吐 null"的防线。
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    /// 响应体 `error` 键的值（普通错误 = 人话；AI 错误 = 稳定码）。
    pub error: String,
    /// 响应体 `detail` 键的值；`None` = 这个键**根本不出现**。
    pub detail: Option<String>,
}

impl ApiError {
    fn new(status: StatusCode, error: impl Into<String>, detail: Option<String>) -> Self {
        Self { status, error: error.into(), detail }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message, None)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message, None)
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message, None)
    }

    /// **`410 Gone`** —— 同步游标落在保留窗口之外（契约 §2）。
    ///
    /// 手机收到它就丢弃镜像、改走 `/api/sync/bootstrap`。
    /// ⚠ 这个分支**不是可选项**：缺了它，手机少掉的那几百条**永远补不回来**
    /// （增量接口照着旧游标往后扫，永远扫不到那些被裁掉的洞）。
    ///
    /// `error` 照样是**人话**（信封规则对同步端点一视同仁）；手机判 410 认的是**状态码**。
    pub fn gone(message: impl Into<String>) -> Self {
        Self::new(StatusCode::GONE, message, None)
    }

    /// 还没搬到 Rust 的端点：**明确 501**，不要拿空数组假装成功（那会让界面以为"真的没有"）。
    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_IMPLEMENTED, message, None)
    }

    /// **AI 领域错误**（`AiException` 的对应物，AIH-024）：`error` = 稳定码、`detail` = 人话。
    ///
    /// 状态码照 Kotlin 的 StatusPages 固定 **400**（`UNSUPPORTED_CONTENT` / `UNKNOWN_MODEL` /
    /// `CONFIG_ERROR` … 都是 400），客户端拿 `error` 分支、拿 `detail` 给用户看。
    /// 空的人话回落到"只给码"（Kotlin 那边 `cause.message` 为 null 时 `detail` 就是不出现）。
    pub fn ai(code: impl Into<String>, message: impl Into<String>) -> Self {
        let message = message.into();
        Self::new(
            StatusCode::BAD_REQUEST,
            code,
            (!message.trim().is_empty()).then_some(message),
        )
    }

    /// **只给内部用**的人类可读说明（普通错误 = `error`，AI 错误 = `detail`）。
    ///
    /// 用在"把这个错误当成一句话塞进另一个结构"的地方，例如
    /// `sync.push` 的 `rejected[].message`、上传时单个文件失败的原因。
    /// **它不进错误响应体**（响应体只有 `error` / `detail` 两个键）。
    pub fn human(&self) -> &str {
        self.detail.as_deref().unwrap_or(&self.error)
    }

    /// **`403`：没带令牌**（或这台设备没被授权做这件事）。
    ///
    /// ⚠ 与 `401` 的语义**不能混**（`docs/pairing-design.md` §7）：
    /// `403` = "你还没证明自己是谁"，`401` = "你证明了自己，但钥匙不对/已被吊销"。
    /// 混了以后用户看到的那句话会指向错误的排查方向（该去配对，却跑去重新配对）。
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message, None)
    }

    /// **`401`：带了令牌但不对 / 已被吊销**。
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message, None)
    }

    /// **`429`：配对请求太频繁**（按来源地址限速）。
    pub fn new_rate_limited(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, message, None)
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        Self::internal(format!("数据库错误：{e}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // 手拼对象而**不用 json!({"detail": self.detail})**：那样 `None` 会变成 `"detail": null`，
        // 与 `explicitNulls = false` 的 Kotlin 响应不是同一个形状（多一个键就是多一块差异）。
        let mut obj = serde_json::Map::with_capacity(2);
        obj.insert("error".into(), serde_json::Value::String(self.error));
        if let Some(detail) = self.detail {
            obj.insert("detail".into(), serde_json::Value::String(detail));
        }
        (self.status, Json(serde_json::Value::Object(obj))).into_response()
    }
}

/// 没匹配到任何路由 → `404 {"error":"接口不存在: <路径>"}`。
///
/// Kotlin 的 StatusPages 对未知路径也给这句话（`Application.kt:589`），
/// 但它**把状态码写成了 200**（`call.respond` 在 status 处理器里会重置状态）——
/// 那是 Kotlin 侧的 bug，这里**只对齐 body、不对齐那个 200**：
/// 未知路径必须仍是 404，否则任何"路径写错"都会被界面当成成功。
async fn fallback_route(uri: axum::http::Uri) -> ApiError {
    ApiError::not_found(format!("接口不存在: {}", uri.path()))
}

// ---------------------------------------------------------------------------
//  /api/health
// ---------------------------------------------------------------------------

/// Kotlin 的 `APP_VERSION`（`Application.kt:56`）—— 一个字都别改，设置页会显示它。
const APP_VERSION: &str = "1.0.0";

/// `GET /api/health`。
///
/// # ⚠ 四个键是**承重的**，不是装饰（2026-09-24 对齐时抓到）
///
/// `lib/core/backend_launcher.dart:271/379/426` 把 **`health['database'] == "ok"`** 当作
/// "后端已就绪"的唯一判据：拿不到这个键，它会一直认为库坏了，去走"修复"那条路
/// （表现是启动时反复重试 —— 而这不是"少显示一个字段"那么轻）。
/// 设置页还读 `version`（连接成功那句）与 `storageDir`（显示 + 「打开目录」按钮）。
///
/// 也就是说 P4 切流那天，`{status, db, impl, schemaVersion, engine}` 这份形状会让
/// **App 认为后端没起来**。现在与 Kotlin 的 `HealthDto` 对齐：`status` / `version` /
/// `database` / `storageDir` / `serverTime`；另外三样是 Rust 侧多给的诊断信息
/// （`doctor`、灰度期分辨"是哪一版后端"用），客户端按 key 取值，多给不影响。
async fn health(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let storage_dir = storage_dir_of(&state);
    let (engine, schema) = state.with(|conn| Ok((db::engine_info(conn)?, db::schema_version(conn)?)))?;

    // `database` 就是 Kotlin 那句 "SELECT 1" 的结果（ok / error: …），不是常量
    let database = match state.with(|conn| Ok(conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?)) {
        Ok(_) => "ok".to_string(),
        Err(e) => format!("error: {}", e.human()),
    };
    let server_time: String =
        state.with(|conn| Ok(conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| r.get(0))?))?;

    Ok(Json(json!({
        "status": if database == "ok" { "ok" } else { "degraded" },
        "version": APP_VERSION,
        "database": database,
        "storageDir": storage_dir,
        "serverTime": server_time,
        // ↓ Rust 侧多给的（不影响任何客户端；灰度期用来认"这是哪一版后端"）
        "impl": "rust",
        "db": state.db_path,
        "schemaVersion": schema,
        "engine": engine,
    })))
}

/// `<storage>`（`media_dir` 的父目录）—— Kotlin `cfg.storageDir` 的对应物。
fn storage_dir_of(state: &AppState) -> String {
    let media = state.media_dir();
    media
        .parent()
        .unwrap_or(media)
        .to_string_lossy()
        .to_string()
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
    Id(id): Id,
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
