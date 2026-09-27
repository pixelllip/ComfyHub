//! `comfy_submit`（P4 第 48 步）—— **R10 在 Rust 侧的落点**。
//!
//! # 它做什么
//!
//! 把一个**工作流**真的送进 ComfyUI 跑一次，跑完把产物**入库**（建提示词 + 进画廊），
//! 并把 `mediaIds` 如实回给模型。逐条对照 Kotlin 的三处实现：
//! `ComfySubmitter`（提交 + 等 + 状态）、`ComfySubmitter.WorkflowEdit`（参数覆盖 / 补线）、
//! `AiWorkflowSearch.submit`（工具那一层的编排与返回的 JSON 形状）。
//!
//! # 四条纪律（每条都来自实测）
//!
//!  1. **只认 API 格式节点图**（R10）：图从 `prompts.source_ref` → `capture_runs.raw` 那条链取
//!     （[`crate::capture::prompt_api_graph`]）。界面格式（`nodes`/`links`）的 `widgets_values`
//!     只有位置没有参数名，猜着转**一定是错的**。取不到就报 `NO_API_GRAPH`，绝不猜着造一张。
//!  2. **提交后必须自带"等 + 立即入库"**：ComfyUI 的 `/history` 是**进程内的**
//!     （2026-09-25 实测：重启实例之后上一次运行就没了）。所以"提交完靠后台轮询补收"
//!     只在同一个实例生命周期内成立 —— 主路是这里自己盯着跑完并 [`capture_run`]，
//!     轮询（[`crate::capture_poller`]）只是兜底。
//!  3. **超时不等于失败**：等满 `waitSeconds` 还没完就如实回 `timeout` + "跑完会自动入库"，
//!     既不假装完成也不假装失败。等待上限 900 秒（`MAX_WAIT_SECONDS`）。
//!  4. **不做无限等待、不接受任意 URL**：地址只来自应用配置（`capture.config.comfyUrl`）。
//!
//! # 提交前的三道预检（§10.1 的 O1/O3 那一条线）
//!
//! 放在 `overrides` / `connections` **都应用完之后**（Kotlin 的原话：模型常常就是在这一次覆盖里
//! 把模型名换成能用的那个），检查的是**真正要提交的那张图**：
//!
//!  | 闸门 | 判据 | 拒了之后让模型干什么 |
//!  | --- | --- | --- |
//!  | 审核里"必跑不了"的三项 | [`crate::workflow_audit::audit`] 的 [`Report::blocking_refusal`]：纯前端节点 / 已知版本不兼容节点 / 本机没有的节点类 | 照审核给的那句话做（更新节点包 / 让 `comfy_load_workflow` 照前端行为改写 / 启停实例） |
//!  | 节点类在不在实例里 | `class_type` 不在 `/object_info` 的键里（**纯前端那一类除外**，见下） | 让用户装/启用那几个自定义节点包，或换实例；**不要**改用户的工作流 |
//!  | 模型文件实例认不认 | 节点类名含 `Loader` 的、输入值是字符串、而 `/object_info` 给该输入列的候选里有它没有 | 让用户**重启 ComfyUI**（ComfyHub 起实例时会挂上 Desktop 的模型根）；**别去下载** |
//!
//! 后两道是**实例侧**的（`/object_info` 直接说了算）；第一道是第 63 步那份
//! [`crate::workflow_audit`] 的第 70 步接线，位置与 Kotlin `Application.kt` 的 `preflight`
//! lambda（`WorkflowAudit.audit(graph, info, inv).blockingRefusal() ?: ModelPreflight.check(graph, inv)`）
//! 一致。**只有前两道会拒**；非阻断的那些（参数档位 / 显存 / 提示词风格 / 输出前缀 /
//! 加载器目录）走 `preflightWarnings` 如实带给模型与用户。
//!
//! ⚠ **拿不到 `/object_info` 就一个都不拦**（Kotlin 同）：误拦一个本来能跑的工作流，
//! 比白跑一次更糟。⚠ 唯一不看 `/object_info` 的是审核里那两条**静态知识表**
//!（纯前端节点 / 已知版本不兼容节点）—— 它们说的是"服务端根本没有这个类"，
//! ComfyUI 没跑时也成立，所以照样拦（Kotlin 的 `objectInfo` 为 null 时同样会走到那两条）。
//!
//! ## 与 Kotlin 的三处差别（如实记在这里，不假装逐字一致）
//!
//!  1. **非阻断的审核结论会回给模型**（`preflightWarnings`）：Kotlin 的 `preflight` 只回
//!     `String?`（拒 / 不拒），`audit` 的 report 其余部分**就地丢掉** —— 于是模型看不到
//!     "参数档位不对 / 显存吃紧"，除非它自己再调一次 `comfy_audit_workflow`。
//!     这里用 [`Report::render_non_blocking`] 把它们接到已有的警告通道上。
//!  2. **模型文件那一档只警告、不拦**（Kotlin 的 `ModelPreflight` 是**拒**）：Rust 这一版
//!     还没有"盘上有但实例看不到"那种盘扫措辞，只按 `/object_info` 的候选判会误拦 ——
//!     `scripts\e2e-submit-test.ps1` 第 4/5 幕就是活证（见 [`preflight_refusal`] 的说明）。
//!  3. **"实例里没有这个节点类"这一道排在审核之前**：Kotlin 的 `ModelPreflight` **不看节点类**
//!     （它只管模型文件），所以这个重叠只存在于 Rust 侧，没有 Kotlin 的先后可照搬。
//!     这一道先说话是因为它的措辞正是本仓库的纪律（"让用户启停/换实例，**不要**改用户的工作流"）；
//!     而**纯前端**那一类被它**故意跳过**（对那种类说"装节点包"是错的方向），交给审核那一档说。

use std::sync::Mutex;

use serde_json::{json, Map, Value};

use crate::capture::CaptureConfig;
use crate::routes::AppState;
use crate::tools::{ToolFailure, ToolOutcome};
use crate::workflow_audit;

/// 等一次运行跑完的默认时长（Kotlin `DEFAULT_WAIT_SECONDS`）。
pub const DEFAULT_WAIT_SECONDS: i64 = 240;
/// 等待上限（Kotlin `MAX_WAIT_SECONDS`）。
pub const MAX_WAIT_SECONDS: i64 = 900;
/// 内存里最多保留多少条提交记录（Kotlin `MAX_TRACKED`）。
pub const MAX_TRACKED: usize = 30;
/// 轮询间隔（Kotlin `delay(1500)`）。
const POLL_INTERVAL_MS: u64 = 1_500;

// ---------------------------------------------------------------------------
//  提交记录（内存快照，给界面右侧栏与 `comfy_get_status` 看）
// ---------------------------------------------------------------------------

/// 最近提交的运行（`prompt_id` → 进度）。
///
/// Kotlin 侧它是 `ConcurrentHashMap` + 按 `submittedAt` 倒序取最近 N 条；
/// 这里只存**已经序列化好的 JSON**（形状与 `ComfySubmission` 逐字一致），
/// 于是 `/api/capture/jobs` 的 `submissions[]` 直接就是模型看到的那一份，不会两处漂移。
#[derive(Default)]
pub struct Submissions {
    items: Mutex<Vec<Value>>,
}

impl Submissions {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一条（同一个 `promptId` 就地更新 —— 一次运行只会有一条记录）。
    pub fn record(&self, submission: Value) {
        let prompt_id = submission.get("promptId").and_then(Value::as_str).unwrap_or_default().to_string();
        let mut guard = self.items.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = guard
            .iter_mut()
            .find(|item| item.get("promptId").and_then(Value::as_str) == Some(prompt_id.as_str()))
        {
            *slot = submission;
        } else {
            guard.push(submission);
        }
        // 只留最近 30 条：按 submittedAt 升序排掉最老的
        if guard.len() > MAX_TRACKED {
            guard.sort_by_key(|item| {
                item.get("submittedAt").and_then(Value::as_str).unwrap_or_default().to_string()
            });
            let drop_count = guard.len() - MAX_TRACKED;
            guard.drain(0..drop_count);
        }
        // 新的在前（Kotlin `sortedByDescending { it.submittedAt }`）
        guard.sort_by(|a, b| {
            let left = b.get("submittedAt").and_then(Value::as_str).unwrap_or_default();
            let right = a.get("submittedAt").and_then(Value::as_str).unwrap_or_default();
            left.cmp(right)
        });
    }

    /// 最近的提交记录，新的在前。
    pub fn list(&self, limit: usize) -> Vec<Value> {
        let guard = self.items.lock().unwrap_or_else(|e| e.into_inner());
        guard.iter().take(limit.clamp(1, MAX_TRACKED)).cloned().collect()
    }

    pub fn find(&self, prompt_id: &str) -> Option<Value> {
        let key = prompt_id.trim();
        self.items
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|item| item.get("promptId").and_then(Value::as_str) == Some(key))
            .cloned()
    }
}

/// 一次提交的进度快照（`ComfySubmission`）。
///
/// ⚠ Kotlin 的 `AppJson` 是 `encodeDefaults = true` + `explicitNulls = false`：
/// **有默认值的键一律出现**（`elapsedMs: 0`、`mediaIds: []`），
/// 只有可空键（`finishedAt` / `capturedPromptId` / `error` / `message`）为 null 时**整键省略**。
#[derive(Debug, Clone)]
pub struct Submission {
    pub prompt_id: String,
    pub submitted_by: String,
    pub title: Option<String>,
    pub status: String,
    pub submitted_at: String,
    pub finished_at: Option<String>,
    pub elapsed_ms: i64,
    pub media_ids: Vec<i64>,
    pub captured_prompt_id: Option<i64>,
    pub error: Option<String>,
    pub message: Option<String>,
}

impl Submission {
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("promptId".into(), json!(self.prompt_id));
        out.insert("submittedBy".into(), json!(self.submitted_by));
        if let Some(title) = self.title.as_ref() {
            out.insert("title".into(), json!(title));
        }
        out.insert("status".into(), json!(self.status));
        out.insert("submittedAt".into(), json!(self.submitted_at));
        if let Some(finished) = self.finished_at.as_ref() {
            out.insert("finishedAt".into(), json!(finished));
        }
        out.insert("elapsedMs".into(), json!(self.elapsed_ms));
        out.insert("mediaIds".into(), json!(self.media_ids));
        if let Some(id) = self.captured_prompt_id {
            out.insert("capturedPromptId".into(), json!(id));
        }
        if let Some(error) = self.error.as_ref() {
            out.insert("error".into(), json!(error));
        }
        if let Some(message) = self.message.as_ref() {
            out.insert("message".into(), json!(message));
        }
        Value::Object(out)
    }
}

fn now_iso() -> String {
    crate::iso_time::from_system_time(std::time::SystemTime::now())
}

// ---------------------------------------------------------------------------
//  工作流改写（`ComfySubmitter.WorkflowEdit` 的逐条移植）
// ---------------------------------------------------------------------------

/// 参数覆盖 / 补线。**两个函数都是纯函数**（只动传进来的那张图），所以能单独测。
pub struct WorkflowEdit;

impl WorkflowEdit {
    /// 把一处参数覆盖写进 API 格式节点图。
    ///
    /// `overrides` 的键是 `节点id.输入名`（例如 `3.steps`、`6.text`）。三条规矩：
    ///  1. **只在类型兼容时改写**（原来存的是数字就不能塞字符串 —— ComfyUI 会当场 400，
    ///     而用户看到的是一句莫名其妙的报错）；
    ///  2. 转换不了 / 路径不存在 / 输入本来没有 → **抛错**，让模型自己纠正（绝不静默忽略）；
    ///  3. 返回实际改掉的清单，工具如实告诉用户"改了哪几个参数"。
    pub fn apply_overrides(
        graph: &Value,
        overrides: &Map<String, Value>,
    ) -> Result<(Value, Vec<String>), String> {
        let mut nodes: Map<String, Value> = graph
            .as_object()
            .cloned()
            .ok_or_else(|| "工作流不是 API 格式节点图（顶层要是对象）".to_string())?;
        let mut applied: Vec<String> = Vec::new();
        for (path, value) in overrides {
            let dot = path.rfind('.').ok_or_else(|| {
                format!("参数路径要写成 节点id.输入名（例如 3.steps），收到的是「{path}」")
            })?;
            if dot == 0 || dot + 1 >= path.len() {
                return Err(format!("参数路径要写成 节点id.输入名（例如 3.steps），收到的是「{path}」"));
            }
            let node_id = &path[..dot];
            let field = &path[dot + 1..];
            let node = nodes
                .get(node_id)
                .and_then(Value::as_object)
                .ok_or_else(|| format!("工作流里没有节点 {node_id}（参数路径 {path}）"))?
                .clone();
            let mut inputs = node
                .get("inputs")
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(|| format!("节点 {node_id} 没有 inputs（参数路径 {path}）"))?;
            // 只允许覆盖**这个节点本来就有的输入**：凭空加一个字段 ComfyUI 会当未知参数拒掉，
            // 报错却只会说"节点参数不对"，用户根本查不出来
            let previous = inputs.get(field).ok_or_else(|| {
                let existing: Vec<&str> = inputs.keys().map(String::as_str).collect();
                let listed: String = existing.join("、").chars().take(200).collect();
                format!("节点 {node_id} 没有名为「{field}」的输入（现有：{listed}）")
            })?;
            let coerced = coerce(Some(previous), value, path)?;
            inputs.insert(field.to_string(), coerced);
            let mut patched = node.clone();
            patched.insert("inputs".into(), Value::Object(inputs));
            nodes.insert(node_id.to_string(), Value::Object(patched));
            applied.push(format!("{path} ← {}", scalar_text(value)));
        }
        Ok((Value::Object(nodes), applied))
    }

    /// 把若干**连线**接进 API 图（转换不了的纯前端节点交回给 AI 收拾）。
    ///
    /// 键的写法和 [`Self::apply_overrides`] 一样，值必须是 `["<上游节点id>", 槽位]`。
    /// 与覆盖参数的两点不同，都是有意为之：
    ///  1. **允许写一个当前不存在的输入名** —— 转换时"空着的那个输入"本来就不会出现在 inputs 里，
    ///     而这些正是要补的位置（清单见 `comfy_load_workflow` 返回的 `openInputs`）；
    ///  2. 上游节点必须**在这张图里真的存在** —— 这是最容易写错、后果又最难查的一处。
    pub fn apply_connections(
        graph: &Value,
        connections: &Map<String, Value>,
    ) -> Result<(Value, Vec<String>), String> {
        let mut nodes: Map<String, Value> = graph
            .as_object()
            .cloned()
            .ok_or_else(|| "工作流不是 API 格式节点图（顶层要是对象）".to_string())?;
        let mut applied: Vec<String> = Vec::new();
        for (path, value) in connections {
            let dot = path
                .rfind('.')
                .ok_or_else(|| format!("连线路径要写成 节点id.输入名（例如 60.vae），收到的是「{path}」"))?;
            if dot == 0 || dot + 1 >= path.len() {
                return Err(format!("连线路径要写成 节点id.输入名（例如 60.vae），收到的是「{path}」"));
            }
            let node_id = &path[..dot];
            let field = &path[dot + 1..];
            let node = nodes
                .get(node_id)
                .and_then(Value::as_object)
                .ok_or_else(|| format!("工作流里没有节点 {node_id}（连线路径 {path}）"))?
                .clone();
            let reference = value.as_array().ok_or_else(|| {
                format!("连线 {path} 的值要写成 [\"上游节点id\", 槽位]，例如 [\"1\", 0]")
            })?;
            if reference.len() != 2 {
                return Err(format!("连线 {path} 的值要写成 [上游节点id, 槽位]（两个元素）"));
            }
            let source = primitive_text(&reference[0])
                .ok_or_else(|| format!("连线 {path} 的上游节点 id 要是文本或数字"))?;
            if !nodes.contains_key(&source) {
                let existing: Vec<String> = nodes.keys().take(40).cloned().collect();
                return Err(format!(
                    "连线 {path} 指的节点 {source} 不在这张图里（现有节点：{}）",
                    existing.join("、")
                ));
            }
            let slot = primitive_text(&reference[1])
                .and_then(|text| text.trim().parse::<i64>().ok())
                .ok_or_else(|| format!("连线 {path} 的槽位要是整数"))?;
            let mut inputs = node.get("inputs").and_then(Value::as_object).cloned().unwrap_or_default();
            inputs.insert(field.to_string(), json!([reference[0], slot]));
            let mut patched = node.clone();
            patched.insert("inputs".into(), Value::Object(inputs));
            nodes.insert(node_id.to_string(), Value::Object(patched));
            applied.push(format!("{path} ← [{source}, {slot}]"));
        }
        Ok((Value::Object(nodes), applied))
    }
}

/// 按原值类型转换（Kotlin `coerce`）：原来是整数就不能塞非整数，原来是布尔同理。
fn coerce(previous: Option<&Value>, value: &Value, path: &str) -> Result<Value, String> {
    let is_string = value.is_string();
    if !value.is_number() && !is_string && !value.is_boolean() {
        return Err(format!("参数 {path} 只支持标量（数字 / 文本 / 布尔）"));
    }
    if !is_string {
        return Ok(value.clone());
    }
    let text = value.as_str().unwrap_or_default();
    match previous {
        Some(previous) if previous.is_string() => Ok(json!(text)),
        Some(previous) if previous.is_i64() || previous.is_u64() => text
            .trim()
            .parse::<i64>()
            .map(|n| json!(n))
            .map_err(|_| format!("参数 {path} 原来是整数，给的值「{text}」不是整数")),
        Some(previous) if previous.is_f64() || previous.is_number() => text
            .trim()
            .parse::<f64>()
            .map(|n| json!(n))
            .map_err(|_| format!("参数 {path} 原来是数字，给的值「{text}」不是数字")),
        Some(previous) if previous.is_boolean() => match text.trim() {
            "true" => Ok(json!(true)),
            "false" => Ok(json!(false)),
            _ => Err(format!("参数 {path} 原来是布尔，给的值「{text}」不是布尔")),
        },
        // 原来没有这个字段：数字就当数字，其余当文本
        None => Ok(text
            .trim()
            .parse::<i64>()
            .map(|n| json!(n))
            .or_else(|_| text.trim().parse::<f64>().map(|n| json!(n)))
            .unwrap_or_else(|_| json!(text))),
        Some(_) => Err(format!("参数 {path} 不是标量，不能覆盖")),
    }
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// 标量 → 文本（字符串与数字都认）。
///
/// ⚠ 这条与 `graph_parse` 里那条同源：节点图里的 id 可以是数字（`"negative": [76, 0]`）
/// 也可以是字符串，只认 `as_str` 会让整条引用链静默断掉（真库里踩过）。
fn primitive_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
//  提交前的实例侧预检（纯函数版 + 取清单的薄包装）
// ---------------------------------------------------------------------------

/// 从 `/object_info` 判断这份图的**节点类**能不能跑；`None` = 放行。
///
/// 抽成纯函数就是为了**能测**：`info` 是 `/object_info` 的响应，注入一份夹具即可，
/// 不必真的起一个 ComfyUI。
///
/// ⚠ **这里只拦"节点类在不在实例里"这一档**（`/object_info` 对节点类是权威的：
/// 它列了哪些类，实例就能建哪些节点）。**模型文件那一档只警告、不拦**（[`preflight_warnings`]），
/// 理由是实打实测出来的：
///
/// * Kotlin 的模型预检（`ModelPreflight` + `WorkflowFit`）靠 `ComfyModels` 的**并集清单**
///   （`/object_info` ∪ **盘扫**），而**盘扫那一半 Rust 还没搬**（归 `comfy_env_report`）；
/// * 只按 `/object_info` 的候选判，会把"文件名对不上但盘上真有 / 用户马上要换掉"的图**误拦**——
///   `scripts\e2e-submit-test.ps1` 第 4/5 幕就是活证：那份界面格式工作流写的
///   `e2e_model.safetensors` 不在假实例的候选里，而这在真实场景里只是一次正常的提交
///   （Kotlin 那条链同样不拦：它的清单来源与判据都比"单个输入框的候选"宽）。
///
/// **误拦比白跑一次更糟**（Kotlin 那边也是这个口径），所以这一档现在是**警告**：
/// 提交照常走，但把"实例看不到这些模型文件"如实回给模型与用户。
///
/// ⚠ **纯前端节点不在这里拦**（`Label (rgthree)` / `Anything Everywhere` / 显示节点…）：
/// 它们不是"这个实例没装"，而是"**服务端根本没有这个类**" —— 装节点包也装不上，
/// 那句"让用户装/启用节点包"是错的方向。这一档交给 [`workflow_audit`] 里
/// [`crate::workflow_audit::CHECK_FRONTEND`] 那条结论说（它的修法才对）。
pub fn preflight_refusal(graph: &Value, info: &Value) -> Option<String> {
    let info = info.as_object()?;
    let Some(nodes) = graph.as_object() else { return None };
    if nodes.is_empty() {
        return None;
    }

    let mut unknown: Vec<String> = Vec::new();
    for (node_id, node) in nodes {
        let Some(class_type) = node.get("class_type").and_then(Value::as_str) else { continue };
        if workflow_audit::is_frontend_only(class_type) {
            continue;
        }
        if !info.contains_key(class_type) {
            unknown.push(format!("{node_id}: {class_type}"));
        }
    }
    if unknown.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str(&format!(
        "**没有提交**：这份工作流要的 {} 处东西当前这个 ComfyUI 实例加载不了 —— \
         硬提交只会白跑一次（ComfyUI 只会回一句节点不存在）。\n",
        unknown.len()
    ));
    out.push_str(&format!("\n【实例里没有这些节点类】{} 个：\n", unknown.len()));
    for item in &unknown {
        out.push_str(&format!("  · {item}\n"));
    }
    out.push_str(
        "  → 多半是这个实例没装/没启用那几个自定义节点包：让用户在 ComfyUI Manager 里装、\
         或启停/换一个装了它们的实例。**不要**改用户的工作流。\n",
    );
    Some(out)
}

/// 模型文件那一档的**警告**（不拦，见 [`preflight_refusal`] 的说明）。
///
/// 判据与 Kotlin 的模型预检同一类：类名含 `Loader` 的节点、输入是字符串、
/// 而 `/object_info` 给该输入列的候选里有它没有 —— 那就**很可能**是"盘上有、实例看不到"
/// （重启实例/改用带共享模型根的实例就能解决），也可能是名字确实写错了。
/// 两种情形给的动作都是"先确认、别硬提交"，但**不替用户做决定**。
pub fn preflight_warnings(graph: &Value, info: &Value) -> Vec<String> {
    let Some(info) = info.as_object() else { return Vec::new() };
    let Some(nodes) = graph.as_object() else { return Vec::new() };
    let mut invisible: Vec<String> = Vec::new();
    for (node_id, node) in nodes {
        let Some(class_type) = node.get("class_type").and_then(Value::as_str) else { continue };
        let Some(spec) = info.get(class_type) else { continue };
        // 只查**加载器**节点：模型文件（checkpoint / LoRA / VAE / unet / clip）都在这一类上，
        // 而别的节点（采样器/图片输入/文件名前缀）的候选列表是动态的，拿它判会误报。
        if !class_type.to_ascii_lowercase().contains("loader") {
            continue;
        }
        let Some(required) = spec.get("input").and_then(|v| v.get("required")).and_then(Value::as_object)
        else {
            continue;
        };
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else { continue };
        for (name, value) in inputs {
            let Some(text) = value.as_str() else { continue };
            let allowed = required
                .get(name)
                .and_then(|v| v.get(0))
                .and_then(Value::as_array)
                .filter(|list| !list.is_empty() && list.iter().all(Value::is_string));
            let Some(allowed) = allowed else { continue };
            if !allowed.iter().any(|candidate| candidate.as_str() == Some(text)) {
                invisible.push(format!("{node_id}.{name} 要 {text}"));
            }
        }
    }
    if invisible.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!(
        "⚠ 提交前的预检提示（**没有拦**）：这个实例的 /object_info 里看不到 {} 个模型文件 —— \
         硬提交可能会失败。",
        invisible.len()
    )];
    for item in &invisible {
        out.push(format!("  · {item}"));
    }
    out.push(
        "  → 大概率是「盘上有、实例看不到」（让用户**重启 ComfyUI**——ComfyHub 起实例时会挂上 \
         ComfyUI Desktop 的模型根；**别去下载**）；也可能是名字确实写错了。\
         现在这一档只提示不拦（完整的模型清单要等 `comfy_env_report`/盘扫搬过来）。"
            .to_string(),
    );
    out
}

/// 本机模型清单：与 `comfy_env_report` / `comfy_fit_workflow` / `comfy_audit_workflow`
/// **同一份取法**（`/object_info` ∪ 盘扫）—— 审核的第 ⑦ 项（加载器与目录不匹配）吃它。
///
/// 四件工具各写一份必然漂移，而漂移的表现是"预检说能用、审核说没有"，从界面上看不出来。
fn model_inventory(
    state: &AppState,
    object_info: Option<&Map<String, Value>>,
) -> crate::comfy_models::Inventory {
    let project_root = state.project_root();
    let desktop = crate::mcp::desktop(None);
    let model_dirs: Vec<std::path::PathBuf> =
        desktop.as_ref().map(|d| d.models_dirs.clone()).unwrap_or_default();
    crate::comfy_models::inventory(object_info, Some(&project_root), &model_dirs)
}

/// 三道预检：返回（**阻断性**的拒因，**提示性**的警告）。
///
/// 图是**最终那张**（`overrides` / `connections` 都应用完之后），与 Kotlin 的位置一致。
/// `/object_info` **只问一次**：审核与两道实例侧检查吃的是同一份响应 ——
/// 分两次请求的话，两个闸门可能对着两个不同的实例状态下结论。
async fn preflight(base: &str, graph: &Value, state: &AppState) -> (Option<String>, Vec<String>) {
    let Some(nodes) = graph.as_object() else { return (None, Vec::new()) };
    let info = crate::comfy_client::get_json(&format!("{base}/object_info")).await.ok();
    let object_info = info.as_ref().and_then(Value::as_object);
    let inventory = model_inventory(state, object_info);
    // 审核（`object_info` 拿不到时照样跑：纯前端节点 / 版本不兼容那两条是静态知识表）
    let report = workflow_audit::audit(nodes, object_info, Some(&inventory));

    // ① 实例里没有这个节点类（纯前端那一类留给审核，见 `preflight_refusal`）
    // ② 审核里"必跑不了"的那三项（纯前端节点 / 版本不兼容 / 本机没有的节点类）
    let refusal = info
        .as_ref()
        .and_then(|info| preflight_refusal(graph, info))
        .or_else(|| report.blocking_refusal());

    // ③ 只提醒的那些：实例看不到的模型文件 + 审核里非阻断的结论
    let mut warnings = match info.as_ref() {
        Some(info) => preflight_warnings(graph, info),
        None => Vec::new(),
    };
    if let Some(notes) = report.render_non_blocking() {
        warnings.push(format!("⚠ 提交前的审核提示（**没有拦**）：\n{notes}"));
    }
    (refusal, warnings)
}

// ---------------------------------------------------------------------------
//  提交 + 等 + 入库
// ---------------------------------------------------------------------------

/// 一次提交的结果（工具那一层要的几个字段）。
pub struct SubmitOutcome {
    pub submission: Submission,
    pub applied: Vec<String>,
}

/// `POST /prompt`：把节点图放进队列，拿回 `prompt_id`。
async fn post_prompt(base: &str, graph: &Value) -> Result<String, String> {
    let client_id = uuid_like();
    let body = json!({ "prompt": graph, "client_id": client_id });
    let root = crate::comfy_client::post_json(&format!("{base}/prompt"), &body).await?;
    root.get("prompt_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| id.to_string())
        .ok_or_else(|| format!("ComfyUI 没有返回 prompt_id：{}", take(&root.to_string(), 300)))
}

/// 一个够用的 UUID v4（不引 `uuid` crate：提交只用它当 `client_id`）。
fn uuid_like() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mixed = nanos ^ (pid << 64) ^ (nanos.rotate_left(17));
    let hex = format!("{mixed:032x}");
    format!(
        "{}-{}-4{}-a{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

fn take(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 查这一次运行在 `/history` 里的条目（还没有就 `None`）。
async fn history_entry(base: &str, prompt_id: &str) -> Option<Value> {
    let body = crate::comfy_client::get_json(&format!("{base}/history/{prompt_id}")).await.ok()?;
    body.get(prompt_id).cloned().or_else(|| {
        // 有些实现直接回条目本身
        body.as_object().filter(|map| map.contains_key("status")).map(|_| body.clone())
    })
}

/// 队列里的一次运行在什么位置（给界面一句人话）。
async fn queue_note(base: &str, prompt_id: &str) -> Option<String> {
    let body = crate::comfy_client::get_json(&format!("{base}/queue")).await.ok()?;
    let running = body.get("queue_running").and_then(Value::as_array);
    let pending = body.get("queue_pending").and_then(Value::as_array);
    if index_of_prompt_id(running, prompt_id) >= 0 {
        return Some("正在生成…".to_string());
    }
    let index = index_of_prompt_id(pending, prompt_id);
    if index >= 0 {
        return Some(format!("排队中（前面还有 {index} 个任务）"));
    }
    let total = running.map(Vec::len).unwrap_or(0) + pending.map(Vec::len).unwrap_or(0);
    if total > 0 {
        Some(format!("队列里还有 {total} 个任务"))
    } else {
        None
    }
}

/// 队列项是 `[number, prompt_id, graph, extra, outputs]`，按内容找 prompt_id。
fn index_of_prompt_id(items: Option<&Vec<Value>>, prompt_id: &str) -> i64 {
    let Some(items) = items else { return -1 };
    for (index, item) in items.iter().enumerate() {
        if let Some(row) = item.as_array() {
            if row.iter().any(|cell| primitive_text(cell).as_deref() == Some(prompt_id)) {
                return index as i64;
            }
        }
    }
    -1
}

/// 把一次**跑完了的**运行收进库（Kotlin `ComfyCapture.captureRun`）。
///
/// 与轮询走**同一条路**（[`crate::capture_ingest`]）：幂等靠 `prompt_id` + 文件 sha256。
/// 抢不到说明这一次已经被收过了（轮询先到）—— 那时**如实**回 `alreadyCaptured` 与已有的
/// 提示词 / 产物，绝不重复入库。
pub async fn capture_run(
    state: &AppState,
    config: &CaptureConfig,
    run_key: &str,
    entry: &Value,
    nsfw: bool,
) -> Result<crate::capture_ingest::IngestResult, String> {
    let claim = crate::capture_ingest::begin_run(state, run_key, "ComfyUI", Some(entry), false)
        .map_err(|e| e.human().to_string())?;
    if !claim.claimed {
        let prompt_id = claim.existing_prompt_id;
        let media_ids = match prompt_id {
            Some(id) => state
                .with(|conn| {
                    let mut stmt = conn.prepare("SELECT id FROM media_assets WHERE prompt_id = ?1 ORDER BY id")?;
                    let rows = stmt.query_map(rusqlite::params![id], |row| row.get::<_, i64>(0))?;
                    let mut out = Vec::new();
                    for row in rows {
                        out.push(row?);
                    }
                    Ok(out)
                })
                .unwrap_or_default(),
            None => Vec::new(),
        };
        return Ok(crate::capture_ingest::IngestResult {
            run_key: run_key.to_string(),
            prompt_id,
            media_ids,
            already_captured: true,
            message: Some("这次运行已经捕获过了".to_string()),
            ..Default::default()
        });
    }
    // `isError` 由这一条 `/history` 条目自己说（Kotlin `captureRun` 同：`status_str == "error"`）
    let is_error = entry
        .get("status")
        .and_then(|s| s.get("status_str"))
        .and_then(Value::as_str)
        == Some("error");
    let mut request = crate::capture_ingest::build_ingest_request(run_key, entry, config, is_error);
    request.nsfw = nsfw;
    Ok(crate::capture_ingest::ingest_claimed(state, &request, config).await)
}

/// 工具入口：`comfy_submit`。
pub async fn tool_submit(
    state: &AppState,
    policy: &crate::policy::ToolPolicy,
    arguments: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let overrides = arguments.get("overrides").and_then(Value::as_object).cloned().unwrap_or_default();
    let connections = arguments.get("connections").and_then(Value::as_object).cloned().unwrap_or_default();
    let wait = arguments.get("wait").and_then(Value::as_bool).unwrap_or(true);
    let wait_seconds = arguments
        .get("waitSeconds")
        .and_then(arg_i64)
        .unwrap_or(DEFAULT_WAIT_SECONDS)
        .clamp(0, MAX_WAIT_SECONDS);
    let nsfw = arguments.get("nsfw").and_then(Value::as_bool).unwrap_or(false);
    let title = arguments
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string());

    let mut prompt_id = arguments.get("promptId").and_then(arg_i64);
    let path = arguments
        .get("workflowPath")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty());
    if prompt_id.is_none() && path.is_none() {
        return Err(ToolFailure::new(
            "INVALID_ARGUMENT",
            "要么给 promptId（先 comfy_find_workflow 找），要么给 workflowPath（本机工作流文件路径）",
        ));
    }

    // 给了文件路径就先加载（界面格式在这里被转成 API 节点图）；
    // **允许带缺口入库**（tolerateUnsupported=true）：模型可以同一次调用里一边加载一边补线。
    if prompt_id.is_none() {
        let raw = path.unwrap();
        let file = policy.resolve_read(raw)?;
        let loaded = crate::workflow_load::load_from_file(state, &file, title.as_deref(), false, true, true)
            .await
            .map_err(|failure| ToolFailure::new(&failure.code, failure.message))?;
        prompt_id = loaded.get("promptId").and_then(arg_i64);
        if prompt_id.is_none() {
            return Err(ToolFailure::new("COMFY_LOAD_FAILED", "加载了工作流文件，但没拿到 promptId"));
        }
    }
    let prompt_id = prompt_id.unwrap_or_default();

    // R10：只认 API 格式节点图（取不到就如实报，绝不猜着造一张）
    let graph = crate::capture::prompt_api_graph(state, prompt_id)
        .map_err(|e| ToolFailure::new("TOOL_ERROR", format!("读取节点图出错：{}", e.human())))?
        .ok_or_else(|| {
            let title = prompt_title(state, prompt_id).unwrap_or_else(|| format!("id={prompt_id}"));
            ToolFailure::new(
                "NO_API_GRAPH",
                format!(
                    "这条提示词（{title}）没有 API 格式节点图，不能直接提交。\
                     可以换一条 runnable=true 的，或者先在 ComfyUI 里手动跑一次让它被捕获。"
                ),
            )
        })?;

    let (overridden, mut applied) = WorkflowEdit::apply_overrides(&graph, &overrides)
        .map_err(|message| ToolFailure::new("INVALID_ARGUMENT", message))?;
    let (patched, applied_connections) = WorkflowEdit::apply_connections(&overridden, &connections)
        .map_err(|message| ToolFailure::new("INVALID_ARGUMENT", message))?;
    applied.extend(applied_connections);

    let config = crate::capture::capture_config(state)
        .map_err(|e| ToolFailure::new("TOOL_ERROR", format!("读捕获配置出错：{}", e.human())))?;
    let base = config.comfy_url.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return Err(ToolFailure::new("COMFY_DISABLED", "没有配置 ComfyUI 地址"));
    }

    // 预检放在这里（overrides / connections **都应用完之后**）：检查的就是真正要提交的那张图。
    // 审核里"必跑不了"的三项 + 节点类那一档**拦**；模型文件与审核里非阻断的那些只**警告**
    // （见 `preflight` / `preflight_refusal` 的注释）。
    let (refusal, warnings) = preflight(&base, &patched, state).await;
    if let Some(refusal) = refusal {
        return Err(ToolFailure::new("MODEL_NOT_LOADABLE", refusal));
    }
    if !warnings.is_empty() {
        for line in warnings.iter() {
            eprintln!("[comfy_submit] {}", line.trim());
        }
    }

    let label = title
        .clone()
        .or_else(|| prompt_title(state, prompt_id))
        .unwrap_or_else(|| format!("prompt {prompt_id}"));

    let run_key = post_prompt(&base, &patched).await.map_err(|message| {
        ToolFailure::new(
            "COMFY_SUBMIT_FAILED",
            format!(
                "提交失败：{message}。可以先 comfy_get_status 看 ComfyUI 是否在跑；\
                 不要反复重试同一个工作流。"
            ),
        )
    })?;

    let mut submission = Submission {
        prompt_id: run_key.clone(),
        submitted_by: "ai".to_string(),
        title: Some(label.clone()),
        status: "queued".to_string(),
        submitted_at: now_iso(),
        finished_at: None,
        elapsed_ms: 0,
        media_ids: Vec::new(),
        captured_prompt_id: None,
        error: None,
        message: Some("已提交到 ComfyUI 队列".to_string()),
    };
    state.submissions.record(submission.to_json());

    let started = std::time::Instant::now();
    if wait && wait_seconds > 0 {
        let deadline = started + std::time::Duration::from_secs(wait_seconds as u64);
        while std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
            let Some(entry) = history_entry(&base, &run_key).await else {
                let note = queue_note(&base, &run_key).await;
                submission.status = "running".to_string();
                submission.elapsed_ms = started.elapsed().as_millis() as i64;
                submission.message = Some(note.unwrap_or_else(|| "正在生成…".to_string()));
                state.submissions.record(submission.to_json());
                continue;
            };
            let status_object = entry.get("status");
            let completed = status_object
                .and_then(|s| s.get("completed"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let status_str = status_object.and_then(|s| s.get("status_str")).and_then(Value::as_str);
            let has_outputs = entry
                .get("outputs")
                .and_then(Value::as_object)
                .map(|outputs| !outputs.is_empty())
                .unwrap_or(false);
            if !completed && status_str != Some("error") && !has_outputs {
                submission.status = "running".to_string();
                submission.elapsed_ms = started.elapsed().as_millis() as i64;
                submission.message = Some("正在生成…".to_string());
                state.submissions.record(submission.to_json());
                continue;
            }

            let failed = status_str == Some("error");
            let elapsed = started.elapsed().as_millis() as i64;
            match capture_run(state, &config, &run_key, &entry, nsfw).await {
                Err(message) => {
                    // 运行结束了但入库失败：如实说，别把它说成"没跑"
                    submission.status = if failed { "error" } else { "success" }.to_string();
                    submission.finished_at = Some(now_iso());
                    submission.elapsed_ms = elapsed;
                    submission.error = Some(message.clone());
                    submission.message = Some(format!("运行结束，但产物入库失败：{message}"));
                    state.submissions.record(submission.to_json());
                    break;
                }
                Ok(result) => {
                    submission.status = if failed {
                        "error".to_string()
                    } else if result.imported > 0 {
                        "success".to_string()
                    } else {
                        "empty".to_string()
                    };
                    submission.finished_at = Some(now_iso());
                    submission.elapsed_ms = elapsed;
                    submission.media_ids = result.media_ids.clone();
                    submission.captured_prompt_id = result.prompt_id;
                    submission.error = if failed { result.message.clone() } else { None };
                    submission.message = Some(if failed {
                        "ComfyUI 报错，这次没有产物".to_string()
                    } else if result.imported > 0 {
                        "完成，产物已入库".to_string()
                    } else if result.duplicates > 0 {
                        "完成（产物与已有文件相同，未重复入库）".to_string()
                    } else if result.already_captured {
                        "这次运行已经捕获过了".to_string()
                    } else {
                        "完成，但没有产物文件".to_string()
                    });
                    state.submissions.record(submission.to_json());
                    break;
                }
            }
        }
        if submission.finished_at.is_none() {
            // 超时：**如实说**，不假装失败也不假装完成
            submission.status = "timeout".to_string();
            submission.elapsed_ms = started.elapsed().as_millis() as i64;
            submission.message = Some(format!(
                "等待超过 {wait_seconds} 秒，任务可能还在 ComfyUI 里跑；跑完之后会自动被捕获，\
                 稍后可以在画廊里看到。"
            ));
            state.submissions.record(submission.to_json());
        }
    }

    // 返回给模型的 JSON（`AiWorkflowSearch.submit` 的键集合，一个不多一个不少）
    let mut out = Map::new();
    out.insert("promptId".into(), json!(prompt_id));
    out.insert("runKey".into(), json!(submission.prompt_id));
    out.insert("comfyPromptId".into(), json!(submission.prompt_id));
    out.insert("title".into(), json!(label));
    out.insert("status".into(), json!(submission.status));
    out.insert("elapsedMs".into(), json!(submission.elapsed_ms));
    if let Some(message) = submission.message.as_ref() {
        out.insert("message".into(), json!(message));
    }
    if let Some(error) = submission.error.as_ref() {
        out.insert("error".into(), json!(error));
    }
    if let Some(id) = submission.captured_prompt_id {
        out.insert("capturedPromptId".into(), json!(id));
    }
    out.insert(
        "idHint".into(),
        json!(format!(
            "查这次运行用 comfy_get_run(runKey=\"{}\")；capturedPromptId 是捕获记录编号（画廊 / 提示词用的那个是 promptId）",
            submission.prompt_id
        )),
    );
    out.insert("appliedOverrides".into(), json!(applied));
    out.insert("mediaIds".into(), json!(submission.media_ids));
    out.insert("mediaCount".into(), json!(submission.media_ids.len()));
    out.insert("nsfw".into(), json!(nsfw));
    // 预检的**提示**（不拦）：让模型/用户知道"实例可能看不到这几个模型文件"
    if !warnings.is_empty() {
        out.insert("preflightWarnings".into(), json!(warnings));
    }
    out.insert(
        "nsfwMediaIds".into(),
        if nsfw { json!(submission.media_ids) } else { json!(Vec::<i64>::new()) },
    );
    if submission.status == "timeout" {
        out.insert(
            "hint".into(),
            json!(format!(
                "还没跑完：产物会在跑完后自动入库，让用户稍后刷新画廊即可；也可以隔一会儿用 comfy_get_run runKey={} 查",
                submission.prompt_id
            )),
        );
    }
    let payload = Value::Object(out);
    Ok(ToolOutcome::new(payload.to_string(), payload))
}

fn arg_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// 库里那条提示词的标题（找不到就 `None`）。
fn prompt_title(state: &AppState, id: i64) -> Option<String> {
    state
        .with(|conn| {
            Ok(conn
                .query_row("SELECT title FROM prompts WHERE id = ?1", rusqlite::params![id], |row| {
                    row.get::<_, Option<String>>(0)
                })
                .ok()
                .flatten())
        })
        .ok()
        .flatten()
        .map(|title| title.trim().to_string())
        .filter(|title| !title.is_empty())
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> Value {
        json!({
            "3": {
                "class_type": "KSampler",
                "inputs": { "seed": 42, "steps": 20, "cfg": 7.5, "denoise": 1.0, "model": ["4", 0] }
            },
            "4": {
                "class_type": "CheckpointLoaderSimple",
                "inputs": { "ckpt_name": "krea2.safetensors" }
            },
            "6": { "class_type": "CLIPTextEncode", "inputs": { "text": "old prompt", "clip": ["4", 1] } }
        })
    }

    #[test]
    fn 覆盖参数按原值类型转换() {
        let mut overrides = Map::new();
        overrides.insert("3.steps".into(), json!("30")); // 原是整数 → 转成整数
        overrides.insert("6.text".into(), json!("new prompt")); // 原是文本
        overrides.insert("3.cfg".into(), json!(1.0)); // 数字原样
        let (patched, applied) = WorkflowEdit::apply_overrides(&graph(), &overrides).unwrap();
        assert_eq!(patched["3"]["inputs"]["steps"], json!(30));
        assert_eq!(patched["6"]["inputs"]["text"], json!("new prompt"));
        assert_eq!(patched["3"]["inputs"]["cfg"], json!(1.0));
        assert_eq!(applied.len(), 3);
        assert!(applied.contains(&"3.steps ← 30".to_string()));
        // 原图不许被改（纯函数）
        assert_eq!(graph()["3"]["inputs"]["steps"], json!(20));
    }

    #[test]
    fn 覆盖参数的四种拒绝都是人话() {
        let mut bad_path = Map::new();
        bad_path.insert("3".into(), json!("x"));
        assert!(WorkflowEdit::apply_overrides(&graph(), &bad_path).unwrap_err().contains("节点id.输入名"));

        let mut no_node = Map::new();
        no_node.insert("99.steps".into(), json!(1));
        assert!(WorkflowEdit::apply_overrides(&graph(), &no_node).unwrap_err().contains("没有节点 99"));

        let mut no_input = Map::new();
        no_input.insert("3.nope".into(), json!(1));
        let message = WorkflowEdit::apply_overrides(&graph(), &no_input).unwrap_err();
        assert!(message.contains("没有名为「nope」的输入"), "{message}");
        assert!(message.contains("seed"), "要列出已有的输入名：{message}");

        let mut wrong_type = Map::new();
        wrong_type.insert("3.steps".into(), json!("很多"));
        assert!(WorkflowEdit::apply_overrides(&graph(), &wrong_type)
            .unwrap_err()
            .contains("原来是整数"));

        let mut non_scalar = Map::new();
        non_scalar.insert("3.steps".into(), json!([1, 2]));
        assert!(WorkflowEdit::apply_overrides(&graph(), &non_scalar)
            .unwrap_err()
            .contains("只支持标量"));
    }

    #[test]
    fn 补线允许新输入但要求上游节点存在() {
        let mut connections = Map::new();
        // VAELoader 那边缺的线：节点 5 是空的（转换时被摘过），这里补一个**新输入名**
        let mut with_hole = graph();
        with_hole["6"]["inputs"].as_object_mut().unwrap().remove("clip");
        connections.insert("6.clip".into(), json!(["4", 1]));
        let (patched, applied) = WorkflowEdit::apply_connections(&with_hole, &connections).unwrap();
        assert_eq!(patched["6"]["inputs"]["clip"], json!(["4", 1]));
        assert_eq!(applied, vec!["6.clip ← [4, 1]".to_string()]);

        let mut bad_source = Map::new();
        bad_source.insert("6.clip".into(), json!(["77", 0]));
        let message = WorkflowEdit::apply_connections(&graph(), &bad_source).unwrap_err();
        assert!(message.contains("节点 77 不在这张图里"), "{message}");

        let mut bad_shape = Map::new();
        bad_shape.insert("6.clip".into(), json!(["4"]));
        assert!(WorkflowEdit::apply_connections(&graph(), &bad_shape).unwrap_err().contains("两个元素"));

        let mut bad_slot = Map::new();
        bad_slot.insert("6.clip".into(), json!(["4", "第一个"]));
        assert!(WorkflowEdit::apply_connections(&graph(), &bad_slot).unwrap_err().contains("槽位要是整数"));

        // 数字 id 与字符串 id 必须都能用（真库里有 `"negative": [76, 0]` 这种）
        let mut numeric = Map::new();
        numeric.insert("6.clip".into(), json!([4, 1]));
        let (patched, _) = WorkflowEdit::apply_connections(&graph(), &numeric).unwrap();
        assert_eq!(patched["6"]["inputs"]["clip"], json!([4, 1]));
    }

    fn object_info() -> Value {
        json!({
            "KSampler": { "input": { "required": { "seed": ["INT", {}], "steps": ["INT", {}] } } },
            "CheckpointLoaderSimple": {
                "input": { "required": { "ckpt_name": [["krea2.safetensors", "anima.safetensors"], {}] } }
            },
            "CLIPTextEncode": { "input": { "required": { "text": ["STRING", {}] } } }
        })
    }

    #[test]
    fn 预检_节点类拦_模型文件只警告() {
        assert!(preflight_refusal(&graph(), &object_info()).is_none(), "都对就必须放行");

        // ① 实例不认识的节点类 → **拦**（`/object_info` 对节点类是权威的）
        let mut missing_node = graph();
        missing_node["12"] = json!({ "class_type": "Krea2CustomNode", "inputs": {} });
        let refusal = preflight_refusal(&missing_node, &object_info()).unwrap();
        assert!(refusal.contains("实例里没有这些节点类"), "{refusal}");
        assert!(refusal.contains("12: Krea2CustomNode"), "{refusal}");
        assert!(refusal.contains("不要") && refusal.contains("改用户的工作流"), "{refusal}");

        // ② 模型文件不在实例的候选里 → **只警告不拦**（见 `preflight_refusal` 的说明：
        //    盘扫还没搬，只按候选判会误拦；e2e-submit 第 4/5 幕就是活证）
        let mut wrong_model = graph();
        wrong_model["4"]["inputs"]["ckpt_name"] = json!("anima-base-v1.0.safetensors");
        assert!(
            preflight_refusal(&wrong_model, &object_info()).is_none(),
            "模型文件那一档现在是提示，不许拦"
        );
        let warnings = preflight_warnings(&wrong_model, &object_info());
        assert_eq!(warnings.len(), 3, "首行 + 一条明细 + 一条动作：{warnings:?}");
        let joined = warnings.join("\n");
        assert!(joined.contains("没有拦"), "{joined}");
        assert!(joined.contains("4.ckpt_name 要 anima-base-v1.0.safetensors"), "{joined}");
        assert!(joined.contains("重启 ComfyUI"), "{joined}");
        assert!(joined.contains("别去下载"), "{joined}");
        // 都对的时候没有警告
        assert!(preflight_warnings(&graph(), &object_info()).is_empty());

        // ③ 非加载器节点的候选列表**不参与**判定（图片输入那种动态列表会误报）
        let mut with_image = graph();
        with_image["9"] = json!({ "class_type": "LoadImage", "inputs": { "image": "not-in-list.png" } });
        let mut info = object_info();
        info["LoadImage"] = json!({ "input": { "required": { "image": [["a.png"], {}] } } });
        assert!(preflight_refusal(&with_image, &info).is_none(), "LoadImage 不是 Loader，不判它");
        assert!(preflight_warnings(&with_image, &info).is_empty(), "也不该警告");
    }

    #[test]
    fn 纯前端节点不走实例侧这一档_交给审核那一条说() {
        // 对 `Label (rgthree)` / `Anything Everywhere` 说"让用户装/启用节点包"是**错的方向**：
        // 这些类只存在于 ComfyUI 前端，服务端根本没有（装也装不上）。
        // 这一档必须让给 `workflow_audit` 里那条"服务端根本没有这些类"的结论。
        let mut frontend = graph();
        frontend["2"] = json!({ "class_type": "Label (rgthree)", "inputs": {} });
        frontend["5"] = json!({ "class_type": "Anything Everywhere", "inputs": {} });
        frontend["8"] = json!({ "class_type": "PreviewImage", "inputs": {} });
        assert!(
            preflight_refusal(&frontend, &object_info()).is_none(),
            "纯前端节点要被跳过（它们不在这一档的射程里）"
        );
        // 而审核那一档真的会拦它们（否则上面那个 continue 就成了漏网）
        let info = object_info();
        let report =
            workflow_audit::audit(frontend.as_object().unwrap(), info.as_object(), None);
        assert!(
            report.blocking_refusal().is_some(),
            "纯前端节点必须由审核拦下：{}",
            report.render()
        );
        // 判据本身（三张表/模式的合取）钉一下
        for cls in ["Label (rgthree)", "Anything Everywhere", "PreviewImage", "Display Any"] {
            assert!(workflow_audit::is_frontend_only(cls), "{cls}");
        }
        for cls in ["Krea2CustomNode", "KSampler", "CheckpointLoaderSimple"] {
            assert!(!workflow_audit::is_frontend_only(cls), "{cls} 不是纯前端类");
        }
    }

    #[test]
    fn 提交记录的保留窗口与就地更新() {
        let submissions = Submissions::new();
        for i in 0..(MAX_TRACKED + 5) {
            let mut item = Submission {
                prompt_id: format!("run-{i:03}"),
                submitted_by: "ai".into(),
                title: None,
                status: "queued".into(),
                submitted_at: format!("2026-09-26T00:00:{i:02}Z"),
                finished_at: None,
                elapsed_ms: 0,
                media_ids: Vec::new(),
                captured_prompt_id: None,
                error: None,
                message: None,
            }
            .to_json();
            // submittedAt 只到秒，秒数超过 59 会不合法 —— 直接改成递增的分钟
            item["submittedAt"] = json!(format!("2026-09-26T00:{:02}:00Z", i));
            submissions.record(item);
        }
        let listed = submissions.list(MAX_TRACKED);
        assert_eq!(listed.len(), MAX_TRACKED, "只留最近 {MAX_TRACKED} 条");
        assert_eq!(listed[0]["promptId"], json!(format!("run-{:03}", MAX_TRACKED + 4)), "新的在前");

        // 同一个 promptId 就地更新，不新增一条
        let before = submissions.list(MAX_TRACKED).len();
        let mut updated = Submission {
            prompt_id: "run-034".into(),
            submitted_by: "ai".into(),
            title: Some("改了标题".into()),
            status: "success".into(),
            submitted_at: "2026-09-26T01:00:00Z".into(),
            finished_at: Some("2026-09-26T01:00:05Z".into()),
            elapsed_ms: 5000,
            media_ids: vec![7, 8],
            captured_prompt_id: Some(333),
            error: None,
            message: Some("完成，产物已入库".into()),
        }
        .to_json();
        updated["promptId"] = json!("run-034");
        submissions.record(updated);
        assert_eq!(submissions.list(MAX_TRACKED).len(), before, "就地更新不许多一行");
        let found = submissions.find("run-034").unwrap();
        assert_eq!(found["status"], json!("success"));
        assert_eq!(found["mediaIds"], json!([7, 8]));
        assert_eq!(found["capturedPromptId"], json!(333));
        assert!(submissions.find("不存在").is_none());
    }

    #[test]
    fn 提交记录的形状与_kotlin_的可空约定一致() {
        let queued = Submission {
            prompt_id: "abc".into(),
            submitted_by: "ai".into(),
            title: None,
            status: "queued".into(),
            submitted_at: "2026-09-26T00:00:00Z".into(),
            finished_at: None,
            elapsed_ms: 0,
            media_ids: Vec::new(),
            captured_prompt_id: None,
            error: None,
            message: None,
        }
        .to_json();
        let map = queued.as_object().unwrap();
        // 有默认值的键一律出现（Kotlin `encodeDefaults = true`）
        for key in ["promptId", "submittedBy", "status", "submittedAt", "elapsedMs", "mediaIds"] {
            assert!(map.contains_key(key), "{key} 必须出现：{queued}");
        }
        // 可空键为 null 时整键省略（`explicitNulls = false`）
        for key in ["title", "finishedAt", "capturedPromptId", "error", "message"] {
            assert!(!map.contains_key(key), "{key} 不该出现：{queued}");
        }
    }
}
