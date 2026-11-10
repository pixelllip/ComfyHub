//! 备份与恢复的 HTTP 面（`/api/system/backups*` + `/api/system/restore`）。
//!
//! 逻辑全在 [`crate::infra::backup`]（`VACUUM INTO` / 标记 / 换文件），这里只做三件事：
//! 取参数、把阻塞 IO 丢给 `spawn_blocking`、把人话错误塞回统一信封。
//!
//! # 为什么恢复要 `restartRequired`
//!
//! 换库只能发生在**没有任何连接**的时候（理由见 `infra/backup.rs` 的文件头）。后端没法
//! 自己把正在跑的连接收掉再打开一条（还有后台轮询 / SSE / 执行器都握着 `AppState`），
//! 所以它只落一个标记，由 App 重启后端来完成最后一步 —— 界面上看到的就是
//! "已准备好，正在重启本地服务…"。
//!
//! # 为什么恢复只允许回环
//!
//! 它会把这台机器上的库换掉。配对令牌是给"手机连桌面"用的，不该顺手把"清空桌面数据"
//! 也授权出去。备份 / 列表 / 删除不设这条限制（它们只碰 `backups\` 目录，破坏面小）。

use axum::{
    extract::{Path as UrlPath, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

use crate::infra::backup;
use crate::web::remote::MaybeRemote;
use crate::web::routes::{ApiError, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/system/backups", get(list_backups).post(create_backup))
        .route(
            "/api/system/backups/{name}",
            axum::routing::delete(delete_backup),
        )
        .route("/api/system/restore", post(restore))
}

fn db_path_of(state: &AppState) -> PathBuf {
    PathBuf::from(&state.db_path)
}

/// `GET /api/system/backups` —— 目录、清单、有没有待完成的恢复、上一次恢复的结果。
///
/// 四个键一起给：界面要同时回答"备份在哪 / 有哪些 / 我刚才点的那次成了没成"。
/// 缺少 `lastRestore` 时，重启后只能看到一个没有变化的列表 —— 无法判断恢复
/// 到底做没做（这正是"界面看不见的现象只能靠猜"那一类）。
async fn list_backups(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let root = state.project_root();
    let items = backup::list(&root).map_err(ApiError::internal)?;
    let db = db_path_of(&state);
    Ok(Json(json!({
        "dir": backup::dir(&root).to_string_lossy(),
        "items": items,
        "pending": backup::pending(&db),
        "lastRestore": backup::last_restore(&db),
    })))
}

/// `POST /api/system/backups` —— 现在拍一张。响应 `201` + `{"item": {...}}`。
async fn create_backup(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let root = state.project_root();
    let state_for_task = state.clone();
    // `VACUUM INTO` 会把整库读一遍写一遍：别占着事件循环
    let item = tokio::task::spawn_blocking(move || {
        let guard = state_for_task
            .conn
            .lock()
            .map_err(|_| "连接锁已中毒".to_string())?;
        backup::create(&guard, &root)
    })
    .await
    .map_err(|e| ApiError::internal(format!("备份任务失败：{e}")))?
    .map_err(ApiError::internal)?;
    Ok((StatusCode::CREATED, Json(json!({ "item": item }))))
}

/// `DELETE /api/system/backups/{name}` —— 删一份。不存在回 404（不假装成功）。
async fn delete_backup(
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    let root = state.project_root();
    let name_for_task = name.clone();
    let removed = tokio::task::spawn_blocking(move || backup::delete(&root, &name_for_task))
        .await
        .map_err(|e| ApiError::internal(format!("删除任务失败：{e}")))?
        .map_err(ApiError::bad_request)?;
    if !removed {
        return Err(ApiError::not_found(format!("没有这份备份：{name}")));
    }
    Ok(Json(json!({ "deleted": name })))
}

#[derive(Debug, Deserialize)]
struct RestoreInput {
    name: String,
}

/// `POST /api/system/restore` —— 校验 + 拍保险 + 落标记；真正换库在**下次启动**。
///
/// 响应：`{"ok": true, "name": …, "safety": …, "restartRequired": true}`。
async fn restore(
    State(state): State<AppState>,
    remote: MaybeRemote,
    Json(input): Json<RestoreInput>,
) -> Result<Json<Value>, ApiError> {
    if !crate::web::remote::is_loopback(&remote) {
        return Err(ApiError::forbidden(
            "恢复库只能在这台机器上操作 —— 它会替换掉当前数据库",
        ));
    }
    if state.db_path == ":memory:" {
        return Err(ApiError::bad_request(
            "当前用的是内存库，没有可以替换的文件",
        ));
    }
    let root = state.project_root();
    let db = db_path_of(&state);
    let state_for_task = state.clone();
    let name = input.name.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        let guard = state_for_task
            .conn
            .lock()
            .map_err(|_| "连接锁已中毒".to_string())?;
        backup::restore_prepare(&guard, &root, &db, &name)
    })
    .await
    .map_err(|e| ApiError::internal(format!("恢复任务失败：{e}")))?
    .map_err(ApiError::bad_request)?;

    Ok(Json(json!({
        "ok": true,
        "name": prepared.name,
        "safety": prepared.safety,
        "restartRequired": true,
    })))
}
