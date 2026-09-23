//! 捕获配置与工作流原文（P4 第 7 步）。
//!
//! 这一批刻意**只搬"本地、不依赖 ComfyUI 在线"的端点**：
//!
//! | 端点 | 为什么能先搬 |
//! | --- | --- |
//! | `GET/PUT /api/capture/config` | 就是 `app_settings` 的一行 JSON（设置页读写它） |
//! | `GET /api/prompts/{id}/workflow` | 读库里的 `workflow_json` 文本 |
//! | `GET /api/media/{id}/workflow` | 同上（产物自身的快照） |
//!
//! **没搬**（都需要 ComfyUI 在场，属于下一批）：`/capture/status`、`/capture/jobs`（要真的去问队列）、
//! `/capture/poll`、`/capture/locate{,/apply}`、`/capture/import*`、`/ingest/comfyui`、
//! `/prompts/{id}/api-graph`（要从 `capture_runs.raw` 里取 API 图，还要跑一遍图解析）。
//!
//! # 两条容易写错的语义
//!
//! 1. **`workflow_json` 为 NULL → 204**（不是 404、也不是 `null` 正文）：
//!    前端据此区分"这条老数据没有工作流"与"这条提示词根本不存在"（后者才是 404）。
//! 2. **`PUT /config` 是整体覆盖 + 归一化**（App 的设置页总是回传完整配置）：
//!    `comfyUrl` 去尾斜杠、空了回落 `http://127.0.0.1:8188`；`pollSeconds`/`maxPerPoll` **夹到合法区间**；
//!    `outputDir` 空串当 NULL。**不做字段级合并** —— 语义更简单，也不会出现"一半新一半旧"。

use axum::extract::{Path, State};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::routes::{ApiError, AppState};

/// 自动捕获配置（与 Kotlin `CaptureConfig` 逐字同形）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_comfy_url")]
    pub comfy_url: String,
    #[serde(default)]
    pub output_dir: Option<String>,
    #[serde(default = "default_poll_seconds")]
    pub poll_seconds: i64,
    #[serde(default = "default_auto_tag")]
    pub auto_tag: String,
    #[serde(default = "default_max_per_poll")]
    pub max_per_poll: i64,
    #[serde(default = "default_true")]
    pub download_fallback: bool,
}

fn default_true() -> bool {
    true
}
fn default_comfy_url() -> String {
    "http://127.0.0.1:8188".to_string()
}
fn default_poll_seconds() -> i64 {
    4
}
fn default_auto_tag() -> String {
    "ComfyUI".to_string()
}
fn default_max_per_poll() -> i64 {
    20
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            comfy_url: default_comfy_url(),
            output_dir: None,
            poll_seconds: default_poll_seconds(),
            auto_tag: default_auto_tag(),
            max_per_poll: default_max_per_poll(),
            download_fallback: true,
        }
    }
}

const KEY_CAPTURE: &str = "capture.config";

/// 读配置：库里没有（或解析不了）就用**环境变量给的环境默认值**。
///
/// ⚠ 发布包/源码树两条路的默认值来自脚本传的环境变量（Kotlin 侧是 `AppConfig`），
/// 所以这里也认 `COMFYHUB_COMFY_URL` / `COMFYHUB_COMFY_OUTPUT_DIR` ——
/// 否则 Rust 版会在"用户还没在设置页改过"时给出一份与 Kotlin 不同的默认值。
pub fn capture_config(state: &AppState) -> Result<CaptureConfig, ApiError> {
    let raw: Option<String> = state.with(|conn| {
        Ok(conn
            .query_row("SELECT v FROM app_settings WHERE k = ?1", params![KEY_CAPTURE], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten())
    })?;
    let env_default = CaptureConfig {
        comfy_url: std::env::var("COMFYHUB_COMFY_URL").unwrap_or_else(|_| default_comfy_url()),
        output_dir: std::env::var("COMFYHUB_COMFY_OUTPUT_DIR").ok().filter(|v| !v.trim().is_empty()),
        ..CaptureConfig::default()
    };
    match raw {
        Some(text) if !text.trim().is_empty() => Ok(serde_json::from_str::<CaptureConfig>(&text)
            .map_err(|_| ())
            .unwrap_or(env_default)),
        _ => Ok(env_default),
    }
}

/// 存配置：**先归一化再落库**，返回归一化后的那份（前端拿它刷新界面）。
pub fn save_capture_config(state: &AppState, config: CaptureConfig) -> Result<CaptureConfig, ApiError> {
    let normalized = normalize(config);
    let text = serde_json::to_string(&normalized)
        .map_err(|e| ApiError::internal(format!("序列化捕获配置失败：{e}")))?;
    state.write(|tx| {
        // R3：被同步的表禁用 INSERT OR REPLACE（会点亮 DELETE 触发器写假 delete）——
        // `app_settings` 不在同步范围内，但统一用 ON CONFLICT 的写法，省得将来把它加进同步时踩坑
        tx.execute(
            "INSERT INTO app_settings (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
            params![KEY_CAPTURE, text],
        )?;
        Ok(())
    })?;
    Ok(normalized)
}

fn normalize(config: CaptureConfig) -> CaptureConfig {
    let comfy_url = {
        let trimmed = config.comfy_url.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            default_comfy_url()
        } else {
            trimmed.to_string()
        }
    };
    CaptureConfig {
        enabled: config.enabled,
        comfy_url,
        output_dir: config
            .output_dir
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty()),
        // 夹到合法区间：轮询太快会把 ComfyUI 问崩，太慢用户以为坏了
        poll_seconds: config.poll_seconds.clamp(1, 600),
        auto_tag: config.auto_tag.trim().to_string(),
        max_per_poll: config.max_per_poll.clamp(1, 500),
        download_fallback: config.download_fallback,
    }
}

/// 工作流原文 `/api/prompts/{id}/workflow` 与 `/api/media/{id}/workflow` 的共用实现。
///
/// 三种结果，**每一种都要与 Kotlin 一致**：
///  - 目标不存在 → 404；
///  - 存在但 `workflow_json` 是 NULL → **204**（"这条老数据没有工作流"）；
///  - 有 → 200 + `application/json` 的**原文**（不重新序列化：原文可能有几百 KB，
///    而且前端要按原样塞进工作流弹窗）。
pub fn workflow_text(
    state: &AppState,
    table: &str,
    id: i64,
    not_found: &str,
) -> Result<Option<String>, ApiError> {
    // 表名与列名都是**编译期常量**（下面只有两个调用点），所以这里的 format! 不是注入面
    state.with(|conn| {
        let exists: Option<i64> = conn
            .query_row(&format!("SELECT id FROM {table} WHERE id = ?1"), params![id], |row| row.get(0))
            .optional()?;
        if exists.is_none() {
            return Ok(WorkflowLookup::Missing);
        }
        let text: Option<String> = conn
            .query_row(
                &format!("SELECT workflow_json FROM {table} WHERE id = ?1"),
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        Ok(match text {
            Some(t) if !t.trim().is_empty() => WorkflowLookup::Found(t),
            _ => WorkflowLookup::Empty,
        })
    })
    .and_then(|lookup| match lookup {
        WorkflowLookup::Missing => Err(ApiError::not_found(not_found.to_string())),
        WorkflowLookup::Empty => Ok(None),
        WorkflowLookup::Found(text) => Ok(Some(text)),
    })
}

enum WorkflowLookup {
    Missing,
    Empty,
    Found(String),
}

/// 路由用的小包装（顺带把"哪张表"钉死在调用点上）。
pub fn prompt_workflow(state: &AppState, id: i64) -> Result<Option<String>, ApiError> {
    workflow_text(state, "prompts", id, &format!("提示词 {id} 不存在"))
}

pub fn media_workflow(state: &AppState, id: i64) -> Result<Option<String>, ApiError> {
    workflow_text(state, "media_assets", id, &format!("产物 {id} 不存在"))
}

/// `GET /api/capture/config` 的处理体。
pub fn get_config(state: &AppState) -> Result<CaptureConfig, ApiError> {
    capture_config(state)
}

/// `PUT /api/capture/config` 的处理体。
pub fn put_config(state: &AppState, config: CaptureConfig) -> Result<CaptureConfig, ApiError> {
    save_capture_config(state, config)
}

/// 路由层要的 JSON 响应类型（工作流原文用 `Response` 直出，不走这里）。
pub type ConfigJson = axum::Json<CaptureConfig>;

/// 让 `Path` 与 `State` 的导入在只用到一处时不报 unused。
#[allow(dead_code)]
type UnusedImports = (Path<i64>, State<AppState>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_clamps_and_trims_like_kotlin() {
        let cfg = normalize(CaptureConfig {
            enabled: false,
            comfy_url: "  http://127.0.0.1:8188/  ".into(),
            output_dir: Some("   ".into()),
            poll_seconds: 99_999,
            auto_tag: "  ComfyUI  ".into(),
            max_per_poll: 0,
            download_fallback: false,
        });
        assert_eq!(cfg.comfy_url, "http://127.0.0.1:8188");
        assert_eq!(cfg.output_dir, None, "空白目录当没配");
        assert_eq!(cfg.poll_seconds, 600, "轮询间隔夹到上限（太快会把 ComfyUI 问崩）");
        assert_eq!(cfg.max_per_poll, 1, "每次最多处理条数夹到下限");
        assert_eq!(cfg.auto_tag, "ComfyUI");
        assert!(!cfg.enabled);
        assert!(!cfg.download_fallback);
    }

    #[test]
    fn empty_comfy_url_falls_back_to_the_default() {
        let cfg = normalize(CaptureConfig { comfy_url: "   ".into(), ..CaptureConfig::default() });
        assert_eq!(cfg.comfy_url, "http://127.0.0.1:8188");
    }

    #[test]
    fn config_round_trips_with_camel_case_keys() {
        let json = serde_json::to_string(&CaptureConfig::default()).unwrap();
        // 前端读的是驼峰键（Kotlin 的 @Serializable 也是驼峰默认名）
        for key in ["comfyUrl", "outputDir", "pollSeconds", "autoTag", "maxPerPoll", "downloadFallback"] {
            assert!(json.contains(key), "缺键 {key}：{json}");
        }
        let back: CaptureConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, CaptureConfig::default());
    }
}
