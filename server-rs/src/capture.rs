//! 捕获配置与工作流原文（P4 第 7 步）；查询类在 P4 第 41/42 步补齐。
//!
//! | 端点 | 状态 |
//! | --- | --- |
//! | `GET/PUT /api/capture/config` | ✅ 就是 `app_settings` 的一行 JSON（设置页读写它） |
//! | `GET /api/prompts/{id}/workflow` / `/media/{id}/workflow` | ✅ 读库里的 `workflow_json` 文本 |
//! | `GET /api/prompts/{id}/api-graph` | ✅ 从 `capture_runs.raw` 取 API 图（第 9 步） |
//! | `GET /api/capture/status` / `/jobs` | ✅ 第 42 步（真的去问 ComfyUI 的 `/queue`） |
//! | `/api/capture/poll` | ⛔ 未搬（要整套捕获入库：Claim → 下载产物 → 建提示词与产物行） |
//! | `/api/capture/{import,import-workflows}`、`/api/ingest/comfyui` | ⛔ 未搬（要转换栈 / ComfyUI 在场） |
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
use serde_json::{json, Map, Value};

/// 自动捕获配置（与 Kotlin `CaptureConfig` 逐字同形）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_comfy_url")]
    pub comfy_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
/// 所以这里也认 `COMFYHUB_COMFY_URL` / `COMFYHUB_COMFY_OUTPUT` ——
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
        output_dir: comfy_output_env(),
        ..CaptureConfig::default()
    };
    match raw {
        Some(text) if !text.trim().is_empty() => Ok(serde_json::from_str::<CaptureConfig>(&text)
            .map_err(|_| ())
            .unwrap_or(env_default)),
        _ => Ok(env_default),
    }
}

/// 产物目录的环境默认值。
///
/// ⚠ **两个名字都认**（2026-09-30 踩到，是那个 `400 Invalid image file` 的一半根因）：
/// 脚本、README 与 AGENTS 用的都是 **`COMFYHUB_COMFY_OUTPUT`**（`scripts\comfy-path.ps1`
/// 就是按它判的，Kotlin 版 `Config.kt` 也只认它），而 Rust 版原来只认
/// `COMFYHUB_COMFY_OUTPUT_DIR` —— **那个名字没有任何地方设置过**。
///
/// 后果不是"少一个旋钮"，而是：库里没有 outputDir（发布包第一次用就是这样）时，
/// `output_dir` 静默为空 → `image_edit::resolve_input_dir` 退到 `find_home` 探到的
/// **安装目录**那份 input，而实例挂的是 ComfyUI Desktop 配的**共享**那份 ——
/// 投放的图躺在安装目录里，实例读共享目录 → 提交 400，看着像"文件不存在"。
///
/// 两份实现（`scripts\comfy-path.ps1` 与这里）的名字必须一致：先认带 `_DIR` 的全名，
/// 再认脚本与老版本用的那个短名。
fn comfy_output_env() -> Option<String> {
    ["COMFYHUB_COMFY_OUTPUT_DIR", "COMFYHUB_COMFY_OUTPUT"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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

// ---------------------------------------------------------------------------
//  capture_runs 的查询（`comfy_get_run` 工具用，P4 第 41 步）
// ---------------------------------------------------------------------------

/// 一条捕获记录（Kotlin `CaptureRunInfo`，7 个键、可空的**省略键**）。
///
/// 三个"id"必须分清（Kotlin 那边为此专门加了一个查询入口）：
/// `run_key` 是 ComfyUI 的 prompt_id（UUID）**或** `file:<sha256>` / `import:<sha256>`；
/// `prompt_id` 是 `capture_runs` 表的数字主键。模型很容易拿数字那个来查，
/// 所以 [`find_run`] 两个都认。
#[derive(Debug, Clone)]
pub struct CaptureRunInfo {
    pub run_key: String,
    pub prompt_id: Option<i64>,
    pub status: String,
    pub media_count: i64,
    pub title: Option<String>,
    pub error: Option<String>,
    pub captured_at: Option<String>,
}

impl CaptureRunInfo {
    /// Kotlin 的 `AppJson`（`explicitNulls = false`）：可空键**整个不出现**。
    pub fn to_json(&self) -> serde_json::Value {
        let mut out = serde_json::Map::new();
        out.insert("runKey".into(), serde_json::json!(self.run_key));
        out.insert("status".into(), serde_json::json!(self.status));
        out.insert("mediaCount".into(), serde_json::json!(self.media_count));
        if let Some(id) = self.prompt_id {
            out.insert("promptId".into(), serde_json::json!(id));
        }
        if let Some(title) = &self.title {
            out.insert("title".into(), serde_json::json!(title));
        }
        if let Some(error) = &self.error {
            out.insert("error".into(), serde_json::json!(error));
        }
        if let Some(at) = &self.captured_at {
            out.insert("capturedAt".into(), serde_json::json!(at));
        }
        serde_json::Value::Object(out)
    }
}

/// 按 runKey 查一条捕获记录（Kotlin `CaptureRepo.findRun`）。
pub fn find_run(state: &AppState, run_key: &str) -> Result<Option<CaptureRunInfo>, ApiError> {
    let key = run_key.to_string();
    state.with(|conn| {
        Ok(conn
            .query_row(
                "SELECT run_key, prompt_id, status, media_count, title, error, created_at \
                 FROM capture_runs WHERE run_key = ?1",
                rusqlite::params![key],
                row_to_run_info,
            )
            .ok())
    })
}

/// 按**数字 prompt_id** 查一条捕获记录（Kotlin `CaptureRepo.findRunByPromptId`）。
///
/// `ORDER BY id DESC LIMIT 1`：同一个 ComfyUI prompt_id 理论上唯一，但老库里有重复行，
/// 取最新那条与 Kotlin 一致。
pub fn find_run_by_prompt_id(
    state: &AppState,
    prompt_id: i64,
) -> Result<Option<CaptureRunInfo>, ApiError> {
    state.with(|conn| {
        Ok(conn
            .query_row(
                "SELECT run_key, prompt_id, status, media_count, title, error, created_at \
                 FROM capture_runs WHERE prompt_id = ?1 ORDER BY id DESC LIMIT 1",
                rusqlite::params![prompt_id],
                row_to_run_info,
            )
            .ok())
    })
}

fn row_to_run_info(row: &rusqlite::Row<'_>) -> rusqlite::Result<CaptureRunInfo> {
    let created: Option<String> = row.get(6)?;
    Ok(CaptureRunInfo {
        run_key: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
        prompt_id: row.get(1)?,
        // 老库里 status 可能是 NULL：Kotlin 的 `strOr("status", "success")` 回落到 success
        status: row.get::<_, Option<String>>(2)?.unwrap_or_else(|| "success".to_string()),
        media_count: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
        title: row.get(4)?,
        error: row.get(5)?,
        // ⚠ 时间列走 `iso_time`（Kotlin 是"先解析再按 Java `Instant.toString()` 重排"），
        // 不是原样透传：`.000Z` 在那边发出来是**没有小数点**的（P4 第 21 步实测）
        captured_at: crate::iso_time::iso_time(created.as_deref()),
    })
}

/// 最近若干条捕获记录（Kotlin `CaptureRepo.recentRuns`，`ORDER BY id DESC`）。
pub fn recent_runs(state: &AppState, limit: i64) -> Result<Vec<CaptureRunInfo>, ApiError> {
    let limit = limit.clamp(1, 200);
    state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT run_key, prompt_id, status, media_count, title, error, created_at \
             FROM capture_runs ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], row_to_run_info)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    })
}

/// 捕获统计 `(运行数, 产物数)`（Kotlin `CaptureRepo.stats`）。
pub fn stats(state: &AppState) -> Result<(i64, i64), ApiError> {
    state.with(|conn| {
        let runs: i64 = conn.query_row("SELECT COUNT(*) FROM capture_runs", [], |row| row.get(0))?;
        let media: i64 = conn.query_row(
            "SELECT COALESCE(SUM(media_count), 0) FROM capture_runs",
            [],
            |row| row.get(0),
        )?;
        Ok((runs, media))
    })
}

// ---------------------------------------------------------------------------
//  GET /api/capture/status 与 /jobs（P4 第 42 步）
// ---------------------------------------------------------------------------

/// 一次队列探测的结果：`(ComfyUI 可达吗, 快照)`。
///
/// 不可达时快照是**零值**（不是错误）—— 与 Kotlin `status()` 一样：连不上就如实说
/// `comfyReachable=false`，而不是让整个状态查询 500。
pub async fn probe_queue(state: &AppState) -> (bool, crate::comfy_client::QueueSnapshot) {
    let empty = crate::comfy_client::QueueSnapshot { running: 0, pending: 0, running_label: None };
    let Ok(config) = capture_config(state) else { return (false, empty) };
    match crate::comfy_client::queue(&config.comfy_url).await {
        Ok(snapshot) => (true, snapshot),
        Err(_) => (false, empty),
    }
}

/// `GET /api/capture/status`（Kotlin `CaptureStatus`，11 个键，可空键省略）。
///
/// ⚠ `lastPollAt` / `lastError` 在 Kotlin 那边是 `@Volatile` 的内存字段，**只由后台轮询器写**。
/// P4 第 47 步起 Rust 侧也有那个轮询器了（[`crate::capture_poller::CapturePoller`]），
/// 所以这两个键**真的会跟着轮询走**：成功写时间并清空错误、失败写人话。
/// 轮询器还没跑过第一轮（或配置停用）时它们**如实不出现** —— 不编一个时间戳。
///
/// ⚠ `runningLabel` **不在**这里（它在 `/api/capture/jobs` 里）：Kotlin 的 `CaptureStatus`
/// 就 11 个键，多塞一个会让"两端的形状"从这一步开始漂。
pub async fn status_json(state: &AppState) -> Result<Value, ApiError> {
    let config = capture_config(state)?;
    let (runs, media) = stats(state).unwrap_or((0, 0));
    let (reachable, snapshot) = probe_queue(state).await;
    let recent = recent_runs(state, 20).unwrap_or_default();
    let (last_poll_at, last_error) = state.capture_poller.snapshot();

    let mut out = Map::new();
    out.insert("enabled".into(), json!(config.enabled));
    out.insert("comfyUrl".into(), json!(config.comfy_url));
    if let Some(dir) = &config.output_dir {
        out.insert("outputDir".into(), json!(dir));
    }
    out.insert("comfyReachable".into(), json!(reachable));
    out.insert("queueRunning".into(), json!(snapshot.running));
    out.insert("queuePending".into(), json!(snapshot.pending));
    if let Some(at) = last_poll_at {
        out.insert("lastPollAt".into(), json!(at));
    }
    if let Some(error) = last_error {
        out.insert("lastError".into(), json!(error));
    }
    out.insert("capturedRuns".into(), json!(runs));
    out.insert("capturedMedia".into(), json!(media));
    out.insert("recent".into(), Value::Array(recent.iter().map(|r| r.to_json()).collect()));
    // 正在跑的那一条（P4 第 72 步）——**没有正在跑的就整键不出现**
    // （与旁边 `lastPollAt` / `outputDir` 那些可空键同一条规则）。
    if let Some(current) = state.comfy_progress.current() {
        out.insert("current".into(), current.to_json());
    }
    Ok(Value::Object(out))
}

/// 提交记录 + 它们各自的**实时进度**（P4 第 72 步）。
///
/// 进度**嵌在每条 submission 里**（`progress` 子对象）而不是另起一个数组：
/// 界面右侧栏本来就是按 `promptId` 把两条对起来显示，分成两个数组反而要前端自己 join，
/// 而且两边一旦不同步就会出现"这条任务挂着别人的进度"。
/// 没有这个 `promptId` 的进度时**一个字节都不加**（不塞空对象）。
///
/// ⚠ 三处（`/status`、`/jobs`、`comfy_get_status`）读的都是这一个函数 ——
/// 各写一份的后果是"模型看到的进度与界面看到的不是一回事"。
pub fn submissions_json(state: &AppState, limit: usize) -> Vec<Value> {
    state
        .submissions
        .list(limit)
        .iter()
        .map(|item| state.comfy_progress.decorate(item))
        .collect()
}

/// `GET /api/capture/jobs`（Kotlin `CaptureJobSnapshot`，5 个键 + 第 72 步的 `current`）。
///
/// `submissions` 是**本进程提交过的任务**（P4 第 48 步起由 `comfy_submit` 维护，
/// 与 `comfy_get_status` / 界面右侧栏读的是同一份内存清单），
/// 第 72 步起每条带上 `progress`（实时百分比 + 当前步骤）。
pub async fn jobs_json(state: &AppState) -> Result<Value, ApiError> {
    let (reachable, snapshot) = probe_queue(state).await;
    let mut out = Map::new();
    out.insert("queueRunning".into(), json!(snapshot.running));
    out.insert("queuePending".into(), json!(snapshot.pending));
    out.insert("comfyReachable".into(), json!(reachable));
    if let Some(label) = snapshot.running_label {
        out.insert("runningLabel".into(), json!(label));
    }
    out.insert("submissions".into(), Value::Array(submissions_json(state, 10)));
    // 正在跑的那一条提到**顶层**：界面右侧栏靠它置顶/高亮，不必自己挑
    // （列表里也能找到，但"谁在跑"是服务端知道得更准的事实）
    if let Some(current) = state.comfy_progress.current() {
        out.insert("current".into(), current.to_json());
    }
    Ok(Value::Object(out))
}

// ---------------------------------------------------------------------------
//  API 格式节点图（P4 第 9 步）
// ---------------------------------------------------------------------------

/// **API 格式**节点图 `/api/prompts/{id}/api-graph`。
///
/// 这是"当时真正跑的东西"（提交给 ComfyUI `/prompt` 的那一份），
/// 与 `/workflow`（界面格式，能拖回 ComfyUI 复现）**不是一回事**。
///
/// 取数链条与 Kotlin `ComfyCapture.apiGraphOf` 逐字一致：
/// `prompts.source_ref`（= `run_key`）→ `capture_runs.raw` → `HistoryEntry.parse(entry).graph`。
///
/// # 三种结果，一种都不能少（R10 的前提）
///
/// | 情况 | 应答 |
/// | --- | --- |
/// | 提示词不存在 | **404** |
/// | 存在但没有 `source_ref` / 没有 `capture_runs` 行 / 解析不出节点图 | **204** |
/// | 有 | **200** + 节点图 JSON |
///
/// ⚠ 老数据没有 API 图时回 204 而不是"猜着转换"：界面/模型据此如实说"这条跑不了"
/// （AGENTS §10.1 的 `NO_API_GRAPH`）。**204 与 404 混在一起就等于把"缺数据"说成"没这条提示词"**。
pub fn prompt_api_graph(state: &AppState, id: i64) -> Result<Option<serde_json::Value>, ApiError> {
    let lookup = state.with(|conn| {
        let prompt: Option<Option<String>> = conn
            .query_row("SELECT source_ref FROM prompts WHERE id = ?1", params![id], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()?;
        let Some(source_ref) = prompt else {
            return Ok(GraphLookup::Missing);
        };
        // source_ref 是"这把工作流是从哪来的"：空/空白当没有（别拿空串去查出别的行）
        let Some(run_key) = source_ref.filter(|s| !s.trim().is_empty()) else {
            return Ok(GraphLookup::NoRun);
        };
        let raw: Option<String> = conn
            .query_row("SELECT raw FROM capture_runs WHERE run_key = ?1", params![run_key], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten();
        Ok(match raw.filter(|t| !t.trim().is_empty()) {
            Some(text) => GraphLookup::Raw(text),
            None => GraphLookup::NoRun,
        })
    })?;

    match lookup {
        GraphLookup::Missing => Err(ApiError::not_found(format!("提示词 {id} 不存在"))),
        GraphLookup::NoRun => Ok(None),
        GraphLookup::Raw(text) => {
            // 解析不了（脏数据 / 截断的 raw）也只是 204 —— 与 Kotlin 的 runCatching 同语义
            let entry: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            Ok(history::parse(&entry).graph)
        }
    }
}

enum GraphLookup {
    Missing,
    NoRun,
    Raw(String),
}

/// `HistoryEntry` 的逐行移植（Kotlin `ComfyCapture.kt` 的 `internal object HistoryEntry`）。
///
/// ComfyUI 的 `/history/{id}` 条目里，"节点图"藏在三层不同的形状里，历史上三种都出现过：
///
/// 1. `{prompt: {<节点id>: {class_type, inputs}}}` —— 最常见（`prompt` 就是图）；
/// 2. `{prompt: {prompt: <图>, extra_data: {...}}}` —— 具名形式；
/// 3. `{prompt: [<图>, <extra_data>]}` —— 数组形式。
///
/// 判据是**结构特征**而不是键名：图 = 每个值都是带 `class_type` 的对象；`extra_data` = 带
/// `extra_pnginfo` / `client_id` / `create_time` 之一。**不要改成按键名猜** ——
/// 猜错的后果是把 `extra_data` 当成节点图发给 ComfyUI（提交必炸，且报错信息毫无线索）。
pub mod history {
    use serde_json::{Map, Value};

    #[derive(Debug, Default, PartialEq)]
    pub struct Parsed {
        pub graph: Option<Value>,
        pub extra_data: Option<Value>,
        pub workflow: Option<Value>,
    }

    pub fn parse(entry: &Value) -> Parsed {
        let Some(field) = entry.get("prompt") else {
            return Parsed::default();
        };
        match field {
            Value::Object(obj) => {
                // `(field["prompt"] as? JsonObject) ?: field.takeIf { isNodeGraph(it) }`
                let graph = obj
                    .get("prompt")
                    .filter(|v| v.is_object())
                    .cloned()
                    .or_else(|| is_node_graph(obj).then(|| Value::Object(obj.clone())));
                let extra_data = obj
                    .get("extra_data")
                    .filter(|v| v.is_object())
                    .cloned()
                    .or_else(|| looks_like_extra_data(obj).then(|| Value::Object(obj.clone())));
                Parsed {
                    workflow: workflow_of(extra_data.as_ref()),
                    graph,
                    extra_data,
                }
            }
            Value::Array(items) => {
                let objects: Vec<&Map<String, Value>> =
                    items.iter().filter_map(|v| v.as_object()).collect();
                // Kotlin 用的是 `it !== graph`（引用比较），这里换成**下标比较**，
                // 避免同一个对象既是图又像 extra_data 时两处指向同一行
                let graph_idx = objects.iter().position(|o| is_node_graph(o));
                let graph = graph_idx.map(|i| Value::Object(objects[i].clone()));
                let extra_data = objects
                    .iter()
                    .enumerate()
                    .find(|(i, o)| Some(*i) != graph_idx && looks_like_extra_data(o))
                    .map(|(_, o)| Value::Object((*o).clone()));
                Parsed {
                    workflow: workflow_of(extra_data.as_ref()),
                    graph,
                    extra_data,
                }
            }
            _ => Parsed::default(),
        }
    }

    /// API 格式节点图：非空，且**每个值**都是带 `class_type` 的对象
    pub fn is_node_graph(obj: &Map<String, Value>) -> bool {
        !obj.is_empty()
            && obj
                .values()
                .all(|v| v.as_object().is_some_and(|o| o.contains_key("class_type")))
    }

    fn looks_like_extra_data(obj: &Map<String, Value>) -> bool {
        obj.contains_key("extra_pnginfo") || obj.contains_key("client_id") || obj.contains_key("create_time")
    }

    /// 界面格式工作流藏在 `extra_data.extra_pnginfo.workflow` 里
    pub fn workflow_of(extra_data: Option<&Value>) -> Option<Value> {
        extra_data?.get("extra_pnginfo")?.get("workflow").cloned()
    }
}

/// 让 `Path` 与 `State` 的导入在只用到一处时不报 unused。
#[allow(dead_code)]
type UnusedImports = (Path<i64>, State<AppState>);

#[cfg(test)]
mod tests;
