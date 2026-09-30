//! `comfy_check_inputs` + `comfy_fit_workflow` 两件"体检工具"
//! （`WorkflowInputs.kt` + `WorkflowFit.kt` 与 `ToolRegistry.kt` 里那两条处理体的逐条对照）。
//!
//! # 两件工具解决的是同一类现场：**模型不看清楚就硬提交**
//!
//!  - [`tool_check_inputs`]（用户建议 ⑤，2026-09-20）：用户的原话是
//!    "用户没有提供参考图，但是他引导的工作流的节点用到参考图"。图里那个 `LoadImage`
//!    要么绑着上次捕获时的某个 jpg（跟本次请求无关的文件），要么压根没连 ——
//!    模型不看清楚就提交，ComfyUI 只回一句缺输入，**用户看到的是"AI 说生成了、其实没跑起来"**。
//!  - [`tool_fit_workflow`]（用户建议 ①，2026-09-21）：即"可以使用 civitai 等网站查找符合
//!    要求的工作流，**优先根据本机已有的模型修改该工作流**"。中间缺的那一步是
//!    "图里点名的这个 `.safetensors`，本机上哪个文件能顶"；现场（`b5233fb4` seq6）里
//!    模型只能手读 safetensors 头部去判断"那个 13.49 GB 的 checkpoint 是裸 DiT"。
//!
//! # 两条纪律
//!
//! 1. **只报不改**：两件工具都不生成图、不写文件、不提交。`comfy_fit_workflow` 吐出来的是一份
//!    报告 + 一份可以直接贴进 `comfy_submit(overrides=…)` 的建议 —— 要真生效必须由模型
//!    明明白白写进调用参数（还会过审批闸门），"AI 悄悄把用户的模型换掉了"因此不可能发生。
//! 2. **不猜也不编**：认不出来的一律如实说（`kind=unknown` 走
//!    [`InputKind::Parameter`]、模型配不上就报 `missing`），
//!    **绝不**为了好看硬塞一个"最接近的"上去（配对判据见 [`crate::comfy::comfy_models`]）。
//!
//! # 与 Kotlin 的一处形状细节
//!
//! Kotlin 是用 `buildJsonObject { put("k", v) }` **手拼** JSON 的，而
//! `JsonObjectBuilder.put(key, String?)` 遇到 null 会写 **`JsonNull`**（`explicitNulls=false`
//! 只管 `@Serializable` 的 DTO，管不到手拼的对象）。所以凡是 Kotlin 那边
//! `m.title?.let { put(...) }` 这种"有才写"的键，这里也**照它的写法省掉键**；
//! 而 `<report>.toJson()` 的顶层结构（含 `put("missingCount", …)` 这种恒定键）一个都不能少 ——
//! 少一个键与多一个 null 键，模型看到的形状不一样，而"形状逐条对照"是这个仓库的纪律。
//!
//! # 一处**如实承认的不一致**
//!
//! Kotlin 的 `comfy_fit_workflow` 处理体开头有一道
//! `comfyModels ?: throw COMFY_DISABLED`（本机模型清单能力没启用时拒掉）——
//! 那个能力在 Kotlin 里是**可注入的**，而 Rust 这边（与 `comfy_env_report` 一样）
//! 直接调 [`crate::comfy::comfy_models::inventory`]，没有"没启用"这一档，所以这条分支**没有对应物**。
//! 它不影响任何用户可见行为：两种实现里清单都拿得到。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::comfy::comfy_models::{Inventory, ModelName, Requirement, Scored, Source};
use crate::web::routes::AppState;
use crate::tools::tools::{ToolFailure, ToolOutcome};

// ---------------------------------------------------------------------------
//  待补输入诊断（`WorkflowInputs.kt`）
// ---------------------------------------------------------------------------

/// 连线型输入里，**必须由外部提供**的那些名字（判据取位置语义，不取节点名）。
///
/// `LoadImage`、`IPAdapter` 的 `image`、`VHS_LoadVideo` 的 `video`、蒙版类都落在这里。
/// 没被认出来的（第三方自定义节点）照样列进清单，**不静默丢弃** ——
/// 列表少一条，模型就会以为那份工作流已经齐了。
const IMAGE_INPUTS: &[&str] = &["image", "images", "mask", "masks", "image1", "image2", "image_b"];
const VIDEO_INPUTS: &[&str] = &["video", "videos", "frames", "frame_load_cap"];

/// 没有真实语义的虚拟节点：它们不该进"待补输入"清单。
const VIRTUAL_NODES: &[&str] =
    &["Note", "MarkdownNote", "Reroute", "PrimitiveNode", "GetNode", "SetNode"];

/// 待补输入的种类 —— 决定"下一步该干什么"，所以单独列出来（`InputKind.wire` 逐字对照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// 等一张图（附件投放 / 问用户要）
    Image,
    /// 等一段视频（本机没有投放通道）
    Video,
    /// 连线型输入空着（照 openInputs 补线，别猜）
    Connection,
    /// 控件型输入没值（覆盖参数或问用户）
    Parameter,
}

impl InputKind {
    pub fn wire(self) -> &'static str {
        match self {
            InputKind::Image => "image",
            InputKind::Video => "video",
            InputKind::Connection => "wire",
            InputKind::Parameter => "value",
        }
    }
}

/// 一条待补输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingInput {
    pub node_id: String,
    pub class_type: String,
    pub input_name: String,
    pub kind: InputKind,
    pub title: Option<String>,
    pub current_value: Option<String>,
    pub hint: String,
}

/// 一次扫描的结果（`WorkflowInputReport`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowInputReport {
    pub missing: Vec<MissingInput>,
    /// 本次附件就能解决的位置（**不列进"要用户补"**）。
    pub supplied_by_attachment: Vec<String>,
    pub node_count: usize,
}

impl WorkflowInputReport {
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty()
    }

    /// 有没有"必须用户出手"的（图 / 视频）：这两类硬提交必然失败。
    pub fn has_external_asset_gap(&self) -> bool {
        self.missing
            .iter()
            .any(|m| m.kind == InputKind::Image || m.kind == InputKind::Video)
    }

    pub fn to_json(&self) -> Value {
        let mut missing: Vec<Value> = Vec::with_capacity(self.missing.len());
        for m in &self.missing {
            let mut o = Map::new();
            o.insert("nodeId".into(), json!(m.node_id));
            o.insert("classType".into(), json!(m.class_type));
            o.insert("inputName".into(), json!(m.input_name));
            o.insert("kind".into(), json!(m.kind.wire()));
            // `m.title?.let { put(...) }`：Kotlin 手拼的是**省掉键**（不是写 null）
            if let Some(t) = &m.title {
                o.insert("title".into(), json!(t));
            }
            if let Some(v) = &m.current_value {
                o.insert("currentValue".into(), json!(v));
            }
            o.insert("hint".into(), json!(m.hint));
            o.insert("ref".into(), json!(format!("{}.{}", m.node_id, m.input_name)));
            missing.push(Value::Object(o));
        }
        let mut out = Map::new();
        out.insert("nodeCount".into(), json!(self.node_count));
        out.insert("missingCount".into(), json!(self.missing.len()));
        out.insert("needsExternalAsset".into(), json!(self.has_external_asset_gap()));
        out.insert("missing".into(), Value::Array(missing));
        if !self.supplied_by_attachment.is_empty() {
            out.insert(
                "suppliedByAttachment".into(),
                Value::Array(
                    self.supplied_by_attachment.iter().map(|s| json!(s)).collect::<Vec<_>>(),
                ),
            );
        }
        Value::Object(out)
    }

    /// 给模型看的一小段：**每条一行**，带头部结论。
    ///
    /// 结论先行（"这份工作流还等着外部给输入"）是有意的：模型最容易漏掉的正是这一句，
    /// 而它决定了后面该问用户还是该直接提交。
    pub fn render(&self) -> String {
        let mut text = String::new();
        if self.missing.is_empty() {
            text.push_str(&format!(
                "这份工作流没有待补输入（{} 个节点，参数都有值或都已连线）—— 可以直接提交。",
                self.node_count
            ));
            // 全部缺口都由本次附件解决时也要说一句：否则模型会以为"什么都没有"，
            // 转头又把 LoadImage 那个绑死的旧文件名当成能用的东西
            append_attachment_note(&mut text, &self.supplied_by_attachment);
            return text;
        }
        let asset = if self.has_external_asset_gap() {
            "其中**有图 / 视频类的缺口**：不解决就会提交失败。"
        } else {
            ""
        };
        text.push_str(&format!("这份工作流有 {} 处**等着外部输入**：", self.missing.len()));
        if !asset.is_empty() {
            text.push_str(asset);
        }
        text.push('\n');
        for m in &self.missing {
            text.push_str(&format!("· {}.{}", m.node_id, m.input_name));
            text.push_str(&format!("（{}", m.class_type));
            if let Some(t) = &m.title {
                text.push_str(&format!(" / {t}"));
            }
            text.push_str(&format!("，缺 {}", m.kind.wire()));
            if let Some(v) = &m.current_value {
                text.push_str(&format!("，现在：{v}"));
            }
            text.push_str(&format!("）\n  {}\n", m.hint));
        }
        append_attachment_note(&mut text, &self.supplied_by_attachment);
        text
    }
}

/// 附件能解决的那些位置（有才写）。
fn append_attachment_note(sb: &mut String, supplied: &[String]) {
    if supplied.is_empty() {
        return;
    }
    sb.push_str("（以下位置本次附件可以解决，不必问用户：");
    sb.push_str(&supplied.join("、"));
    sb.push_str("）\n");
}

/// 扫一份 **API 格式**节点图（`{"1": {"class_type": "...", "inputs": {...}}, ...}`）。
///
/// `has_image_attachment` = 本次请求是否带了图片附件。带了的话，图片类缺口不算"缺"——
/// 模型手里就有图，下一步是 `comfy_use_attachment` 投放，不该被报成"要用户补"。
pub fn scan(graph: &Map<String, Value>, has_image_attachment: bool) -> WorkflowInputReport {
    let mut missing: Vec<MissingInput> = Vec::new();
    let mut known: Vec<String> = Vec::new();

    for (node_id, raw) in graph {
        let Some(node) = raw.as_object() else { continue };
        let class_type = node.get("class_type").and_then(Value::as_str).unwrap_or("");
        if class_type.trim().is_empty() {
            continue;
        }
        if VIRTUAL_NODES.contains(&class_type) {
            continue;
        }
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else { continue };
        let title = node
            .get("_meta")
            .and_then(Value::as_object)
            .and_then(|m| m.get("title"))
            .and_then(Value::as_str);

        for (name, value) in inputs {
            let Some(gap) = gap_of(class_type, name, value) else { continue };
            if gap == InputKind::Image && has_image_attachment {
                // 本次带了图：图片缺口由 `comfy_use_attachment` 投放解决，
                // 记成"已知道"而不是"要用户补" —— 两条混在一起模型就会去问用户要图。
                known.push(format!("{node_id}.{name}（本次附件可以投放）"));
                continue;
            }
            missing.push(MissingInput {
                node_id: node_id.clone(),
                class_type: class_type.to_string(),
                input_name: name.clone(),
                kind: gap,
                title: title.map(str::to_string),
                current_value: describe_value(value),
                hint: hint_for(gap).to_string(),
            });
        }
    }

    WorkflowInputReport {
        missing,
        supplied_by_attachment: known,
        node_count: graph.len(),
    }
}

/// 这个输入算不算"还等着外部给"。
///
/// 返回 `None` = 不用管（有真实值、或是输出节点/模型可控的东西）。
fn gap_of(class_type: &str, name: &str, value: &Value) -> Option<InputKind> {
    // 输出节点（SaveImage / PreviewImage…）是"结果去哪儿"，不是"等谁给输入"
    if class_type.contains("Save") || class_type.contains("Preview") {
        return None;
    }

    let is_wire = matches!(value, Value::Array(_));
    let is_empty = matches!(value, Value::Null)
        || matches!(value, Value::String(s) if s.trim().is_empty());

    // LoadImage 的 image：就算绑着上次捕获的那个 jpg 也是一条"你要自己决定用哪张"的缺口。
    // 这是用户建议 ⑤ 那个现场的核心一条 —— 不报出来，模型就会硬提交一个不存在的文件。
    if class_type == "LoadImage" && name == "image" {
        return Some(InputKind::Image);
    }
    if class_type.starts_with("VHS_LoadVideo") && VIDEO_INPUTS.contains(&name) {
        return Some(InputKind::Video);
    }

    if !is_wire && !is_empty {
        return None;
    }

    // 连线型输入空着，或者值缺失：看名字判它等的是什么
    let lower = name.to_lowercase();
    if IMAGE_INPUTS.contains(&lower.as_str()) {
        Some(InputKind::Image)
    } else if VIDEO_INPUTS.contains(&lower.as_str()) {
        Some(InputKind::Video)
    } else if is_wire {
        Some(InputKind::Connection)
    } else {
        Some(InputKind::Parameter)
    }
}

fn hint_for(kind: InputKind) -> &'static str {
    match kind {
        InputKind::Image => {
            "缺一张图。用户给了附件就走 comfy_use_attachment 投放拿 filename，\
             再覆盖这个输入；用户**没给图**时不要硬提交（ComfyUI 只会报缺输入）——\
             问用户要一张，或换一条不需要参考图的工作流。"
        }
        InputKind::Video => {
            "缺一段视频。本机没有「投放视频」的工具：必须让用户自己在 ComfyUI 里选好，或换工作流。"
        }
        InputKind::Connection => {
            "这个输入是**连线**，现在空着。要么照 openInputs 补线\
             （comfy_submit 的 connections），要么就是这份图还没转完 —— 不要猜着接一根线上去。"
        }
        InputKind::Parameter => {
            "这个输入没有值。多数是控件型输入：覆盖成用户要的参数即可，不确定就问用户。"
        }
    }
}

/// `value` 的如实描述（`describeValue`）：空 / null 一律 `None`（Kotlin 那边省掉键）。
fn describe_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Array(items) => {
            if items.is_empty() {
                Some("空连线".to_string())
            } else {
                Some(format!(
                    "连线 → {}",
                    items.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")
                ))
            }
        }
        Value::String(s) if s.trim().is_empty() => Some("空字符串".to_string()),
        other => Some(take_chars(&other.to_string(), 120)),
    }
}

/// 按**字符**截断（Kotlin 的 `take(n)` 数的是 UTF-16 单元，对 BMP 里的中文与 ASCII 等价）。
fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 从工具结果里挑出"API 节点图"那一段（`apiGraphOf`）。
///
/// 三种来源共用（`comfy_get_run.raw`、`comfy_load_workflow.apiGraph`、`comfy_find_workflow`
/// 的结果），所以判据只有一条：**是个对象、且每个值都是带 `class_type` 的对象**。
/// 认不出来就返回 `None` —— 宁可说"给的那段不是节点图"，也不要对着一份界面格式硬扫。
///
/// 图既可以直接给（schema 声明的是 object），也可以是字符串形态的 JSON ——
/// 模型从别处复制过来时经常带上引号，两种都认。
pub fn api_graph_of(element: Option<&Value>) -> Option<Map<String, Value>> {
    let obj: Map<String, Value> = match element? {
        Value::Object(o) => o.clone(),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Object(o)) => o,
            _ => return None,
        },
        _ => return None,
    };
    if obj.is_empty() {
        return None;
    }
    let looks_like_graph = obj.values().all(|v| {
        v.as_object()
            .and_then(|n| n.get("class_type"))
            .map(is_json_primitive)
            .unwrap_or(false)
    });
    if looks_like_graph {
        Some(obj)
    } else {
        None
    }
}

/// Kotlin 的 `is JsonPrimitive`（字符串 / 数字 / 布尔都算，`JsonNull` 不算）。
fn is_json_primitive(value: &Value) -> bool {
    matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_))
}

/// `comfy_check_inputs`：把"还等着外部输入的位置"逐个列清楚（**只读**）。
///
/// ⚠ 签名与另两条工具不同：Kotlin 的这条处理体只用到 `args` 与 `ctx.hasImageAttachment`，
/// **不碰 `state`**（它不读库、不问 ComfyUI）。所以这里也不硬塞一个用不上的 `AppState`
/// —— 那个值由调用方从 [`crate::tools::tools::ToolSession::has_image_attachment`] 传进来
/// （与 Kotlin `ToolContext.hasImageAttachment` 同一个来源）。
pub fn tool_check_inputs(
    arguments: &Value,
    has_image_attachment: bool,
) -> Result<ToolOutcome, ToolFailure> {
    let graph = api_graph_of(arguments.get("graph")).ok_or_else(|| {
        ToolFailure::new(
            "INVALID_ARGUMENT",
            "graph 不是一份 API 格式节点图（应当形如 \
             {\"1\":{\"class_type\":\"KSampler\",\"inputs\":{…}}}）。\
             界面格式（nodes/links）请先用 comfy_load_workflow 读进库。",
        )
    })?;
    let report = scan(&graph, has_image_attachment);
    Ok(ToolOutcome::new(report.render(), report.to_json()))
}

// ---------------------------------------------------------------------------
//  "本机有没有这份工作流要的模型"（`WorkflowFit.kt`）
// ---------------------------------------------------------------------------

/// 配对结论（四档，见 [`FitReport`] 的注释）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitStatus {
    /// 本机有一模一样的（归一化后同名）**且当前实例能加载** —— 唯一能自动替换的一档。
    Exact,
    /// 本机有更接近的（分数 ≥ 阈值）：候选给出来，用不用由模型 / 用户定。
    Substitute,
    /// **硬盘上有同名的，但当前跑着的那个实例枚举不到它**（2026-09-22 新加，那次事故的直接产物）。
    ///
    /// 机制：`launch_comfyui` 直接跑 `main.py`，ComfyUI Desktop 那套"模型在共享目录 / 别的盘"
    /// 的配置一个都没传 → `/object_info` 里没有这些文件 → 只报"本机没有"会让模型
    /// **让用户去下载一个本来就在硬盘上的文件**（现场：`anima-base-v1.0` 4.18 GB）。
    /// 这一档的正确动作是**修挂载 / 重启**，不是下载。
    OnDiskOnly,
    /// 本机没有像样的替代品：该去装 / 该让用户选，**不许编名字**。
    Missing,
}

impl FitStatus {
    pub fn wire(self) -> &'static str {
        match self {
            FitStatus::Exact => "exact",
            FitStatus::Substitute => "substitute",
            FitStatus::OnDiskOnly => "onDiskOnly",
            FitStatus::Missing => "missing",
        }
    }
}

/// 图里点名的一个模型 vs 本机清单的配对结果。
#[derive(Debug, Clone)]
pub struct FitItem {
    pub requirement: Requirement,
    pub status: FitStatus,
    pub candidates: Vec<Scored>,
    /// 本机那几个"名字沾点边、但不够像"的文件（分数低于阈值的）。
    ///
    /// 只在 [`FitStatus::Missing`] 时说给用户听：他看到"要装它"时，盘上那个像的名字
    /// 很容易让他以为已经有了（用户建议 ② 要的"本机已有替代品"这一栏）。
    pub near_misses: Vec<String>,
}

impl FitItem {
    /// 唯一可以"不问用户就直接换"的情形：同名（归一化后一致）。
    pub fn safe_to_auto_apply(&self) -> bool {
        self.status == FitStatus::Exact
    }

    /// 建议写进 overrides 的值（同名但字面不同时才有；完全一样时为 `None`）。
    pub fn suggested_override(&self) -> Option<String> {
        if self.status != FitStatus::Exact {
            return None;
        }
        self.candidates
            .first()
            .map(|c| c.model.name.clone())
            .filter(|name| *name != self.requirement.file_name)
    }

    pub fn hint(&self) -> String {
        match self.status {
            FitStatus::Exact => "本机有同名文件，不用改。".to_string(),
            FitStatus::Substitute => format!(
                "本机没有同名的；候选见 candidates。换之前**先问用户**（那是他的模型），\
                 确认后写 comfy_submit(overrides={{\"{}\":\"<候选名>\"}})。",
                self.requirement.override_key()
            ),
            FitStatus::OnDiskOnly => {
                "盘上有同名的（见 candidates[0].path），但**当前实例加载不了**：\
                 先让用户重启 ComfyUI / 修好模型根，**不要下载**（文件就在硬盘上），\
                 也不要改 overrides（改了同样报文件不存在）。"
                    .to_string()
            }
            FitStatus::Missing => {
                "本机没有。先去 civitai_search 查它，再用 ask_user_question \
                 让用户决定装 / 换工作流 / 放弃。"
                    .to_string()
            }
        }
    }
}

/// "这份工作流要的模型，本机有没有、最接近的是哪个、要改哪几个键"（`FitReport`）。
///
/// ## 它只报不改
///
/// 这是用户建议 ① 的原话（"**只报不改**，改由 `comfy_submit` 的 overrides 做"）。
/// 结构上就这么保证：这个对象**不生成图、也不写任何文件**。
#[derive(Debug, Clone)]
pub struct FitReport {
    pub inventory_source: Source,
    pub models_dir: Option<String>,
    pub local_count: usize,
    pub truncated: bool,
    pub missing_nodes: Vec<String>,
    pub items: Vec<FitItem>,
    /// 本机清单里"**盘上有、但当前跑的实例枚举不到**"的条数（2026-09-22 事故的诊断口）。
    ///
    /// `> 0` 就说明这个实例的模型根没挂全（`--extra-model-paths-config` 没生效 /
    /// 实例不是从 ComfyUI Desktop 那套配置起的）——报告里会明说这一句。
    pub inventory_disk_only: usize,
}

impl FitReport {
    /// 有没有"必须动手"的项（缺模型，或者要改键）。
    pub fn needs_action(&self) -> bool {
        self.items.iter().any(|i| i.status != FitStatus::Exact)
    }

    /// 一条"一模一样、不用改"的都没有（= 一份工作流都跑不起来）。
    pub fn all_missing(&self) -> bool {
        !self.items.is_empty() && self.items.iter().all(|i| i.status == FitStatus::Missing)
    }

    fn count(&self, status: FitStatus) -> usize {
        self.items.iter().filter(|i| i.status == status).count()
    }

    /// 可以**直接**贴进 `comfy_submit(overrides=…)` 的那几个键（保序、同键只留一条）。
    ///
    /// 只收 [`FitStatus::Exact`]（同名，换上去一定对）—— 替代品**不进这里**：
    /// 那是"看着像"，该由模型判断 + 用户点头，不该由代码替它们决定。
    pub fn suggested_overrides(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for item in self.items.iter().filter(|i| i.status == FitStatus::Exact) {
            // 同名时**不需要覆盖**（图里那个名字本机就有）→ 只有归一化后同名但字面不同才要改
            let Some(local) = item.candidates.first().map(|c| c.model.name.clone()) else {
                continue;
            };
            if local == item.requirement.file_name {
                continue;
            }
            let key = item.requirement.override_key();
            // Kotlin 那边是 `toMap()`：同键后者覆盖前者，位置留在**首次出现**处
            match out.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = local,
                None => out.push((key, local)),
            }
        }
        out
    }

    pub fn to_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("inventorySource".into(), json!(self.inventory_source.wire()));
        if let Some(d) = &self.models_dir {
            o.insert("modelsDir".into(), json!(d));
        }
        o.insert("localModels".into(), json!(self.local_count));
        if self.truncated {
            o.insert("inventoryTruncated".into(), Value::Bool(true));
        }
        o.insert("required".into(), json!(self.items.len()));
        o.insert("exact".into(), json!(self.count(FitStatus::Exact)));
        o.insert("substitute".into(), json!(self.count(FitStatus::Substitute)));
        // 盘上有、但这个实例枚举不到（2026-09-22）：单列一档，别混进 missing ——
        // 混进去的后果是模型让用户去下载一个本来就在硬盘上的文件
        o.insert("onDiskOnly".into(), json!(self.count(FitStatus::OnDiskOnly)));
        o.insert("missing".into(), json!(self.count(FitStatus::Missing)));
        if self.inventory_disk_only > 0 {
            o.insert("invisibleToInstance".into(), json!(self.inventory_disk_only));
        }
        if !self.missing_nodes.is_empty() {
            o.insert("missingNodes".into(), json!(self.missing_nodes));
        }
        o.insert(
            "models".into(),
            Value::Array(self.items.iter().map(item_json).collect()),
        );
        // 直接可用的覆盖建议（**只报**：模型要把它贴进 comfy_submit 的 overrides 才会生效）
        let suggested = self.suggested_overrides();
        if !suggested.is_empty() {
            let mut m = Map::new();
            for (k, v) in suggested {
                m.insert(k, Value::String(v));
            }
            o.insert("suggestedOverrides".into(), Value::Object(m));
        }
        o.insert("hint".into(), json!(self.render_hint()));
        Value::Object(o)
    }

    /// 给模型看的一小段：**结论先行**，然后是逐条。
    pub fn render(&self) -> String {
        let mut text = String::new();
        let required = self.items.len();
        if required == 0 {
            text.push_str(
                "这份工作流**没有点名任何模型文件**（连 checkpoint / LoRA / VAE 加载器都没有），\
                 所以没有模型要对。",
            );
            self.append_missing_nodes(&mut text);
            return text;
        }
        let exact = self.count(FitStatus::Exact);
        let sub = self.count(FitStatus::Substitute);
        let disk_only = self.count(FitStatus::OnDiskOnly);
        let miss = self.count(FitStatus::Missing);
        text.push_str(&format!("这份工作流要 {required} 个模型："));
        text.push_str(&format!("本机有同名的 {exact} 个"));
        if sub > 0 {
            text.push_str(&format!("、有可替代的 {sub} 个"));
        }
        if disk_only > 0 {
            text.push_str(&format!("、**硬盘上有同名但当前实例看不到的 {disk_only} 个**"));
        }
        if miss > 0 {
            text.push_str(&format!("、**本机没有的 {miss} 个**"));
        }
        text.push_str("。\n");
        text.push_str("（本机清单来自 ");
        if self.inventory_source == Source::ObjectInfo {
            text.push_str(&format!(
                "ComfyUI 的 /object_info，共 {} 条 —— 列出来的就是它能加载的",
                self.local_count
            ));
        } else {
            text.push_str(&format!("目录扫描，共 {} 条", self.local_count));
            if let Some(dir) = &self.models_dir {
                text.push_str(&format!("，扫的是 {dir}"));
            }
        }
        text.push_str("）\n");
        if self.inventory_disk_only > 0 {
            text.push_str(&format!(
                "⚠ 本机清单里有 {} 个文件**只出现在硬盘上、当前跑的 ComfyUI 实例枚举不到**——",
                self.inventory_disk_only
            ));
            text.push_str("说明这个实例的模型根没挂全（`--extra-model-paths-config` 没生效）。\n");
            text.push_str(
                "  这类文件**不要拿去 overrides**（提交只会报文件不存在），也**不要下载**（它就在硬盘上）：",
            );
            text.push_str(
                "先让用户重启 ComfyUI（ComfyHub 起实例时会自动挂上 ComfyUI Desktop 的模型根，\
                 见 `comfy_env_report`）再对一次。\n",
            );
        }

        for item in &self.items {
            text.push_str(&format!("\n· {}", item.requirement.override_key()));
            text.push_str(&format!("（{}", item.requirement.class_type));
            if let Some(t) = &item.requirement.title {
                text.push_str(&format!(" / {t}"));
            }
            text.push_str(&format!("，{}）要：", item.requirement.folder));
            text.push_str(&format!("{}\n", item.requirement.file_name));
            text.push_str("  ");
            text.push_str(&item_body(item));
            text.push('\n');
        }

        let suggested = self.suggested_overrides();
        if !suggested.is_empty() {
            text.push_str(
                "\n要改的键（**同名但写法不同**，直接贴进 comfy_submit 的 overrides 即可）：",
            );
            for (k, v) in &suggested {
                text.push_str(&format!("\n  \"{k}\": \"{v}\""));
            }
            text.push('\n');
        } else if exact == required && required > 0 {
            text.push_str("\n**这份工作流的模型本机全都有，不用改任何键。**\n");
        }
        self.append_missing_nodes(&mut text);
        text.push_str(
            "\n（这个工具**只报不改**：上面那些键要真生效，得由你写进 `comfy_submit(overrides=…)`；\
             替代品尤其要**先跟用户确认**再换 —— 那是他的模型。）\n",
        );
        text
    }

    fn render_hint(&self) -> String {
        if self.items.is_empty() {
            let mut s = "这份工作流没有点名模型文件".to_string();
            if self.missing_nodes.is_empty() {
                s.push('。');
            } else {
                s.push_str(&format!(
                    "，但有 {} 个节点类本机 ComfyUI 里没有（见 missingNodes）\
                     ——先用 comfy_check_inputs 看缺口。",
                    self.missing_nodes.len()
                ));
            }
            return s;
        }
        if self.items.iter().any(|i| i.status == FitStatus::OnDiskOnly) {
            return "要的模型**硬盘上就有**，只是当前这个 ComfyUI 实例枚举不到它们（模型根没挂全）。\
                    **不要去下载**，也别改 overrides：先让用户重启 ComfyUI\
                    （ComfyHub 起实例时会自动挂上 ComfyUI Desktop 的模型根），\
                    或用 comfy_env_report 看根目录差在哪。"
                .to_string();
        }
        if self.all_missing() {
            return "本机没有这份工作流要的模型。**不要硬提交**：先去 `civitai_search` 查这些模型\
                    （拿体积与下载链接），再用 `ask_user_question` 给用户 2~4 个具体选项\
                    （自己装 / 换用本机已有模型的等价工作流 / 放弃），让他选。"
                .to_string();
        }
        if self.needs_action() {
            return "有可替代的模型：要不要换由你判断 + 用户点头（overrides 写进 comfy_submit）。\
                    同名的那几个不用动。"
                .to_string();
        }
        "模型全都有同名的，可以直接提交。".to_string()
    }

    fn append_missing_nodes(&self, sb: &mut String) {
        if self.missing_nodes.is_empty() {
            return;
        }
        sb.push_str(
            "\n本机 ComfyUI 里**没有这些节点类**（先 comfy_get_status 确认它在跑；\
             确实是缺节点时，缺的节点要另装，这个工具不负责装）：",
        );
        sb.push_str(&self.missing_nodes.join("、"));
        sb.push('\n');
    }
}

/// 一条配对结果给模型看的那段（`when (item.status)` 逐字对照）。
fn item_body(item: &FitItem) -> String {
    match item.status {
        FitStatus::Exact => {
            let local = &item.candidates[0].model.name;
            if *local == item.requirement.file_name {
                "✅ 本机有同名文件，**不用改**。".to_string()
            } else {
                format!("✅ 本机有同名文件（写法不同：{local}），覆盖成它即可。")
            }
        }
        FitStatus::Substitute => {
            let mut parts: Vec<String> = Vec::with_capacity(item.candidates.len());
            for c in &item.candidates {
                let mut s = format!("{}（像 {}", c.model.name, (c.score * 100.0) as i64);
                if let Some(gb) = c.model.size_gb() {
                    s.push_str(&format!("，{gb:.1}GB"));
                }
                if !c.same_folder {
                    s.push_str(&format!("，在 {}/ 里", c.model.folder));
                }
                if !c.model.visible_to_instance {
                    s.push_str("，**当前实例看不到它**");
                }
                s.push('）');
                parts.push(s);
            }
            let tail = if item.candidates.iter().all(|c| !c.model.visible_to_instance) {
                "\n  ⚠️ 这些候选**当前实例都枚举不到**（在硬盘上但没挂上）：\
                 先让用户重启 ComfyUI 再提交，别拿它们去 overrides（只会得到一句文件不存在）。"
            } else {
                ""
            };
            format!(
                "⚠️ 本机没有同名的。最接近的：{} —— 像不像、能不能顶要你自己判断\
                 （分数只是字面相似度，**不保证架构兼容**）；拿不准就问用户。{tail}",
                parts.join("；")
            )
        }
        FitStatus::OnDiskOnly => {
            let local = &item.candidates[0].model;
            let mut s = format!("🟡 **硬盘上有这个文件**（{}", local.name);
            if let Some(gb) = local.size_gb() {
                s.push_str(&format!("，{gb:.1}GB"));
            }
            if let Some(path) = &local.path {
                s.push_str(&format!("，在 {path}"));
            }
            s.push_str(
                "），但**当前跑的 ComfyUI 实例枚举不到它**\
                 —— 说明这次启动没挂上那套模型根（共享目录 / 别的盘）。\n\
                 \x20 **先修挂载再提交，不要下载**（文件本来就在硬盘上），也**不要**改 overrides \
                 （改成它也一样加载不了）：让用户重启 ComfyUI（ComfyHub 起实例时会自动挂上），\
                 或先 `comfy_env_report` 看三个根到底差在哪。",
            );
            s
        }
        FitStatus::Missing => {
            let mut s = String::from(
                "❌ 本机没找到像样的替代品。**不要编一个文件名**去提交\
                 （ComfyUI 只会报文件不存在）。先去 `civitai_search` 查它（建议查询词：`",
            );
            s.push_str(&ModelName::lookup_query(&item.requirement.file_name));
            s.push_str(
                "`）—— 拿到**体积**（几 GB）与下载链接，\
                 再用 `ask_user_question` 让用户选：装它 / 换一条只用本机已有模型的工作流 / 放弃。\
                 提问的选项里**把体积写进去**（用户就是按这个数决定装不装的）。",
            );
            if item.near_misses.is_empty() {
                s.push_str("（本机连名字沾边的都没有。）");
            } else {
                s.push_str("（本机有几个**名字沾边但不是一个东西**的：");
                s.push_str(&item.near_misses.join("、"));
                s.push_str(" —— 别拿它们顶，报给用户时也可以提一句。）");
            }
            s
        }
    }
}

/// 一条模型 → JSON（键顺序与 Kotlin 的 `buildJsonObject` 一致）。
fn item_json(item: &FitItem) -> Value {
    let mut o = Map::new();
    o.insert("nodeId".into(), json!(item.requirement.node_id));
    o.insert("classType".into(), json!(item.requirement.class_type));
    o.insert("inputName".into(), json!(item.requirement.input_name));
    o.insert("wanted".into(), json!(item.requirement.file_name));
    o.insert("folder".into(), json!(item.requirement.folder));
    if let Some(t) = &item.requirement.title {
        o.insert("title".into(), json!(t));
    }
    o.insert("overrideKey".into(), json!(item.requirement.override_key()));
    o.insert("status".into(), json!(item.status.wire()));
    o.insert("safeToAutoApply".into(), json!(item.safe_to_auto_apply()));
    if !item.candidates.is_empty() {
        o.insert(
            "candidates".into(),
            Value::Array(
                item.candidates
                    .iter()
                    .map(|c| {
                        let mut m = Map::new();
                        m.insert("name".into(), json!(c.model.name));
                        m.insert("folder".into(), json!(c.model.folder));
                        m.insert("score".into(), json!(round2(c.score)));
                        m.insert("sameFolder".into(), json!(c.same_folder));
                        // 这个候选**当前实例能不能加载**：false 的不能拿去 overrides
                        // （ComfyUI 只会回一句文件不存在）
                        m.insert("visibleToInstance".into(), json!(c.model.visible_to_instance));
                        if let Some(p) = &c.model.path {
                            m.insert("path".into(), json!(p));
                        }
                        if let Some(gb) = c.model.size_gb() {
                            m.insert("sizeGB".into(), json!(round2(gb)));
                        }
                        Value::Object(m)
                    })
                    .collect::<Vec<_>>(),
            ),
        );
    }
    if let Some(s) = item.suggested_override() {
        o.insert("suggestedOverride".into(), json!(s));
    }
    // 缺模型时给"这个缺口该怎么补"的可执行事实（用户建议 ②）：
    // 查什么词、装到哪个目录、以及**本机有多少看起来沾边的替代品**
    if item.status == FitStatus::Missing {
        let mut p = Map::new();
        p.insert(
            "lookupQuery".into(),
            json!(ModelName::lookup_query(&item.requirement.file_name)),
        );
        p.insert("installInto".into(), json!(item.requirement.folder));
        p.insert("note".into(), json!(install_note(item)));
        o.insert("installProposal".into(), Value::Object(p));
    }
    o.insert("hint".into(), json!(item.hint()));
    Value::Object(o)
}

/// `installProposal.note`（Kotlin 那段字符串拼接逐字对照）。
fn install_note(item: &FitItem) -> String {
    let near = if item.near_misses.is_empty() {
        "没有".to_string()
    } else {
        item.near_misses.join("、")
    };
    format!(
        "先去 civitai_search(query=<lookupQuery>) 拿**体积与下载链接**，\
         再把\"装它 / 换工作流 / 放弃\"作为选项问用户。\
         本机那种\"沾一点边但不够像\"的近似文件：{near}"
    )
}

/// 保留两位小数（Kotlin `kotlin.math.round(x * 100) / 100.0`）。
fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// 把"图里要的模型"与"本机清单"配起来（`WorkflowFit.fit`，纯函数、可单测）。
///
/// * `known_nodes`：`/object_info` 的**节点类名集合**，用来报"本机没有这些节点"
///   （而"没点名模型"与"缺节点"是两件事，报告里分开说）；`None` = 不检查。
/// * `model_inputs`：[`crate::comfy::comfy_models::enumerate_model_inputs`] 现算的模型枚举输入，
///   用来认出自定义加载器；`None` = 只用内置那张表。
pub fn fit(
    graph: &Map<String, Value>,
    inventory: &Inventory,
    known_nodes: Option<&HashSet<String>>,
    model_inputs: Option<&HashMap<String, HashSet<String>>>,
) -> FitReport {
    let requirements = crate::comfy::comfy_models::requirements(graph, model_inputs);
    let mut items: Vec<FitItem> = Vec::with_capacity(requirements.len());
    for req in requirements {
        let candidates = inventory.candidates_for(&req.folder, &req.file_name);
        // 同名要分两种：**实例能加载**的才算"不用改"；只在盘上的要走 ON_DISK_ONLY
        // （不然就是"AI 说模型齐了、提交却报文件不存在"的反向版本）
        let exact_visible = candidates.iter().position(|c| {
            c.model.visible_to_instance && ModelName::same(&req.file_name, &c.model.name)
        });
        let exact_any =
            candidates.iter().position(|c| ModelName::same(&req.file_name, &c.model.name));
        // 同名优先：就算还有别的更高分的候选，"一模一样"永远是最好的答案
        let item = match (exact_visible, exact_any) {
            (Some(index), _) => FitItem {
                requirement: req,
                status: FitStatus::Exact,
                candidates: hoist(candidates, index),
                near_misses: Vec::new(),
            },
            (None, Some(index)) => FitItem {
                requirement: req,
                status: FitStatus::OnDiskOnly,
                candidates: hoist(candidates, index),
                near_misses: Vec::new(),
            },
            (None, None) if !candidates.is_empty() => FitItem {
                requirement: req,
                status: FitStatus::Substitute,
                candidates,
                near_misses: Vec::new(),
            },
            (None, None) => {
                // 缺模型时把"沾边但不够像"的也报出来：用户盘上那个像的名字最容易
                // 让他以为"我明明有这个模型"（用户建议 ② 的"本机已有替代品"那一栏）
                let near = inventory
                    .near_misses_for(&req.file_name, 3)
                    .into_iter()
                    .map(|m| m.name)
                    .collect();
                FitItem {
                    requirement: req,
                    status: FitStatus::Missing,
                    candidates: Vec::new(),
                    near_misses: near,
                }
            }
        };
        items.push(item);
    }

    let missing_nodes: Vec<String> = match known_nodes {
        None => Vec::new(),
        Some(known) => {
            let mut out: Vec<String> = Vec::new();
            for raw in graph.values() {
                let Some(cls) = raw
                    .as_object()
                    .and_then(|n| n.get("class_type"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                if cls.trim().is_empty() || known.contains(cls) || out.iter().any(|x| x == cls) {
                    continue;
                }
                out.push(cls.to_string());
            }
            out
        }
    };

    FitReport {
        inventory_source: inventory.source,
        models_dir: inventory.models_dir.as_ref().map(|p| p.display().to_string()),
        local_count: inventory.models.len(),
        truncated: inventory.truncated,
        missing_nodes,
        items,
        inventory_disk_only: inventory.disk_only.len(),
    }
}

/// 把第 `index` 条**挪到最前**（Kotlin 的 `listOf(hit) + candidates.filter { it !== hit }`：
/// 引用相等的过滤 = 只去掉那一条、其余顺序不变）。
fn hoist(candidates: Vec<Scored>, index: usize) -> Vec<Scored> {
    let mut out: Vec<Scored> = Vec::with_capacity(candidates.len());
    out.push(candidates[index].clone());
    for (i, c) in candidates.into_iter().enumerate() {
        if i != index {
            out.push(c);
        }
    }
    out
}

/// `comfy_fit_workflow`：把一份工作流要的模型和本机已有的模型对一遍（**只报不改**）。
pub async fn tool_fit_workflow(
    state: &AppState,
    arguments: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let graph = api_graph_of(arguments.get("graph")).ok_or_else(|| {
        ToolFailure::new(
            "INVALID_ARGUMENT",
            "graph 不是一份 API 格式节点图（应当形如 \
             {\"1\":{\"class_type\":\"CheckpointLoaderSimple\",\"inputs\":{\"ckpt_name\":\"…\"}}}）。\
             界面格式（nodes/links）请先用 comfy_load_workflow 读进库。",
        )
    })?;

    // 节点定义表拿得到就一起用：① 自定义加载器也能被认出来（不用维护白名单）；
    // ② 顺手报出"缺哪些节点类"。拿不到（ComfyUI 没跑）不影响配对 —— 那就扫盘。
    // ⚠ 取法必须与 `comfy_env_report` **同一份**（先问 `/object_info` 再并上盘扫），
    //   否则两件工具会对同一台机器给出不同的清单。
    let config = crate::comfy::capture::capture_config(state).ok();
    let base = config
        .as_ref()
        .map(|c| c.comfy_url.trim().trim_end_matches('/').to_string())
        .unwrap_or_default();
    let object_info = if base.is_empty() {
        None
    } else {
        crate::comfy::comfy_client::get_json(&format!("{base}/object_info"))
            .await
            .ok()
            .and_then(|v| v.as_object().cloned())
    };
    let project_root = state.project_root();
    let desktop = crate::tools::mcp::desktop(None);
    let model_dirs: Vec<PathBuf> =
        desktop.as_ref().map(|d| d.models_dirs.clone()).unwrap_or_default();
    let inv = crate::comfy::comfy_models::inventory(object_info.as_ref(), Some(&project_root), &model_dirs);
    if inv.source == Source::None {
        return Err(ToolFailure::new(
            "COMFY_UNREACHABLE",
            "拿不到本机的模型清单：ComfyUI 没在跑（/object_info 取不到），\
             也没在已知的 ComfyUI 目录下找到 models 文件夹。\
             先 comfy_get_status 确认它在跑，再重试一次；\
             **不要**凭记忆假设本机有没有某个模型。",
        ));
    }

    let known: Option<HashSet<String>> =
        object_info.as_ref().map(|m| m.keys().cloned().collect());
    let model_inputs = crate::comfy::comfy_models::enumerate_model_inputs(object_info.as_ref());
    let report = fit(&graph, &inv, known.as_ref(), Some(&model_inputs));
    Ok(ToolOutcome::new(report.render(), report.to_json()))
}

// ---------------------------------------------------------------------------
//  测试（`WorkflowInputsTest.kt` + `ComfyModelsTest.kt` + `ComfyInventoryUnionTest.kt`
//  里跟这两件工具有关的断言，逐条移植）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
