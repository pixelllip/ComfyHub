//! 存储占用 / 孤儿清理 / 事件归档的 HTTP 面（2026-11-06）。
//!
//! 三个端点：
//!
//! | 端点 | 干什么 | 只许回环 |
//! | --- | --- | --- |
//! | `GET /api/system/storage` | 各目录占用 + `ai_run_events` 规模 | 否（只读） |
//! | `POST /api/system/storage/orphans` | `apply=false` 扫描 / `apply=true` 删除 | 删除时是 |
//! | `POST /api/system/events/archive` | 把 N 天前的事件写成 JSONL 再删库 | 是 |
//!
//! 统计要遍历目录（`storage/ai` 里的 MCP 运行时可能上千个文件），所以走
//! `spawn_blocking`，别占着事件循环。删除类端点只允许本机调用：它们是**不可撤销**的。

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ai::runs;
use crate::infra::storage as disk;
use crate::web::remote::MaybeRemote;
use crate::web::routes::{ApiError, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/system/storage", get(storage_usage))
        .route("/api/system/storage/orphans", post(orphans))
        .route("/api/system/events/archive", post(archive))
}

/// `GET /api/system/storage` —— 占用 + 事件规模。
async fn storage_usage(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let root = state.project_root();
    let storage = crate::media::attachments::storage_root(&state);
    let db = std::path::PathBuf::from(&state.db_path);
    let usage = tokio::task::spawn_blocking(move || disk::usage(&root, &storage, &db))
        .await
        .map_err(|e| ApiError::internal(format!("统计任务失败：{e}")))?;
    let events = runs::event_stats(&state)?;

    let mut body = serde_json::to_value(usage)
        .map_err(|e| ApiError::internal(format!("序列化失败：{e}")))?;
    body["events"] = serde_json::to_value(events)
        .map_err(|e| ApiError::internal(format!("序列化失败：{e}")))?;
    Ok(Json(body))
}

#[derive(Debug, Deserialize)]
struct OrphanInput {
    /// 缺省 = 只扫描（**不删**）。删除必须显式传 `true`。
    #[serde(default)]
    apply: bool,
}

/// `POST /api/system/storage/orphans` —— 扫描或删除。
///
/// ⚠ 删除**不认请求体里的文件清单**：进来重新扫一遍再删。让请求体带路径等于把
/// "删任意文件"的权利交给调用方 —— 一个路径穿越就能删掉 `storage` 之外的东西。
async fn orphans(
    State(state): State<AppState>,
    remote: MaybeRemote,
    Json(input): Json<OrphanInput>,
) -> Result<Json<Value>, ApiError> {
    if input.apply && !crate::web::remote::is_loopback(&remote) {
        return Err(ApiError::forbidden(
            "清理文件只能在这台机器上操作 —— 删了就找不回来了",
        ));
    }
    let storage = crate::media::attachments::storage_root(&state);
    let apply = input.apply;
    let state_for_task = state.clone();
    let value = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let guard = state_for_task
            .conn
            .lock()
            .map_err(|_| "连接锁已中毒".to_string())?;
        let now = std::time::SystemTime::now();
        let value = if apply {
            serde_json::to_value(disk::clean_orphans(&guard, &storage, now)?)
        } else {
            serde_json::to_value(disk::scan_orphans(&guard, &storage, now)?)
        }
        .map_err(|e| e.to_string())?;
        Ok(value)
    })
    .await
    .map_err(|e| ApiError::internal(format!("清理任务失败：{e}")))?
    .map_err(ApiError::bad_request)?;
    Ok(Json(value))
}

#[derive(Debug, Deserialize)]
struct ArchiveInput {
    /// 保留多少天（默认 30，夹在 1..3650）。
    #[serde(default = "default_days")]
    days: i64,
}

fn default_days() -> i64 {
    30
}

/// `POST /api/system/events/archive` —— 归档并删除旧事件。
async fn archive(
    State(state): State<AppState>,
    remote: MaybeRemote,
    Json(input): Json<ArchiveInput>,
) -> Result<Json<Value>, ApiError> {
    if !crate::web::remote::is_loopback(&remote) {
        return Err(ApiError::forbidden(
            "归档事件只能在这台机器上操作 —— 它会往磁盘写文件并从库里删行",
        ));
    }
    let dir = crate::media::attachments::storage_root(&state)
        .join("ai")
        .join("run-events");
    let state_for_task = state.clone();
    let report = tokio::task::spawn_blocking(move || {
        runs::archive_events(&state_for_task, input.days, &dir)
    })
    .await
    .map_err(|e| ApiError::internal(format!("归档任务失败：{e}")))??;
    Ok(Json(json!({ "report": report })))
}
