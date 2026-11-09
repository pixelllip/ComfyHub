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
//!     （[`crate::comfy::capture::prompt_api_graph`]）。界面格式（`nodes`/`links`）的 `widgets_values`
//!     只有位置没有参数名，猜着转**一定是错的**。取不到就报 `NO_API_GRAPH`，绝不猜着造一张。
//!  2. **提交后必须自带"等 + 立即入库"**：ComfyUI 的 `/history` 是**进程内的**
//!     （2026-09-25 实测：重启实例之后上一次运行就没了）。所以"提交完靠后台轮询补收"
//!     只在同一个实例生命周期内成立 —— 主路是这里自己盯着跑完并 [`capture_run`]，
//!     轮询（[`crate::comfy::capture_poller`]）只是兜底。
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
//!  | 审核里"必跑不了"的三项 | [`crate::workflow::workflow_audit::audit`] 的 [`Report::blocking_refusal`]：纯前端节点 / 已知版本不兼容节点 / 本机没有的节点类 | 照审核给的那句话做（更新节点包 / 让 `comfy_load_workflow` 照前端行为改写 / 启停实例） |
//!  | 节点类在不在实例里 | `class_type` 不在 `/object_info` 的键里（**纯前端那一类除外**，见下） | 让用户装/启用那几个自定义节点包，或换实例；**不要**改用户的工作流 |
//!  | 模型文件实例认不认 | 节点类名含 `Loader` 的、输入值是字符串、而 `/object_info` 给该输入列的候选里有它没有 | 让用户**重启 ComfyUI**（ComfyHub 起实例时会挂上 Desktop 的模型根）；**别去下载** |
//!
//! 后两道是**实例侧**的（`/object_info` 直接说了算）；第一道是第 63 步那份
//! [`crate::workflow::workflow_audit`] 的第 70 步接线，位置与 Kotlin `Application.kt` 的 `preflight`
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

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

use crate::comfy::capture::CaptureConfig;
use crate::tools::tools::{ToolFailure, ToolOutcome};
use crate::web::routes::AppState;
use crate::workflow::workflow_audit;

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
        let prompt_id = submission
            .get("promptId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
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
                item.get("submittedAt")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            });
            let drop_count = guard.len() - MAX_TRACKED;
            guard.drain(0..drop_count);
        }
        // 新的在前（Kotlin `sortedByDescending { it.submittedAt }`）
        guard.sort_by(|a, b| {
            let left = b
                .get("submittedAt")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let right = a
                .get("submittedAt")
                .and_then(Value::as_str)
                .unwrap_or_default();
            left.cmp(right)
        });
    }

    /// 最近的提交记录，新的在前。
    pub fn list(&self, limit: usize) -> Vec<Value> {
        let guard = self.items.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .iter()
            .take(limit.clamp(1, MAX_TRACKED))
            .cloned()
            .collect()
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
    crate::infra::iso_time::from_system_time(std::time::SystemTime::now())
}

/// T2（2026-11-09）：`comfy_submit` **跑通之后**把"这条配方 × 这组槽位覆盖"记成已验证。
///
/// 为什么只在这里写：失败 / 超时 / 取消记下来的话，"已验证"就退化成"试过"，
/// 而床上场景（`docs/workflow-auto-select-plan.md` §1.2 第 ② 档）正是靠这一档
/// 决定**能不能不打扰用户直接开跑** —— 一次假绿灯就是一次白烧显卡。
///
/// 三条边界：
///  * 对不上配方（用户随手跑的工作流 / 没学过它）→ **不记**（那不是配方验证）；
///  * 只记**这次真的写进 overrides 且属于槽位键**的那几个值（旋钮键不算"这个模型能用"）；
///  * 落库失败只打日志：验证是**下一次**的复利，不该影响这一次已经成功的提交。
fn record_verified_binding(
    state: &AppState,
    recipe_id: Option<i64>,
    overrides: &Map<String, Value>,
    run_key: &str,
) {
    use crate::workflow::workflow_recipe::record_verified;
    let Some(recipe_id) = recipe_id else {
        return;
    };
    let row = match state.with(|conn| crate::workflow::workflow_recipe::get(conn, recipe_id)) {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(error) => {
            eprintln!("[comfy_submit] 读配方失败，跳过已验证回写：{}", error.human());
            return;
        }
    };
    let entries = verified_entries(&row.to_recipe(), overrides, run_key, Some(now_iso()));
    if entries.is_empty() {
        return;
    }
    if let Err(error) = state.write(|conn| Ok(record_verified(conn, recipe_id, &entries)?)) {
        eprintln!("[comfy_submit] 已验证回写失败（这次提交本身不受影响）：{error:?}");
    }
}

/// 从"这次真的提交的 overrides"里挑出**属于模型槽位**的那几条，变成验证记录。
///
/// 只认槽位键：旋钮（提示词 / 步数 / seed）改了不代表"这个模型能用"，
/// 而 `overrides` 里还可能有 `positivePrompt` 派生出来的键 —— 都不该冒充验证。
fn verified_entries(
    recipe: &crate::workflow::workflow_recipe::Recipe,
    overrides: &Map<String, Value>,
    run_key: &str,
    at: Option<String>,
) -> Vec<crate::workflow::workflow_recipe::Verified> {
    let mut entries = Vec::new();
    for slot in &recipe.slots {
        let key = slot.override_key();
        let Some(raw) = overrides.get(&key) else {
            continue;
        };
        let model = scalar_text(raw).trim().to_string();
        if model.is_empty() {
            continue;
        }
        entries.push(crate::workflow::workflow_recipe::Verified {
            model,
            role: slot.role.clone(),
            override_key: key,
            run_key: run_key.to_string(),
            at: at.clone(),
        });
    }
    entries
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
                return Err(format!(
                    "参数路径要写成 节点id.输入名（例如 3.steps），收到的是「{path}」"
                ));
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
    ///
    /// ⚠ 写进图里的引用**永远是字符串 id**（哪怕调用方给的是数字）：ComfyUI 拿它索引
    /// prompt 字典，数字会 `KeyError`（与 `workflow_convert::ref_of` 同一条纪律）。
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
            let dot = path.rfind('.').ok_or_else(|| {
                format!("连线路径要写成 节点id.输入名（例如 60.vae），收到的是「{path}」")
            })?;
            if dot == 0 || dot + 1 >= path.len() {
                return Err(format!(
                    "连线路径要写成 节点id.输入名（例如 60.vae），收到的是「{path}」"
                ));
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
                return Err(format!(
                    "连线 {path} 的值要写成 [上游节点id, 槽位]（两个元素）"
                ));
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
            let mut inputs = node
                .get("inputs")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            // ⚠ 引用里的 id **一律写字符串**（`["4", 1]`）：ComfyUI 拿它直接索引 prompt 字典，
            // 写成数字就是 `KeyError('4')`（见 `workflow_convert::ref_of` 的注释）。
            inputs.insert(field.to_string(), json!([source, slot]));
            let mut patched = node.clone();
            patched.insert("inputs".into(), Value::Object(inputs));
            nodes.insert(node_id.to_string(), Value::Object(patched));
            applied.push(format!("{path} ← [{source}, {slot}]"));
        }
        Ok((Value::Object(nodes), applied))
    }

    /// 把**最终正面提示词**直接写进正面文本编码器，并从提交图里摘掉原来那一路。
    ///
    /// # 为什么需要它（用户 2026-10-21）
    ///
    /// 很多现成工作流里有一个 `TextGenerate`（本地 LLM 扩写 / 布局编译器）：
    /// 它的输出经过 `ComfySwitchNode` / `Fast Bypasser` 一路接到 `CLIPTextEncode.text`。
    /// 以前模型只能干看着：直接覆盖 `text` 会被"这是连线"拒掉；不覆盖就只能让本地 LLM
    /// 按它自己的随机性扩写（慢、不可控，还经常把中文写成英文标签）。
    ///
    /// 现在模型可以**自己服从 `TextGenerate.system_prompt`** 写好最终提示词，再走这个参数：
    /// 1. 从每个采样器的 `positive` 输入往上找**最近的** `CLIPTextEncode`（不碰 negative）；
    /// 2. 把它的 `text` 改成字面量 —— 原来那条线**就此断开**；
    /// 3. **再把 TextGenerate 这一支从提交图里删掉**（2026-10-24 用户口径：摘线 ≠ 绕过）：
    ///    只断线不够，节点还在提交图里，而 ComfyUI 的 `validate_prompt` 校验的是**整张 prompt**
    ///    （不只可达节点）—— 本机没装 TextGenerate 节点包、或它的动态下拉框值不合法时，
    ///    整份报错（`Node type not found: TextGenerate`），哪怕一个输出都没接。
    ///    只删节点不清引用也不行，留下 `39.on_true → [32,0]` 又会 `Node 32 not found`
    ///    （用户看到的"有节点没接入"）。
    ///    所以这里**成对做**：删 TextGenerate 这一支 + 把指向它的下游一并摘掉 / 把只喂它的
    ///    上游一并清掉，删完校验"没有悬挂引用、至少还剩一个输出"；校验不过就退回只断线。
    /// 4. 用户自己的文件**一个字节都不动**：改的只是这一次提交的图。
    ///
    /// 返回 `(改过的图, 改过的 节点id.text, 从提交图里摘掉的节点 id)`。
    /// **找不到正面文本编码器就报错**：静默什么都不改会让模型以为已经按它的提示词出了图。
    pub fn apply_positive_prompt(
        graph: &Value,
        prompt: &str,
    ) -> Result<(Value, Vec<String>, Vec<String>), String> {
        let mut nodes: Map<String, Value> = graph
            .as_object()
            .cloned()
            .ok_or_else(|| "工作流不是 API 格式节点图（顶层要是对象）".to_string())?;
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(
                "positivePrompt 是空的：要给最终提示词（不是给用户原话让本地 LLM 扩写）".into(),
            );
        }

        // ① 所有"正面条件"入口：
        //    · KSampler / CFGGuider 这类有 `positive` 插口的；
        //    · `BasicGuider` 那种只有 `conditioning` 的（Krea 2 的
        //      `SamplerCustomAdvanced → BasicGuider → CLIPTextEncode` 就是它）。
        let mut starts: Vec<String> = Vec::new();
        for node in nodes.values() {
            let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
                continue;
            };
            if let Some((up, _)) = inputs.get("positive").and_then(link_ref) {
                starts.push(up);
            }
            let class = node.get("class_type").and_then(Value::as_str).unwrap_or("");
            if class.to_ascii_lowercase().contains("guider") {
                if let Some((up, _)) = inputs.get("conditioning").and_then(link_ref) {
                    starts.push(up);
                }
            }
        }
        if starts.is_empty() {
            return Err(
                "这份工作流里找不到 `positive` 条件输入（它不是一张能出图的条件采样图）；\
                 不要传 positivePrompt"
                    .into(),
            );
        }

        // ② 往上找最近的文本编码器（遇到就停，不再往它的 clip 上游走）
        let mut queue: VecDeque<(String, usize)> =
            starts.into_iter().map(|id| (id, 0usize)).collect();
        let mut seen: HashSet<String> = HashSet::new();
        let mut found: Vec<(String, usize)> = Vec::new();
        while let Some((id, depth)) = queue.pop_front() {
            if depth > 32 || !seen.insert(id.clone()) {
                continue;
            }
            let Some(node) = nodes.get(&id).and_then(Value::as_object) else {
                continue;
            };
            let class = node.get("class_type").and_then(Value::as_str).unwrap_or("");
            let is_text_encode = class.to_ascii_lowercase().contains("textencode");
            if is_text_encode {
                let has_text = node
                    .get("inputs")
                    .and_then(Value::as_object)
                    .map(|inputs| inputs.contains_key("text"))
                    .unwrap_or(false);
                if has_text {
                    found.push((id, depth));
                }
                continue;
            }
            let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
                continue;
            };
            for (key, value) in inputs {
                if !follows_positive(key) {
                    continue;
                }
                if let Some((up, _)) = link_ref(value) {
                    queue.push_back((up, depth + 1));
                }
            }
        }
        if found.is_empty() {
            return Err(
                "顺着 `positive` 往上没找到带 text 的 CLIPTextEncode：这份工作流的正面条件可能是\
                 别的方式编码的。如实告诉用户，不要硬塞 positivePrompt"
                    .into(),
            );
        }
        // 只改**最近的那一层**：更上游的编码器可能是风格 / 参考分支，不该被同一句话覆盖。
        let nearest = found.iter().map(|(_, depth)| *depth).min().unwrap_or(0);
        found.retain(|(_, depth)| *depth == nearest);
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found.dedup_by(|a, b| a.0 == b.0);

        // ③ 先记下"原来接在正面 text 上的 TextGenerate"（用于如实回报），再断开
        let mut bypassed: BTreeSet<String> = BTreeSet::new();
        for (id, _) in &found {
            let Some(link) = nodes
                .get(id)
                .and_then(|node| node.get("inputs"))
                .and_then(Value::as_object)
                .and_then(|inputs| inputs.get("text"))
                .and_then(link_ref)
            else {
                continue;
            };
            collect_text_generate(&nodes, &link.0, &mut bypassed);
        }

        let mut changed: Vec<String> = Vec::new();
        for (id, _) in &found {
            let Some(node) = nodes.get_mut(id).and_then(Value::as_object_mut) else {
                continue;
            };
            let entry = node.entry("inputs").or_insert_with(|| json!({}));
            let Some(inputs) = entry.as_object_mut() else {
                continue;
            };
            inputs.insert("text".into(), json!(prompt));
            changed.push(format!("{id}.text"));
        }
        // ④ 从**提交图**里摘掉 TextGenerate 这一支（摘节点 + 清引用，成对做）。
        let removed = remove_text_generate_branch(&mut nodes, &bypassed);
        Ok((Value::Object(nodes), changed, removed))
    }
}

/// 顺着正面条件往上走时要跟哪些输入。
///
/// 刻意只跟"条件/开关"这几类：跟着 `model` / `clip` / `image` 走到加载器上，
/// 既没有意义，也会在几百个节点的工作流上白跑一大圈。
fn follows_positive(key: &str) -> bool {
    key == "positive"
        || key == "conditioning"
        || key.starts_with("conditioning_")
        || key == "on_true"
        || key == "on_false"
}

/// 从某个输入往上走，收集 `TextGenerate` 一族（摘线时先记下"原来接着的是哪一路"）。
fn collect_text_generate(nodes: &Map<String, Value>, start: &str, out: &mut BTreeSet<String>) {
    let mut queue: VecDeque<(String, usize)> = VecDeque::from([(start.to_string(), 0usize)]);
    let mut seen: HashSet<String> = HashSet::new();
    while let Some((id, depth)) = queue.pop_front() {
        if depth > 16 || !seen.insert(id.clone()) {
            continue;
        }
        let Some(node) = nodes.get(&id).and_then(Value::as_object) else {
            continue;
        };
        let class = node.get("class_type").and_then(Value::as_str).unwrap_or("");
        if class.to_ascii_lowercase().contains("textgenerate") {
            out.insert(id.clone());
        }
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        for value in inputs.values() {
            if let Some((up, _)) = link_ref(value) {
                queue.push_back((up, depth + 1));
            }
        }
    }
}

fn node_class(node: &Value) -> Option<&str> {
    node.get("class_type").and_then(Value::as_str)
}

/// 一个节点上所有连线型输入：`[(上游节点 id, 槽位)]`。
fn node_input_links(node: &Value) -> Vec<(String, i64)> {
    let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
        return Vec::new();
    };
    inputs.values().filter_map(link_ref).collect()
}

/// 输出节点（有它，"这张图有结果"才成立）。
///
/// 判据与 `workflow_prune::OUTPUT_HINTS` 同一档：Save / Preview / VideoCombine / Export。
/// 摘 TextGenerate 那支时要靠它兜底 —— 把最后一个输出摘掉，提交就没意义了。
fn is_output_class(class: &str) -> bool {
    let lower = class.to_ascii_lowercase();
    lower.contains("save")
        || lower.contains("preview")
        || lower.contains("videocombine")
        || lower.contains("export")
}

/// 顺着输入往上走，收集这些种子的**全部上游**（含种子自己）。
fn collect_ancestors(nodes: &Map<String, Value>, seeds: &BTreeSet<String>) -> BTreeSet<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<(String, usize)> = seeds.iter().map(|s| (s.clone(), 0usize)).collect();
    while let Some((id, depth)) = queue.pop_front() {
        if depth > 32 || !seen.insert(id.clone()) {
            continue;
        }
        if let Some(node) = nodes.get(&id) {
            for (src, _) in node_input_links(node) {
                queue.push_back((src, depth + 1));
            }
        }
    }
    seen
}

/// 摘完这一支之后：仍然至少有一个输出节点，且没有节点再指向被摘掉的 id。
///
/// 只在自己的摘除范围内判（图本来就有的悬挂引用不归这里管）；判不过就**放弃摘除**、
/// 退回"只断线"，别为了干净把整张图搞成提交不了的形状。
fn graph_is_sound(nodes: &Map<String, Value>, removed: &BTreeSet<String>) -> bool {
    let has_output = nodes.iter().any(|(id, node)| {
        !removed.contains(id) && node_class(node).map(is_output_class).unwrap_or(false)
    });
    if !has_output {
        return false;
    }
    nodes.iter().all(|(id, node)| {
        removed.contains(id)
            || node_input_links(node)
                .iter()
                .all(|(src, _)| !removed.contains(src))
    })
}

/// 把 `TextGenerate` 这一支从**提交图**里摘掉，返回真正删掉的节点 id（升序）。
///
/// 2026-10-24 用户口径："摘除了还得阻止该节点运行（绕过节点），不然工作流运行时可能
/// 产生报错（有节点没接入之类）"。两件事必须成对做：
///
/// 1. **删节点**：ComfyUI 的 `validate_prompt` 校验整张 prompt（不只可达节点），
///    留在图里的 `TextGenerate` 仍会被查类名 / 动态下拉框 —— 本机没装那个节点包时
///    整份报错，哪怕它一个输出都没接；
/// 2. **清引用**：只删节点会留下 `39.on_true → [32,0]`，变成 `Node 32 not found`。
///
/// 做法：从种子（TextGenerate）出发，① 下游**任何输入指向已摘除节点的**一并摘掉；
/// ② 上游只喂给已摘除节点、自己又不是输出节点的辅助节点一并清掉。删完先校验，
/// 校验不过是"至少剩一个输出 + 没有悬挂引用"，不过就退回空列表（调用方保留只断线的图）。
fn remove_text_generate_branch(
    nodes: &mut Map<String, Value>,
    seeds: &BTreeSet<String>,
) -> Vec<String> {
    if seeds.is_empty() {
        return Vec::new();
    }
    let ancestors = collect_ancestors(nodes, seeds);
    let mut removed = seeds.clone();
    loop {
        let mut victim: Option<String> = None;
        // ① 下游：输入指着已摘除节点的，一并摘掉（否则就是"有节点没接入"）
        for (id, node) in nodes.iter() {
            if removed.contains(id) {
                continue;
            }
            if node_input_links(node)
                .iter()
                .any(|(src, _)| removed.contains(src))
            {
                victim = Some(id.clone());
                break;
            }
        }
        // ② 上游：只喂给已摘除节点、自己又不是输出的辅助节点，一并清掉
        if victim.is_none() {
            for id in &ancestors {
                if removed.contains(id) {
                    continue;
                }
                let Some(node) = nodes.get(id) else { continue };
                if node_class(node).map(is_output_class).unwrap_or(false) {
                    continue;
                }
                let has_live_consumer = nodes.iter().any(|(other, n)| {
                    other != id
                        && !removed.contains(other)
                        && node_input_links(n).iter().any(|(src, _)| src == id)
                });
                if !has_live_consumer {
                    victim = Some(id.clone());
                    break;
                }
            }
        }
        match victim {
            Some(id) => {
                removed.insert(id);
            }
            None => break,
        }
    }
    if !graph_is_sound(nodes, &removed) {
        return Vec::new();
    }
    let mut out: Vec<String> = removed.into_iter().collect();
    for id in &out {
        nodes.remove(id);
    }
    out.sort();
    out
}

/// `["12", 0]` / `[12, 0]` → `("12", 0)`。
fn link_ref(value: &Value) -> Option<(String, i64)> {
    let array = value.as_array()?;
    if array.len() != 2 {
        return None;
    }
    let source = primitive_text(&array[0])?;
    let slot = primitive_text(&array[1])?.trim().parse::<i64>().ok()?;
    Some((source, slot))
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
/// [`crate::workflow::workflow_audit::CHECK_FRONTEND`] 那条结论说（它的修法才对）。
pub fn preflight_refusal(graph: &Value, info: &Value) -> Option<String> {
    let info = info.as_object()?;
    let nodes = graph.as_object()?;
    if nodes.is_empty() {
        return None;
    }

    let mut unknown: Vec<String> = Vec::new();
    for (node_id, node) in nodes {
        let Some(class_type) = node.get("class_type").and_then(Value::as_str) else {
            continue;
        };
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
    out.push_str(&format!(
        "\n【实例里没有这些节点类】{} 个：\n",
        unknown.len()
    ));
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
    let Some(info) = info.as_object() else {
        return Vec::new();
    };
    let Some(nodes) = graph.as_object() else {
        return Vec::new();
    };
    let mut invisible: Vec<String> = Vec::new();
    for (node_id, node) in nodes {
        let Some(class_type) = node.get("class_type").and_then(Value::as_str) else {
            continue;
        };
        let Some(spec) = info.get(class_type) else {
            continue;
        };
        // 只查**加载器**节点：模型文件（checkpoint / LoRA / VAE / unet / clip）都在这一类上，
        // 而别的节点（采样器/图片输入/文件名前缀）的候选列表是动态的，拿它判会误报。
        if !class_type.to_ascii_lowercase().contains("loader") {
            continue;
        }
        let Some(required) = spec
            .get("input")
            .and_then(|v| v.get("required"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        for (name, value) in inputs {
            let Some(text) = value.as_str() else { continue };
            let allowed = required
                .get(name)
                .and_then(|v| v.get(0))
                .and_then(Value::as_array)
                .filter(|list| !list.is_empty() && list.iter().all(Value::is_string));
            let Some(allowed) = allowed else { continue };
            if !allowed
                .iter()
                .any(|candidate| candidate.as_str() == Some(text))
            {
                invisible.push(format!("{node_id}.{name} 要 {text}"));
            }
        }
    }
    if invisible.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!(
        "⚠ 这个实例的 /object_info 里看不到 {} 个模型文件 —— \
         硬提交可能会失败。",
        invisible.len()
    )];
    for item in &invisible {
        out.push(format!("  · {item}"));
    }
    out.push(
        "  → 大概率是「盘上有、实例看不到」，完整的模型清单要等 `comfy_env_report`/盘扫搬过来。"
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
) -> crate::comfy::comfy_models::Inventory {
    let project_root = state.project_root();
    let desktop = crate::tools::mcp::desktop(None);
    let model_dirs: Vec<std::path::PathBuf> = desktop
        .as_ref()
        .map(|d| d.models_dirs.clone())
        .unwrap_or_default();
    crate::comfy::comfy_models::inventory(object_info, Some(&project_root), &model_dirs)
}

/// 摘掉 `Anything Everywhere` 一族的广播桩。
///
/// API 图里这些节点是**零输出**的：`workflow_load` 的前端模拟已经把它们的
/// 消费者改写成直连。留着这个桩，审核会按"纯前端节点"把整张图拦下
/// （`krea2 t2i` 的 node 36，2026-11-08 真机），提交上去 ComfyUI 也会把它
/// 当未知类。只删这一类，不动显示节点与真节点。
fn strip_broadcast_stubs(graph: &Value) -> Value {
    let Some(nodes) = graph.as_object() else {
        return graph.clone();
    };
    let mut out = serde_json::Map::new();
    for (id, node) in nodes {
        let class = node
            .get("class_type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if crate::workflow::workflow_audit::is_broadcast(class) {
            continue;
        }
        out.insert(id.clone(), node.clone());
    }
    Value::Object(out)
}

/// 三道预检：返回（**阻断性**的拒因，**提示性**的警告，**这份图认不认负面提示词**）。
///
/// 图是**最终那张**（`overrides` / `connections` 都应用完之后），与 Kotlin 的位置一致。
/// `/object_info` **只问一次**：审核、两道实例侧检查、负面能力判定吃的是同一份响应 ——
/// 分两次请求的话，两个闸门可能对着两个不同的实例状态下结论。
async fn preflight(
    base: &str,
    graph: &Value,
    state: &AppState,
) -> (Option<String>, Vec<String>, Option<Vec<String>>) {
    let Some(nodes) = graph.as_object() else {
        return (None, Vec::new(), None);
    };
    let info = crate::comfy::comfy_client::get_json(&format!("{base}/object_info"))
        .await
        .ok();
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
        warnings.push(format!("⚠ 提交前的审核提示：\n{notes}"));
    }
    let negative = negative_capability(graph, object_info);
    (refusal, warnings, negative)
}

/// **这份工作流认不认负面提示词**（2026-10-11）。
///
/// 有些 turbo 模型的工作流根本没有负面输入，用户/模型发过去的负面提示词**无处可放** ——
/// 这件事只能从**目标工作流**上看出来（它是工作流的属性，不是预设的属性）。
///
/// 判据：按 `/object_info` 的声明，图里有没有**连了线**的 `negative` 条件输入。
/// * 只看"图里有没有 CLIPTextEncode"是错的 —— 每个工作流都有一个（正面那一个）；
/// * 只看"有没有 negative 输入声明"也不够 —— 没接线等于没用到；
/// * **拿不到 `/object_info` 就回 `None`**（不知道就别猜，与预检同一条纪律）。
///
/// 返回的是**接了线的节点类名**（去重排序）；`Some(vec![])` = 查过了、确实没有负面输入。
fn negative_capability(
    graph: &Value,
    object_info: Option<&Map<String, Value>>,
) -> Option<Vec<String>> {
    let info = object_info?;
    let nodes = graph.as_object()?;
    let mut hits: Vec<String> = Vec::new();
    for node in nodes.values() {
        let Some(node) = node.as_object() else {
            continue;
        };
        let class = node.get("class_type").and_then(Value::as_str).unwrap_or("");
        let Some(spec) = info.get(class).and_then(Value::as_object) else {
            continue;
        };
        let Some(inputs) = spec.get("input").and_then(Value::as_object) else {
            continue;
        };
        // `required` / `optional` 两处都要看（负面多半是 optional）
        let declares_negative = ["required", "optional"].iter().any(|slot| {
            inputs
                .get(*slot)
                .and_then(Value::as_object)
                .map(|m| m.contains_key("negative"))
                .unwrap_or(false)
        });
        if !declares_negative {
            continue;
        }
        // **接线才算用到**：值是 `[上游节点id, 槽位]` 那种数组
        let wired = node
            .get("inputs")
            .and_then(Value::as_object)
            .and_then(|m| m.get("negative"))
            .map(Value::is_array)
            .unwrap_or(false);
        if wired {
            hits.push(class.to_string());
        }
    }
    hits.sort();
    hits.dedup();
    Some(hits)
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
///
/// ⚠ `client_id` **必须**是 [`crate::comfy::comfy_progress::ComfyProgress::client_id`] 那个 ——
/// ComfyUI 的 `execution_*` / `progress` 帧只发给 `extra_data.client_id` 对应的那个 WS socket
/// （`execution.py:494`：`if server.client_id is not None: server.send_sync(…)`）。
/// 这里自己现造一个 UUID 的后果是"进度 WS 明明连着，却一帧都收不到"。
async fn post_prompt(base: &str, graph: &Value, client_id: &str) -> Result<String, String> {
    let body = json!({ "prompt": graph, "client_id": client_id });
    let root = crate::comfy::comfy_client::post_json(&format!("{base}/prompt"), &body).await?;
    root.get("prompt_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| id.to_string())
        .ok_or_else(|| {
            format!(
                "ComfyUI 没有返回 prompt_id：{}",
                take(&root.to_string(), 300)
            )
        })
}

fn take(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 查这一次运行在 `/history` 里的条目（还没有就 `None`）。
async fn history_entry(base: &str, prompt_id: &str) -> Option<Value> {
    let body = crate::comfy::comfy_client::get_json(&format!("{base}/history/{prompt_id}"))
        .await
        .ok()?;
    body.get(prompt_id).cloned().or_else(|| {
        // 有些实现直接回条目本身
        body.as_object()
            .filter(|map| map.contains_key("status"))
            .map(|_| body.clone())
    })
}

/// 队列里的一次运行在什么位置（给界面一句人话）。
async fn queue_note(base: &str, prompt_id: &str) -> Option<String> {
    let body = crate::comfy::comfy_client::get_json(&format!("{base}/queue"))
        .await
        .ok()?;
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
            if row
                .iter()
                .any(|cell| primitive_text(cell).as_deref() == Some(prompt_id))
            {
                return index as i64;
            }
        }
    }
    -1
}

/// 把一次**跑完了的**运行收进库（Kotlin `ComfyCapture.captureRun`）。
///
/// 与轮询走**同一条路**（[`crate::comfy::capture_ingest`]）：幂等靠 `prompt_id` + 文件 sha256。
/// 抢不到说明这一次已经被收过了（轮询先到）—— 那时**如实**回 `alreadyCaptured` 与已有的
/// 提示词 / 产物，绝不重复入库。
pub async fn capture_run(
    state: &AppState,
    config: &CaptureConfig,
    run_key: &str,
    entry: &Value,
    nsfw: bool,
    // 本次提交时给的标题（`comfy_submit(title=…)`）：**必须带进捕获**，否则
    // `make_title` 会退回保存节点的 `filename_prefix`（用户看到的是 `test · seed …`）。
    title: Option<&str>,
) -> Result<crate::comfy::capture_ingest::IngestResult, String> {
    let claim =
        crate::comfy::capture_ingest::begin_run(state, run_key, "ComfyUI", Some(entry), false)
            .map_err(|e| e.human().to_string())?;
    if !claim.claimed {
        let prompt_id = claim.existing_prompt_id;
        let media_ids = match prompt_id {
            Some(id) => state
                .with(|conn| {
                    let mut stmt = conn
                        .prepare("SELECT id FROM media_assets WHERE prompt_id = ?1 ORDER BY id")?;
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
        return Ok(crate::comfy::capture_ingest::IngestResult {
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
    let mut request =
        crate::comfy::capture_ingest::build_ingest_request(run_key, entry, config, is_error);
    request.nsfw = nsfw;
    if let Some(title) = title.map(str::trim).filter(|text| !text.is_empty()) {
        request.title = Some(title.to_string());
    }
    Ok(crate::comfy::capture_ingest::ingest_claimed(state, &request, config).await)
}

/// 工具入口：`comfy_submit`。
pub async fn tool_submit(
    state: &AppState,
    policy: &crate::tools::policy::ToolPolicy,
    arguments: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let overrides = arguments
        .get("overrides")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let connections = arguments
        .get("connections")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    // 最终正面提示词：给了就**由模型自己写完**（通常已经服从了工作流里 TextGenerate 的
    // system prompt），后端只负责把它写进正面编码器并摘掉原来那一路。
    let positive_prompt = match arguments.get("positivePrompt") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Some(_) => {
            return Err(ToolFailure::new(
                "INVALID_ARGUMENT",
                "positivePrompt 要是一个非空字符串（最终提示词本体）；\
                 不要传用户原话让本地 TextGenerate 去扩写",
            ))
        }
    };
    // 技能名（可选）：服务端按名读 skill 正文，写进 `TextGenerate.system_prompt` 的源节点。
    // 模型只报名字 —— 18KB 的正文让它复述既贵又容易抄错。
    let text_generate_skill = arguments
        .get("textGenerateSkill")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let wait = arguments
        .get("wait")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let wait_seconds = arguments
        .get("waitSeconds")
        .and_then(arg_i64)
        .unwrap_or(DEFAULT_WAIT_SECONDS)
        .clamp(0, MAX_WAIT_SECONDS);
    let nsfw = arguments
        .get("nsfw")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let title = arguments
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string());

    // 来源档（T3，2026-11-09）：**缺省不落** —— 只有显式声明才写
    // `capture_runs.source_kind`；外部在 ComfyUI 里跑的保持列默认 `'ComfyUI'`。
    let source_kind = match arguments.get("sourceKind") {
        None | Some(Value::Null) => None,
        Some(Value::String(raw)) => {
            let kind = raw.trim();
            if !matches!(kind, "user" | "recipe" | "recent" | "template") {
                return Err(ToolFailure::new(
                    "INVALID_ARGUMENT",
                    "sourceKind 只能是 user / recipe / recent / template 之一",
                ));
            }
            Some(kind.to_string())
        }
        Some(_) => {
            return Err(ToolFailure::new(
                "INVALID_ARGUMENT",
                "sourceKind 要是一个字符串：user / recipe / recent / template",
            ))
        }
    };
    let recipe_id = arguments.get("recipeId").and_then(arg_i64);

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
        let loaded = crate::workflow::workflow_load::load_from_file(
            state,
            &file,
            title.as_deref(),
            false,
            true,
            true,
        )
        .await
        .map_err(|failure| ToolFailure::new(&failure.code, failure.message))?;
        prompt_id = loaded.get("promptId").and_then(arg_i64);
        if prompt_id.is_none() {
            return Err(ToolFailure::new(
                "COMFY_LOAD_FAILED",
                "加载了工作流文件，但没拿到 promptId",
            ));
        }
    }
    let prompt_id = prompt_id.unwrap_or_default();

    // 配方 id 尽量由后端按 promptId 推（对得上就填）；调用方显式给的优先。
    let recipe_id = recipe_id.or_else(|| {
        state
            .with(|conn| {
                Ok(conn
                    .query_row(
                        "SELECT id FROM workflow_recipes WHERE prompt_id = ?1 \
                         ORDER BY id DESC LIMIT 1",
                        rusqlite::params![prompt_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .ok())
            })
            .ok()
            .flatten()
    });

    // R10：只认 API 格式节点图（取不到就如实报，绝不猜着造一张）
    let graph = crate::comfy::capture::prompt_api_graph(state, prompt_id)
        .map_err(|e| ToolFailure::new("TOOL_ERROR", format!("读取节点图出错：{}", e.human())))?
        .ok_or_else(|| {
            let title = prompt_title(state, prompt_id).unwrap_or_else(|| format!("id={prompt_id}"));
            ToolFailure::new(
                "NO_API_GRAPH",
                format!(
                    "这条提示词（{title}）没有 API 格式节点图，不能直接提交。\
                     可以换一条能跑的，或者先在 ComfyUI 里手动跑一次让它被捕获。"
                ),
            )
        })?;

    // 先按名把 skill 正文写进 TextGenerate.system_prompt（只改这一次提交的图）
    let (graph, system_prompt_targets) = match text_generate_skill.as_deref() {
        Some(name) => {
            let (builtin, user) = crate::ai::skills::roots_of(state);
            let skill = crate::ai::skills::read_body(&builtin, &user, name).ok_or_else(|| {
                ToolFailure::new(
                    "INVALID_ARGUMENT",
                    format!("找不到 skill `{name}`——先 list_skills 看装了什么，不要把正文猜出来"),
                )
            })?;
            if !skill.valid {
                return Err(ToolFailure::new(
                    "INVALID_ARGUMENT",
                    format!("skill `{name}` 的 frontmatter 不合法（界面上有校验错误），不能加载"),
                ));
            }
            let (graph, targets) = apply_text_generate_system_prompt(&graph, &skill.body)
                .map_err(|message| ToolFailure::new("INVALID_ARGUMENT", message))?;
            (graph, targets)
        }
        None => (graph, Vec::new()),
    };

    // 先摘 TextGenerate / 写正面提示词，再应用 overrides：显式 overrides 仍然最后说了算。
    let (graph, positive_targets, text_generate_bypassed) = match positive_prompt.as_deref() {
        Some(prompt) => WorkflowEdit::apply_positive_prompt(&graph, prompt)
            .map_err(|message| ToolFailure::new("INVALID_ARGUMENT", message))?,
        None => (graph, Vec::new(), Vec::new()),
    };
    let (overridden, mut applied) = WorkflowEdit::apply_overrides(&graph, &overrides)
        .map_err(|message| ToolFailure::new("INVALID_ARGUMENT", message))?;
    let (patched, applied_connections) = WorkflowEdit::apply_connections(&overridden, &connections)
        .map_err(|message| ToolFailure::new("INVALID_ARGUMENT", message))?;
    applied.extend(applied_connections);

    // Anything Everywhere 一族的广播桩在服务端是零输出；`workflow_load` 的前端
    // 模拟已经把消费者改写成直连，留着这个桩只会让审核按"纯前端节点"误拦，
    // 提交上去也会被 ComfyUI 当成未知类（2026-11-08 真机 krea2）。
    let patched = strip_broadcast_stubs(&patched);

    // 这次提示词是谁写的：caller（模型自己写完传 positivePrompt）/ text-generate（本地生成）
    // / literal（图里写死的文本）。**写进结果**，工具卡上就能看见 —— 静默失效才查得出来。
    let prompt_source = if positive_prompt.is_some() {
        "caller"
    } else if !system_prompt_targets.is_empty() || positive_prompt_is_generated(&patched) {
        "text-generate"
    } else {
        "literal"
    };

    let config = crate::comfy::capture::capture_config(state)
        .map_err(|e| ToolFailure::new("TOOL_ERROR", format!("读捕获配置出错：{}", e.human())))?;
    let base = config.comfy_url.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return Err(ToolFailure::new("COMFY_DISABLED", "没有配置 ComfyUI 地址"));
    }

    // 预检放在这里（overrides / connections **都应用完之后**）：检查的就是真正要提交的那张图。
    // 审核里"必跑不了"的三项 + 节点类那一档**拦**；模型文件与审核里非阻断的那些只**警告**
    // （见 `preflight` / `preflight_refusal` 的注释）。
    let (refusal, warnings, negative) = preflight(&base, &patched, state).await;
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

    let run_key = post_prompt(&base, &patched, state.comfy_progress.client_id())
        .await
        .map_err(|message| {
            ToolFailure::new("COMFY_SUBMIT_FAILED", format!("提交失败：{message}。"))
        })?;

    // 提交时手里就有 API 图 → 把 `节点 id → 可读名`（`_meta.title` / `class_type`）记下来。
    // 进度 WS 的 `executing` / `progress` 帧只带节点 id，靠这份映射才能翻成
    // "正在执行 KSampler"；**没有映射时如实只给节点 id，不编名字**。
    state.comfy_progress.register_graph(&run_key, &patched);

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
            let status_str = status_object
                .and_then(|s| s.get("status_str"))
                .and_then(Value::as_str);
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
            match capture_run(state, &config, &run_key, &entry, nsfw, title.as_deref()).await {
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
                    // 这一跑**真的跑通了**吗？判据是"没报错 + 捕获那条路认了这次结果"——
                    // 产物与已有文件重复（duplicates）也算跑通：那次运行确实出图了，
                    // 只是文件早就在库里。**"完成但没有产物文件"不算**（没东西证明它跑出了什么）。
                    let ran_ok = !failed
                        && (result.imported > 0 || result.duplicates > 0 || result.already_captured);
                    // T3：把这次提交的来源档 / 配方记在 capture_runs 上（外部捕获那条路不动）。
                    // `COALESCE` 保证"缺省不落"：没声明的列保持原值（默认 'ComfyUI'）。
                    if source_kind.is_some() || recipe_id.is_some() {
                        if let Err(error) = state.write(|tx| {
                            tx.execute(
                                "UPDATE capture_runs SET source_kind = COALESCE(?1, source_kind), \
                                 recipe_id = COALESCE(?2, recipe_id) WHERE run_key = ?3",
                                rusqlite::params![&source_kind, recipe_id, run_key],
                            )?;
                            Ok(())
                        }) {
                            eprintln!("[comfy_submit] provenance 写入失败：{error:?}");
                        }
                    }
                    // T2：跑通了就把"这条配方 × 这组槽位覆盖"记成已验证（失败 / 超时 / 取消不记）。
                    if ran_ok {
                        record_verified_binding(state, recipe_id, &overrides, &run_key);
                    }
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
                "等待超过 {wait_seconds} 秒，任务可能还在 ComfyUI 里跑，\
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
            "查这次运行用 comfy_get_run(runKey=\"{}\")；注意capturedPromptId 是捕获记录编号（画廊 / 提示词用的那个是 promptId）",
            submission.prompt_id
        )),
    );
    out.insert("appliedOverrides".into(), json!(applied));
    out.insert("promptSource".into(), json!(prompt_source));
    // T3：这一跑是哪一档挑出来的（缺省不落 = 不出现这个键）。
    if let Some(kind) = &source_kind {
        out.insert("sourceKind".into(), json!(kind));
    }
    if let Some(id) = recipe_id {
        out.insert("recipeId".into(), json!(id));
    }
    if !system_prompt_targets.is_empty() {
        out.insert("systemPromptApplied".into(), json!(system_prompt_targets));
    }
    if !positive_targets.is_empty() {
        out.insert("positivePromptApplied".into(), json!(positive_targets));
        out.insert("textGenerateBypassed".into(), json!(text_generate_bypassed));
        out.insert(
            "positivePromptHint".into(),
            json!(
                "已经把最终提示词写进正面条件编码；TextGenerate 那一路（含只喂它的上游、\
                 指向它的显示节点）已经从**这次提交的图**里摘掉 —— ComfyUI 既不会校验也不会\
                 执行它（只断线不够：校验看的是整张 prompt）。用户的原文件没有被改。"
            ),
        );
    }
    out.insert("mediaIds".into(), json!(submission.media_ids));
    out.insert("mediaCount".into(), json!(submission.media_ids.len()));
    out.insert("nsfw".into(), json!(nsfw));
    // **这份工作流认不认负面提示词**（2026-10-11）：有些 turbo 模型没有负面输入。
    //
    // 为什么要把这个事实交给模型：它是**工作流的属性**，不是预设的属性 ——
    // 预设那边只有"我只发正面"这个意图（`negativeMode`）。两件事分开之后，
    // 模型在"用这份预设跑"的时候才能自己判断：负面发不出去就别在正文里吹"已按负面约束"。
    //
    // 判据：图里有没有**连着线**的 `negative` 条件输入（按 `/object_info` 的声明算），
    // 而不是"图里有没有 CLIPTextEncode" —— 后者每个工作流都有（正面那一个）。
    if let Some(cap) = negative {
        out.insert("negativeInputs".into(), json!(cap));
    }
    // 预检的**提示**（不拦）：让模型/用户知道"实例可能看不到这几个模型文件"
    if !warnings.is_empty() {
        out.insert("preflightWarnings".into(), json!(warnings));
    }
    out.insert(
        "nsfwMediaIds".into(),
        if nsfw {
            json!(submission.media_ids)
        } else {
            json!(Vec::<i64>::new())
        },
    );
    if submission.status == "timeout" {
        out.insert(
            "hint".into(),
            json!(format!(
                "还没跑完，可以隔一会儿用 comfy_get_run runKey={} 查",
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

/// 把一段 skill 正文写进工作流里 `TextGenerate.system_prompt` 指向的那个字符串节点。
///
/// 为什么放在服务端：18KB 的 r2i skill 正文让模型在 `overrides` 里复述一遍既贵、又容易抄错
/// （2026-10-25 的口径：模型只说 skill 名，正文由后端按名读出来写进去）。
/// 只改**这一次提交的图**，用户的原文件一个字节都不动。
///
/// 能写两种形状：`system_prompt` 直接是内联字符串；或者它连着某个 `*String*` 节点（改它的 `value`）。
pub fn apply_text_generate_system_prompt(
    graph: &Value,
    body: &str,
) -> Result<(Value, Vec<String>), String> {
    let mut nodes: Map<String, Value> = graph
        .as_object()
        .cloned()
        .ok_or_else(|| "工作流不是 API 格式节点图（顶层要是对象）".to_string())?;
    let body = body.trim();
    if body.is_empty() {
        return Err("textGenerateSkill 指向的 skill 正文是空的".into());
    }
    let mut targets: Vec<String> = Vec::new();
    let ids: Vec<String> = nodes.keys().cloned().collect();
    for id in ids {
        let Some(node) = nodes.get(&id).and_then(Value::as_object) else {
            continue;
        };
        let class = node.get("class_type").and_then(Value::as_str).unwrap_or("");
        if !class.to_ascii_lowercase().contains("textgenerate") {
            continue;
        }
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        let Some(system_prompt) = inputs.get("system_prompt") else {
            continue;
        };
        if system_prompt.is_string() {
            if let Some(node) = nodes.get_mut(&id).and_then(Value::as_object_mut) {
                if let Some(inputs) = node.get_mut("inputs").and_then(Value::as_object_mut) {
                    inputs.insert("system_prompt".into(), Value::String(body.to_string()));
                    targets.push(format!("{id}.system_prompt"));
                }
            }
            continue;
        }
        let Some((upstream, _)) = link_ref(system_prompt) else {
            continue;
        };
        let Some(upstream_node) = nodes.get_mut(&upstream).and_then(Value::as_object_mut) else {
            continue;
        };
        let is_string_node = upstream_node
            .get("class_type")
            .and_then(Value::as_str)
            .map(|c| c.to_ascii_lowercase().contains("string"))
            .unwrap_or(false);
        if !is_string_node {
            continue;
        }
        if let Some(inputs) = upstream_node
            .get_mut("inputs")
            .and_then(Value::as_object_mut)
        {
            inputs.insert("value".into(), Value::String(body.to_string()));
            targets.push(format!("{upstream}.value"));
        }
    }
    if targets.is_empty() {
        return Err(
            "这份工作流里找不到可写的 `TextGenerate.system_prompt`（没有 TextGenerate，\
             或者它接的不是字符串节点）；确认它是不是布局 / 扩写节点"
                .into(),
        );
    }
    Ok((Value::Object(nodes), targets))
}

/// 这张图的正面条件是不是**由本地 TextGenerate 生成**的（而不是图里写死的文本）。
///
/// 用途只有一个：`comfy_submit` 结果里的 `promptSource`。没有它，"模型以为走了生成支路、
/// 其实 switch 还指着纯文本拼接"这种静默失效就查不出来。
pub fn positive_prompt_is_generated(graph: &Value) -> bool {
    let Some(nodes) = graph.as_object() else {
        return false;
    };
    let mut starts: Vec<String> = Vec::new();
    for node in nodes.values() {
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        if let Some((up, _)) = inputs.get("positive").and_then(link_ref) {
            starts.push(up);
        }
        let class = node.get("class_type").and_then(Value::as_str).unwrap_or("");
        if class.to_ascii_lowercase().contains("guider") {
            if let Some((up, _)) = inputs.get("conditioning").and_then(link_ref) {
                starts.push(up);
            }
        }
    }
    let mut queue: VecDeque<(String, usize)> = starts.into_iter().map(|id| (id, 0usize)).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut encoders: Vec<String> = Vec::new();
    while let Some((id, depth)) = queue.pop_front() {
        if depth > 32 || !seen.insert(id.clone()) {
            continue;
        }
        let Some(node) = nodes.get(&id).and_then(Value::as_object) else {
            continue;
        };
        let class = node
            .get("class_type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        if class.contains("textencode") {
            encoders.push(id);
            continue;
        }
        if let Some(inputs) = node.get("inputs").and_then(Value::as_object) {
            for value in inputs.values() {
                if let Some((up, _)) = link_ref(value) {
                    queue.push_back((up, depth + 1));
                }
            }
        }
    }
    // 从每个编码器的 text 连线往上找一层（switch / concat / TextGenerate 都在这条链上）
    for encoder in encoders {
        let Some(node) = nodes.get(&encoder).and_then(Value::as_object) else {
            continue;
        };
        let Some(text) = node
            .get("inputs")
            .and_then(Value::as_object)
            .and_then(|i| i.get("text"))
        else {
            continue;
        };
        let mut frontier: Vec<String> = link_ref(text).map(|(up, _)| vec![up]).unwrap_or_default();
        let mut hop = 0;
        while !frontier.is_empty() && hop < 8 {
            let mut next: Vec<String> = Vec::new();
            for id in frontier {
                let Some(node) = nodes.get(&id).and_then(Value::as_object) else {
                    continue;
                };
                let class = node
                    .get("class_type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if class.contains("textgenerate") {
                    return true;
                }
                if let Some(inputs) = node.get("inputs").and_then(Value::as_object) {
                    for value in inputs.values() {
                        if let Some((up, _)) = link_ref(value) {
                            next.push(up);
                        }
                    }
                }
            }
            frontier = next;
            hop += 1;
        }
    }
    false
}

/// 库里那条提示词的标题（找不到就 `None`）。
fn prompt_title(state: &AppState, id: i64) -> Option<String> {
    state
        .with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT title FROM prompts WHERE id = ?1",
                    rusqlite::params![id],
                    |row| row.get::<_, Option<String>>(0),
                )
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
mod tests;
