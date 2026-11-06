//! ComfyUI 生成任务的**实时进度 + 当前步骤**（WS 常驻 + 重连）。
//!
//! # 为什么只能走 WebSocket
//!
//! 2026-09-27 在本机 ComfyUI（`D:\Comfy-Desktop\ComfyUI-Installs\ComfyUI\ComfyUI`）上实测：
//!
//! | 途径 | 结果 |
//! | --- | --- |
//! | `GET /queue` | 只有 `{"queue_running":[],"queue_pending":[]}` —— **没有进度** |
//! | `GET /progress`、`GET /api/progress` | **404** |
//! | `GET /history` | 只有**跑完**的运行 |
//! | `ws://<base>/ws?clientId=<任意 id>` | ✅ 握手成功，进度帧全在这里 |
//!
//! 所以这个模块是"进度"的**唯一**来源，形态与 [`crate::comfy::capture_poller`] 一致：
//! 一个**永不退出**的后台任务（`main.rs` 随 `AppState` 起一次），断线指数退避重连。
//! **绝不允许"每次请求现连一次"** —— 那既收不到已经发出去的帧，也会把 ComfyUI 的
//! socket 表撑爆（ComfyUI 按 clientId 记 `self.sockets[sid]`）。
//!
//! # 帧字段名是**实测**的，不是照文档猜的
//!
//! 下面每个字段名都来自本机抓到的**真帧**（原文见各分支注释），或 ComfyUI 源码里
//! `send_sync(...)` 的那一行。改这里之前先看那张对照表：
//!
//! | 帧 `type` | 数据来源 | 字段 |
//! | --- | --- | --- |
//! | `status` | `server.py:1400`（`queue_updated`）/ 握手时 `server.py:287` | `data.status.exec_info.queue_remaining`、`data.sid` |
//! | `execution_start` | `execution.py:742` | `data.prompt_id`、`data.timestamp` |
//! | `execution_cached` | `execution.py:770` | `data.nodes[]`、`data.prompt_id` |
//! | `executing` | `execution.py:496` **与** `main.py:374` | `data.node`（**可为 null**）、`data.display_node`、`data.prompt_id` |
//! | `progress` | `main.py:435`（`hijack_progress` 的钩子） | `data.value`、`data.max`、`data.prompt_id`、`data.node` |
//! | `progress_state` | `comfy_execution/progress.py:184` | `data.prompt_id`、`data.nodes{<id>:{value,max,state,node_id,…}}` |
//! | `executed` | `execution.py:577` | `data.node`、`data.display_node`、`data.output`、`data.prompt_id` |
//! | `execution_success` | `execution.py:824` | `data.prompt_id`、`data.timestamp` |
//! | `execution_error` | `execution.py:712`（`add_message`） | `data.prompt_id`、`data.node_id`、`data.node_type`、`data.exception_message`、`data.exception_type`、`data.traceback[]`、`data.executed[]` |
//! | `execution_interrupted` | `execution.py:699`（`broadcast=True`） | `data.prompt_id`、`data.node_id`、`data.node_type`、`data.executed[]` |
//!
//! ⚠ 一处**没抓到活帧**的老式 `progress`：本机这个 ComfyUI 版本里，常见节点（采样器 /
//! VAE）走的是 `progress_state` 或 VAE 自己的原生 tiled 路径，`hijack_progress` 的
//! `comfy.utils.ProgressBar` 钩子那条路没走到。它**照样要解析**——别的 ComfyUI 版本
//! 与 `comfy.sample.sample` 的采样循环都会发它，而且它是最"标准"的百分比来源。
//! 单测里用手写帧覆盖（形状抄自 `main.py:432`），注释里已标明。
//!
//! # ⚠ `percent` 的纪律：**拿不到就如实说未知，绝不伪造**
//!
//! `percent` **只**来自 ComfyUI 自己发的 `value` / `max`：
//!  - 有 `progress` / `progress_state` 帧（且该节点带 `value`/`max`）→ `value / max * 100`；
//!  - 否则 `percent = None`（界面显示"进度未知"）。
//!
//! **绝不用"已执行节点数 / 总节点数"冒充采样进度**：那是"跑了几个节点"，不是"这张图
//! 画了几成"。KSampler 通常占掉九成时间，按节点数算会在 90% 卡住然后瞬间跳到 100%。
//!
//! 另一条相关的诚实：**终态不把 `percent` 抬到 100**。运行成功 = 这次运行结束了，
//! 但它最后一次报的采样进度就是最后一次报的那个数；界面按 `state` 显示"已完成"，
//! 而不是拿一个编出来的 100 去糊进度条。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::web::routes::AppState;

/// 启动后第一次连接前的等待（与 [`crate::comfy::capture_poller::STARTUP_DELAY_MS`] 同因：
/// 别在后端刚起来、ComfyUI 还没好的那几秒里疯狂重连）。
pub const STARTUP_DELAY_MS: u64 = 2_500;
/// 断线重连的退避下限 / 上限（指数退避，夹在这个区间里）。
pub const RECONNECT_MIN_MS: u64 = 1_000;
pub const RECONNECT_MAX_MS: u64 = 30_000;
/// 内存里最多记多少个 `prompt_id` 的进度（老的先丢）。
pub const MAX_TRACKED_PROMPTS: usize = 50;

// ---------------------------------------------------------------------------
//  帧的纯解析（**可测、不许 panic**）
// ---------------------------------------------------------------------------

/// 一帧被识别出来的进度事件。**未知帧一律不在这里**（[`parse_frame`] 回 `None`）。
#[derive(Debug, Clone, PartialEq)]
pub enum ProgressEvent {
    /// `status`：队列里还剩几个。
    Status { queue_remaining: Option<i64> },
    /// `execution_start`：这次运行开始了。
    ExecutionStart { prompt_id: String },
    /// `execution_cached`：这些节点的结果直接命中缓存（**不是**进度）。
    ExecutionCached { prompt_id: String },
    /// `executing`：`node` 是**正在执行**的那个节点；`node: null` 表示**没有节点在执行了**
    /// （`main.py:374` 在队列任务收尾时发的就是这一种）。
    Executing { prompt_id: String, node_id: Option<String> },
    /// `progress`：**最标准的采样进度**（`main.py:435`）。
    Progress { prompt_id: String, node_id: Option<String>, value: f64, max: f64 },
    /// `progress_state`：新版按节点的进度表（`comfy_execution/progress.py:184`）。
    ProgressState { prompt_id: String, node_id: Option<String>, value: f64, max: f64 },
    /// `executed`：某个节点跑完了（**不推进 percent** —— 见文件头那条纪律）。
    Executed { prompt_id: String, node_id: Option<String> },
    ExecutionSuccess { prompt_id: String },
    ExecutionError { prompt_id: String, node_id: Option<String>, message: Option<String> },
    ExecutionInterrupted { prompt_id: String },
}

fn str_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn num_field(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64).filter(|n| n.is_finite())
}

/// 把一条 WS 文本帧解析成 [`ProgressEvent`]。
///
/// 三条硬要求（都是"界面不许被脏数据打死"）：
///  1. **未知 `type` → `None`**（ComfyUI 与自定义节点会发 `logs` / `crystools.*` /
///     二进制预览等一堆我们不管的东西，冒出来一个就当错误会让整条连接断掉）；
///  2. **字段缺失 → `None` 或对应字段留空**，绝不 `unwrap`（`executing` 的 `node` 本来就
///     可以是 `null`，`progress` 的 `prompt_id` 理论上也可能是 `null`）；
///  3. 非 JSON（二进制帧被当文本喂进来）→ `None`。
pub fn parse_frame(text: &str) -> Option<ProgressEvent> {
    let root: Value = serde_json::from_str(text).ok()?;
    let kind = root.get("type")?.as_str()?;
    let data = root.get("data").unwrap_or(&Value::Null);
    match kind {
        // 握手时第一条与每次队列变化（server.py:1400 / server.py:287）
        "status" => Some(ProgressEvent::Status {
            queue_remaining: data
                .get("status")
                .and_then(|s| s.get("exec_info"))
                .and_then(|e| e.get("queue_remaining"))
                .and_then(Value::as_i64),
        }),
        "execution_start" => {
            Some(ProgressEvent::ExecutionStart { prompt_id: str_field(data, "prompt_id")? })
        }
        "execution_cached" => {
            Some(ProgressEvent::ExecutionCached { prompt_id: str_field(data, "prompt_id")? })
        }
        // ⚠ `node` 允许是 null（"没有节点在执行"）；`prompt_id` 才是必备的那个
        "executing" => Some(ProgressEvent::Executing {
            prompt_id: str_field(data, "prompt_id")?,
            // `display_node` 是子图/分组的显示节点；UI 认它（execution.py:496 两个都给）
            node_id: str_field(data, "node").or_else(|| str_field(data, "display_node")),
        }),
        "progress" => {
            let value = num_field(data, "value")?;
            let max = num_field(data, "max")?;
            Some(ProgressEvent::Progress {
                prompt_id: str_field(data, "prompt_id")?,
                node_id: str_field(data, "node").or_else(|| str_field(data, "node_id")),
                value,
                max,
            })
        }
        // progress_state 一次给**所有非 pending 节点**；取"正在跑的那个"那个的 value/max
        "progress_state" => {
            let prompt_id = str_field(data, "prompt_id")?;
            let nodes = data.get("nodes")?.as_object()?;
            let mut running: Option<(String, f64, f64)> = None;
            for (node_id, node) in nodes {
                if node.get("state").and_then(Value::as_str) != Some("running") {
                    continue;
                }
                let (Some(value), Some(max)) = (num_field(node, "value"), num_field(node, "max"))
                else {
                    continue;
                };
                // 插入序就是执行序：取**最后一个** running（最新开始的那个）
                running = Some((node_id.clone(), value, max));
            }
            let (node_id, value, max) = running?;
            Some(ProgressEvent::ProgressState { prompt_id, node_id: Some(node_id), value, max })
        }
        "executed" => Some(ProgressEvent::Executed {
            prompt_id: str_field(data, "prompt_id")?,
            node_id: str_field(data, "node").or_else(|| str_field(data, "display_node")),
        }),
        "execution_success" => {
            Some(ProgressEvent::ExecutionSuccess { prompt_id: str_field(data, "prompt_id")? })
        }
        "execution_error" => Some(ProgressEvent::ExecutionError {
            prompt_id: str_field(data, "prompt_id")?,
            node_id: str_field(data, "node_id").or_else(|| str_field(data, "node")),
            message: str_field(data, "exception_message"),
        }),
        "execution_interrupted" => {
            Some(ProgressEvent::ExecutionInterrupted { prompt_id: str_field(data, "prompt_id")? })
        }
        // `logs` / 二进制预览 / 自定义节点自己的事件 → **不是错误，只是不关我们的事**
        _ => None,
    }
}

// ---------------------------------------------------------------------------
//  进度快照
// ---------------------------------------------------------------------------

/// 一条 `prompt_id` 的进度快照（`state` 是 `queued` / `running` / `success` /
/// `error` / `cancelled` 之一，与 `workflow_submit::Submission::status` 同一套词）。
#[derive(Debug, Clone, PartialEq)]
pub struct PromptProgress {
    pub prompt_id: String,
    pub state: String,
    /// ComfyUI 报的 `value` / `max` 原样（拿不到就是 `None`）。
    pub value: Option<i64>,
    pub max: Option<i64>,
    /// `value / max * 100` 的整数；**拿不到 `value`/`max` 时是 `None`**（= 进度未知）。
    pub percent: Option<i64>,
    pub node_id: Option<String>,
    /// 节点的**可读名**：来自提交时登记的 API 图（`_meta.title` → `class_type`）。
    /// **没有映射时就是 `None`** —— 界面只显示 `node_id`，不许编一个名字。
    pub node_name: Option<String>,
    /// 出错时的人话（`exception_message`）。
    pub error: Option<String>,
    pub updated_at: String,
}

impl PromptProgress {
    /// JSON 形状：`state` / `updatedAt` 恒在，其余可空键**整个不出现**
    /// （与 `Submission::to_json`、`CaptureRunInfo::to_json` 同一套规则）。
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("promptId".into(), json!(self.prompt_id));
        out.insert("state".into(), json!(self.state));
        if let Some(value) = self.value {
            out.insert("value".into(), json!(value));
        }
        if let Some(max) = self.max {
            out.insert("max".into(), json!(max));
        }
        if let Some(percent) = self.percent {
            out.insert("percent".into(), json!(percent));
        }
        if let Some(node_id) = self.node_id.as_ref() {
            out.insert("nodeId".into(), json!(node_id));
        }
        if let Some(node_name) = self.node_name.as_ref() {
            out.insert("nodeName".into(), json!(node_name));
        }
        if let Some(error) = self.error.as_ref() {
            out.insert("error".into(), json!(error));
        }
        out.insert("updatedAt".into(), json!(self.updated_at));
        Value::Object(out)
    }

    /// 一句话人话（`comfy_get_status` 的正文里给模型看）。
    pub fn human(&self) -> String {
        let step = match (self.node_name.as_ref(), self.node_id.as_ref()) {
            (Some(name), Some(id)) => Some(format!("正在执行 {name}（节点 {id}）")),
            // 没有映射时**只给节点 id**，不编名字（文件头那条纪律）
            (None, Some(id)) => Some(format!("正在执行节点 {id}")),
            _ => None,
        };
        match (&self.percent, step) {
            (Some(percent), Some(step)) => format!("{percent}% · {step}"),
            (Some(percent), None) => format!("{percent}%"),
            (None, Some(step)) => format!("进度未知 · {step}"),
            (None, None) => match self.state.as_str() {
                "queued" => "排队中".to_string(),
                "success" => "已完成".to_string(),
                "error" => "已报错".to_string(),
                "cancelled" => "已取消".to_string(),
                _ => "进度未知".to_string(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
//  常驻状态
// ---------------------------------------------------------------------------

/// 内存里的进度表 + 节点名映射（都**只在本进程内存**，不落库）。
struct Inner {
    /// `prompt_id` → 进度；`HashMap` 的插入序在 `preserve_order` 下会保留，
    /// 但这里取"最新的"是靠 `updated_at`，不靠顺序。
    prompts: HashMap<String, PromptProgress>,
    /// `prompt_id` → （节点 id → 可读名）。提交时登记，`executing` / `progress` 帧用它翻成人话。
    node_names: HashMap<String, HashMap<String, String>>,
    /// `percent` 的**高水位**：同一个 `prompt_id` 的百分比**只许升不许降**
    /// （ComfyUI 偶尔会补发一条更早的帧，界面进度条倒着走会被当成"卡了又重来"）。
    percent_high_water: HashMap<String, i64>,
    /// 正在跑的那一个（终态帧会清掉它）。
    current: Option<String>,
    /// `status` 帧里的 `queue_remaining`。
    queue_remaining: Option<i64>,
}

/// ComfyUI 进度跟踪器（`AppState` 里的一个 `Arc`）。
pub struct ComfyProgress {
    /// 我们连 WS 用的 `clientId`，**也是 `/prompt` 提交时带的 `client_id`**：
    /// ComfyUI 的 `execution_*` / `progress` 只发给 `extra_data.client_id` 对应的那个 socket
    /// （`execution.py:494` `if server.client_id is not None`），
    /// 两者不一致就会"WS 连上了但一帧都收不到"。
    client_id: String,
    inner: Mutex<Inner>,
    connected: AtomicBool,
    /// 最后一次连接失败的人话（成功即清空）。
    last_error: Mutex<Option<String>>,
}

impl Default for ComfyProgress {
    fn default() -> Self {
        Self {
            client_id: format!("comfyhub-{}", simple_id()),
            inner: Mutex::new(Inner {
                prompts: HashMap::new(),
                node_names: HashMap::new(),
                percent_high_water: HashMap::new(),
                current: None,
                queue_remaining: None,
            }),
            connected: AtomicBool::new(false),
            last_error: Mutex::new(None),
        }
    }
}

impl ComfyProgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// 提交 `/prompt` 时要带的 `client_id`（与 WS 那个是**同一个**）。
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn queue_remaining(&self) -> Option<i64> {
        self.lock().queue_remaining
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 提交成功后登记 API 图的 `节点 id → 可读名` 映射。
    ///
    /// 名字取 `_meta.title`（ComfyUI 界面上用户自己起的标题），没有才是 `class_type`。
    /// **两者都没有就不登记** —— 于是 `executing` 帧只会给出节点 id，
    /// 界面显示"正在执行节点 33"而不是一个编出来的名字。
    pub fn register_graph(&self, prompt_id: &str, graph: &Value) {
        let Some(nodes) = graph.as_object() else { return };
        let mut names = HashMap::new();
        for (node_id, node) in nodes {
            let title = node
                .get("_meta")
                .and_then(|meta| meta.get("title"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let name = title
                .map(str::to_string)
                .or_else(|| str_field(node, "class_type"));
            if let Some(name) = name {
                names.insert(node_id.clone(), name);
            }
        }
        let mut inner = self.lock();
        inner.node_names.insert(prompt_id.to_string(), names);
        // 只留最近 MAX_TRACKED_PROMPTS 个映射（映射是"提交时登记、运行期查"，
        // 运行结束之后就没用了；不清理会随着 AI 一直提交慢慢涨）
        while inner.node_names.len() > MAX_TRACKED_PROMPTS {
            let oldest = inner
                .node_names
                .keys()
                .find(|k| {
                    inner
                        .prompts
                        .get(*k)
                        .map(|p| !matches!(p.state.as_str(), "running" | "queued"))
                        .unwrap_or(true)
                })
                .cloned();
            match oldest {
                Some(key) => {
                    inner.node_names.remove(&key);
                }
                None => break,
            }
        }
    }

    /// 把一帧的文本喂进来（`parse_frame` 回 `None` 就什么都不做）。
    pub fn frame(&self, text: &str) {
        if let Some(event) = parse_frame(text) {
            self.apply(&event);
        }
    }

    /// 落实一个事件（**纯状态更新**，单测直接喂事件，不需要 WS）。
    pub fn apply(&self, event: &ProgressEvent) {
        let now = crate::infra::iso_time::from_system_time(std::time::SystemTime::now());
        let mut inner = self.lock();
        match event {
            ProgressEvent::Status { queue_remaining } => {
                inner.queue_remaining = *queue_remaining;
            }
            ProgressEvent::ExecutionStart { prompt_id } => {
                let entry = entry_of(&mut inner, prompt_id, &now);
                if !is_terminal(&entry.state) {
                    entry.state = "running".to_string();
                }
                entry.updated_at = now;
                inner.current = Some(prompt_id.clone());
            }
            ProgressEvent::Executing { prompt_id, node_id } => {
                let name = node_id
                    .as_ref()
                    .and_then(|id| {
                        inner.node_names.get(prompt_id).and_then(|map| map.get(id)).cloned()
                    });
                let entry = entry_of(&mut inner, prompt_id, &now);
                if !is_terminal(&entry.state) {
                    entry.state = "running".to_string();
                }
                entry.updated_at = now;
                // `node: null` = "现在没有节点在执行了" → **只清当前节点**，
                // 不动 state（真正的终态由 execution_success / execution_error 说）。
                entry.node_id = node_id.clone();
                entry.node_name = name;
                if node_id.is_some() {
                    inner.current = Some(prompt_id.clone());
                }
            }
            ProgressEvent::Progress { prompt_id, node_id, value, max }
            | ProgressEvent::ProgressState { prompt_id, node_id, value, max } => {
                let name = node_id
                    .as_ref()
                    .and_then(|id| {
                        inner.node_names.get(prompt_id).and_then(|map| map.get(id)).cloned()
                    });
                let percent = percent_of(*value, *max);
                // `percent` 的**高水位**先算（与下面 `entry` 的可变借用分开，否则借用冲突）：
                // 单调不倒退 —— ComfyUI 偶尔会补发一条更早的帧，界面进度条倒着走会被当成"卡了又重来"
                let effective = percent.map(|p| {
                    let high = inner.percent_high_water.entry(prompt_id.clone()).or_insert(p);
                    if p > *high {
                        *high = p;
                    }
                    *high
                });
                let entry = entry_of(&mut inner, prompt_id, &now);
                if !is_terminal(&entry.state) {
                    entry.state = "running".to_string();
                }
                entry.value = Some(*value as i64);
                entry.max = Some(*max as i64);
                if let Some(effective) = effective {
                    entry.percent = Some(effective);
                }
                if let Some(node_id) = node_id {
                    entry.node_id = Some(node_id.clone());
                    entry.node_name = name;
                }
                entry.updated_at = now;
                inner.current = Some(prompt_id.clone());
            }
            ProgressEvent::ExecutionCached { prompt_id } | ProgressEvent::Executed { prompt_id, .. } => {
                let entry = entry_of(&mut inner, prompt_id, &now);
                if !is_terminal(&entry.state) {
                    entry.state = "running".to_string();
                }
                entry.updated_at = now;
            }
            ProgressEvent::ExecutionSuccess { prompt_id } => {
                let entry = entry_of(&mut inner, prompt_id, &now);
                entry.state = "success".to_string();
                // ⚠ **不**把 percent 抬到 100（文件头那条纪律）
                entry.node_id = None;
                entry.node_name = None;
                entry.updated_at = now;
                clear_current(&mut inner, prompt_id);
            }
            ProgressEvent::ExecutionError { prompt_id, node_id, message } => {
                let name = node_id
                    .as_ref()
                    .and_then(|id| {
                        inner.node_names.get(prompt_id).and_then(|map| map.get(id)).cloned()
                    });
                let entry = entry_of(&mut inner, prompt_id, &now);
                entry.state = "error".to_string();
                if node_id.is_some() {
                    entry.node_id = node_id.clone();
                    entry.node_name = name;
                }
                entry.error = message.clone();
                entry.updated_at = now;
                clear_current(&mut inner, prompt_id);
            }
            ProgressEvent::ExecutionInterrupted { prompt_id } => {
                let entry = entry_of(&mut inner, prompt_id, &now);
                entry.state = "cancelled".to_string();
                entry.node_id = None;
                entry.node_name = None;
                entry.updated_at = now;
                clear_current(&mut inner, prompt_id);
            }
        }
        trim(&mut inner);
    }

    /// 某个 `prompt_id` 的进度。
    pub fn snapshot(&self, prompt_id: &str) -> Option<PromptProgress> {
        self.lock().prompts.get(prompt_id.trim()).cloned()
    }

    /// **正在跑的那一个**（终态帧会把它清掉；没有就是 `None`）。
    pub fn current(&self) -> Option<PromptProgress> {
        let inner = self.lock();
        let prompt_id = inner.current.clone()?;
        inner.prompts.get(&prompt_id).cloned()
    }

    /// 给一条提交记录补上进度（没有这个 `promptId` 的进度就**原样返回**）。
    pub fn decorate(&self, submission: &Value) -> Value {
        let Some(prompt_id) = submission.get("promptId").and_then(Value::as_str) else {
            return submission.clone();
        };
        let Some(progress) = self.snapshot(prompt_id) else {
            return submission.clone();
        };
        let mut out = submission.clone();
        if let Some(obj) = out.as_object_mut() {
            obj.insert("progress".into(), progress.to_json());
        }
        out
    }

    /// 最后一次连接失败的人话（测试与排错用）。
    pub fn note_error(&self, message: impl Into<String>) {
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(message.into());
        self.connected.store(false, Ordering::SeqCst);
    }
}

fn entry_of<'a>(inner: &'a mut Inner, prompt_id: &str, now: &str) -> &'a mut PromptProgress {
    inner
        .prompts
        .entry(prompt_id.to_string())
        .or_insert_with(|| PromptProgress {
            prompt_id: prompt_id.to_string(),
            // 第一次见到它就说明 ComfyUI 至少接下了它（`execution_start` / `executing` 之前
            // 没别的可能）—— 但**不猜**：默认就是 `queued`，由具体事件抬到 `running`。
            state: "queued".to_string(),
            value: None,
            max: None,
            percent: None,
            node_id: None,
            node_name: None,
            error: None,
            updated_at: now.to_string(),
        })
}

fn is_terminal(state: &str) -> bool {
    matches!(state, "success" | "error" | "cancelled")
}

fn clear_current(inner: &mut Inner, prompt_id: &str) {
    if inner.current.as_deref() == Some(prompt_id) {
        inner.current = None;
    }
}

/// `value / max * 100` 的整数；`max <= 0` 或非有限值时 **`None`**（进度未知）。
fn percent_of(value: f64, max: f64) -> Option<i64> {
    // 先判 max 有限，再判正：NaN 走 `is_finite` 这一条（用 `!(max > 0.0)` 读着别扭）
    if !max.is_finite() || max <= 0.0 || !value.is_finite() {
        return None;
    }
    let percent = (value / max * 100.0).round();
    Some(percent.clamp(0.0, 100.0) as i64)
}

/// 只留最近 [`MAX_TRACKED_PROMPTS`] 条（按 `updated_at` 丢最老的）。
fn trim(inner: &mut Inner) {
    if inner.prompts.len() <= MAX_TRACKED_PROMPTS {
        return;
    }
    let mut keys: Vec<(String, String)> = inner
        .prompts
        .iter()
        .map(|(k, v)| (k.clone(), v.updated_at.clone()))
        .collect();
    keys.sort_by(|a, b| a.1.cmp(&b.1));
    let drop_count = inner.prompts.len() - MAX_TRACKED_PROMPTS;
    for (key, _) in keys.into_iter().take(drop_count) {
        inner.prompts.remove(&key);
        inner.node_names.remove(&key);
        inner.percent_high_water.remove(&key);
    }
}

// ---------------------------------------------------------------------------
//  常驻 WS 任务
// ---------------------------------------------------------------------------

/// 把 HTTP 基地址翻成 WS 地址（`http://` → `ws://`、`https://` → `wss://`）。
///
/// ⚠ 我们**只连明文 `ws://`**（`tokio-tungstenite` 连 TLS feature 都没开）：
/// ComfyUI 就在本机/内网，为它拖进 native-tls / openssl 会让发布包不再便携。
/// 用户把 `comfyUrl` 配成 `https://` 时**如实连不上并记错误**，而不是假装支持——
/// 那正是"需要一个能用的错误信息"的场合。
pub fn ws_url(comfy_url: &str, client_id: &str) -> Option<String> {
    let base = comfy_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    let base = if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if base.starts_with("ws://") || base.starts_with("wss://") {
        base.to_string()
    } else {
        format!("ws://{base}")
    };
    Some(format!("{base}/ws?clientId={client_id}"))
}

/// 起后台任务（**只在 `main.rs` 调一次**）。
///
/// **永不退出**：ComfyUI 起停、`comfyUrl` 改了，都由下一轮自己适应 ——
/// 与 `capture_poller` 那个循环同一条纪律（用户改完设置不必重启后端）。
pub fn spawn(tracker: &Arc<ComfyProgress>, state: AppState) {
    let tracker = tracker.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(STARTUP_DELAY_MS)).await;
        let mut backoff = RECONNECT_MIN_MS;
        loop {
            let url = crate::comfy::capture::capture_config(&state)
                .ok()
                .and_then(|config| ws_url(&config.comfy_url, tracker.client_id()));
            let Some(url) = url else {
                // 没配地址（或读配置失败）：空转，别忙等
                tracker.note_error("没有配置 ComfyUI 地址，进度 WebSocket 未连接");
                tokio::time::sleep(Duration::from_millis(RECONNECT_MAX_MS)).await;
                continue;
            };
            match pump(&tracker, &url).await {
                Ok(()) => {
                    // 连上了又断了（ComfyUI 重启 / 网络抖动）：**立刻**重连一轮
                    tracker.connected.store(false, Ordering::SeqCst);
                    backoff = RECONNECT_MIN_MS;
                }
                Err(message) => {
                    tracker.connected.store(false, Ordering::SeqCst);
                    tracker.note_error(message);
                    backoff = (backoff * 2).min(RECONNECT_MAX_MS);
                }
            }
            tokio::time::sleep(Duration::from_millis(backoff)).await;
        }
    });
}

/// 连一次并一直读到断（返回值：正常断线 = `Ok`，连不上/读失败 = `Err(人话)`）。
async fn pump(tracker: &Arc<ComfyProgress>, url: &str) -> Result<(), String> {
    use futures_util::StreamExt;

    let (mut socket, _response) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| format!("连不上 ComfyUI 的进度 WebSocket（{url}）：{e}"))?;
    tracker.connected.store(true, Ordering::SeqCst);
    *tracker.last_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
    eprintln!("[progress] 已连上 {url}");

    while let Some(message) = socket.next().await {
        let message = message.map_err(|e| format!("进度 WebSocket 读失败：{e}"))?;
        if message.is_close() {
            break;
        }
        // `to_text()` 对文本帧直接给 &str，对二进制帧尝试 UTF-8（预览图那种会失败 → 忽略）。
        // 一帧坏数据**不许**把整条连接弄断：解析是纯函数，未知帧只是 `None`。
        if let Ok(text) = message.to_text() {
            tracker.frame(text);
        }
    }
    tracker.connected.store(false, Ordering::SeqCst);
    Ok(())
}

/// 一个够用的唯一 id（不引 `uuid` crate —— 与 `workflow_submit::uuid_like` 同一个理由）。
fn simple_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mixed = nanos ^ (pid << 64) ^ (nanos.rotate_left(23));
    format!("{mixed:032x}")
}

// ---------------------------------------------------------------------------
//  测试（录下来的**真帧** + 手写的边界帧）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
