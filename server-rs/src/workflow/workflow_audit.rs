//! `comfy_audit_workflow`：跑之前先审一份本机工作流（`WorkflowAudit.kt` +
//! `ToolRegistry.kt` 里那条工具处理体的逐条对照，P4 第 63 步）。
//!
//! # 它要解决什么（用户 2026-09-22：「让 AI 具有审核本地工作流的能力」）
//!
//! 起因是一句追问：会话 `0bedb3f5` 里用户问"你有没有发现你用的 krea2 工作流本身有问题呢？"，
//! AI 承认"我上一轮确实没审这条骨架就用了"。那次踩到的东西**全是可以在跑之前查出来的**：
//!
//! | 那次的现象 | 代价 | 对应检查项 |
//! | --- | --- | --- |
//! | `Krea2_spine` 固定 seed + 固定前缀 → `mediaCount: 0` | 白跑 78 秒 | [`CHECK_OUTPUT_NAME`] |
//! | 20 个 rgthree 前端节点（Label / Bookmark / Fast Groups Bypasser） | 转换失败 | [`CHECK_FRONTEND`] |
//! | `WidgetToString.IS_CHANGED()` / `SystemNotification` 版本不兼容 | 0.01 秒空跑 | [`CHECK_NODE_VERSION`] |
//! | 用 `CheckpointLoaderSimple` 从 int8 一体包"只取 MODEL" | 与官方模板不符 | [`CHECK_LOADER_FOLDER`] |
//! | 8 步 / cfg 1 被任意改 | 质量崩 | [`CHECK_PARAMS`] |
//! | 1360×1024 + 4K 放大 | 8GB 显存吃紧 | [`CHECK_VRAM`] |
//! | 给 Krea 2 写 Danbooru tag、给 Anima 写自然语言 | 出图跑偏 | [`CHECK_PROMPT_STYLE`] |
//!
//! # 三段之间的分工（别混）
//!
//!  - **确定性检查（这里）**：只报**事实**（哪个节点、哪个值、和什么冲突），可单测、可复现；
//!  - **`skill`**（`skills/builtin/comfy-workflow-audit`）：写"所以该怎么办"的判定与修法；
//!  - **提交闸门**（[`crate::workflow::workflow_submit::preflight`] + [`Report::blocking_refusal`]）：只拦
//!    **必跑不了**的那些（前端节点没摘干净 / 已知版本不兼容节点 / 本机没有的节点类），
//!    参数档位、显存、提示词风格**只提醒不拦** —— 那些是"不推荐"，不是"跑不了"。
//!    （P4 第 70 步接上的：在那之前 `comfy_submit` 只有两道实例侧检查，
//!    [`Report::blocking_refusal`] 造好了却没人调。）
//!
//! # 一条纪律
//!
//! **拿不到的信息不许猜**：没有 `/object_info` 就不判"本机没有这个节点类"；
//! 不认识的模型就不判参数档位（宁可不说，也不要给一个会让人改错方向的结论）。
//!
//! # 与 Kotlin 的两处形状细节
//!
//! 1. `Report.toJson()` / [`Finding`] 是**手拼** JSON（Kotlin 那边是 `buildJsonObject`）：
//!    `put("fix", f.fix?.let { … })` 那种"有才写"的键这里也**省掉键**，不是写 `null`
//!    （`explicitNulls=false` 只管 `@Serializable` DTO，管不到手拼的对象）。
//! 2. Kotlin 的 `FrontendNodes.isBroadcast` / `isDisplay` **没有 Rust 对应物**
//!    （`workflow_convert.rs` 只把广播型节点记进 `unsupported`，不做等价改写）。
//!    这里把它们的两张表与三条正则**逐条搬**成不用 `regex` crate 的判据
//!    （本仓库没有 `regex` 依赖，为两个小模式引一个新依赖不划算）——
//!    判据逐字对齐：大小写不敏感只折 ASCII（Java 的 `CASE_INSENSITIVE` 也不折 Unicode）、
//!    `BROADCAST_EXACT` / `DISPLAY_VIRTUAL` 两张表**大小写敏感**。
//!
//! # 一处**如实承认的不一致**
//!
//! Kotlin 的模型清单是**注入的**（`comfyModels ?: …`），为 null 时 [`Report`] 的第 ⑦ 项直接跳过。
//! Rust 侧（与 `comfy_fit_workflow` / `comfy_env_report` 一样）直接调
//! [`crate::comfy::comfy_models::inventory`]，没有"清单能力没启用"这一档 —— 于是这里恒传 `Some(&inv)`。
//! 不影响任何用户可见行为：两种实现里清单都拿得到，而清单为空时第 ⑦ 项本来就返回空。

use serde_json::{json, Map, Value};

use crate::comfy::comfy_models::{Inventory, ModelName};
use crate::tools::tools::{ToolFailure, ToolOutcome};
use crate::web::routes::AppState;

/// 纯前端节点那一项（`CHECK_FRONTEND`）。
pub const CHECK_FRONTEND: &str = "frontendNodes";
/// 已知版本不兼容节点那一项（`CHECK_NODE_VERSION`）。
pub const CHECK_NODE_VERSION: &str = "nodeVersions";
/// 本机没有这个节点类那一项（`CHECK_UNKNOWN_NODE`）。
pub const CHECK_UNKNOWN_NODE: &str = "unknownNodes";
/// 输出前缀 + 固定 seed 那一项（`CHECK_OUTPUT_NAME`）。
pub const CHECK_OUTPUT_NAME: &str = "outputName";
/// 参数档位那一项（`CHECK_PARAMS`）。
pub const CHECK_PARAMS: &str = "params";
/// 显存风险那一项（`CHECK_VRAM`）。
pub const CHECK_VRAM: &str = "vram";
/// 提示词风格那一项（`CHECK_PROMPT_STYLE`）。
pub const CHECK_PROMPT_STYLE: &str = "promptStyle";
/// 加载器与目录不匹配那一项（`CHECK_LOADER_FOLDER`）。
pub const CHECK_LOADER_FOLDER: &str = "loaderFolder";

/// 结论的严重程度：只有 [`Level::Block`] 会被提交闸门拦下。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// 必跑不了（ComfyUI 验证阶段就会整片丢节点 / 报文件不存在）。
    Block,
    /// 跑得起来，但大概率不是你要的结果（白跑一次显卡时间）。
    Warn,
    /// 只是背景信息。
    Info,
}

impl Level {
    pub fn wire(self) -> &'static str {
        match self {
            Level::Block => "block",
            Level::Warn => "warn",
            Level::Info => "info",
        }
    }
}

/// 一条检查结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    pub check: &'static str,
    pub title: String,
    pub detail: String,
    /// 该怎么办（人话，一句话）。
    pub fix: Option<String>,
    pub nodes: Vec<String>,
}

/// 一次审核的结论（Kotlin `WorkflowAudit.Report`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub node_count: usize,
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn blocking(&self) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|f| f.level == Level::Block)
            .collect()
    }

    fn count(&self, level: Level) -> usize {
        self.findings.iter().filter(|f| f.level == level).count()
    }

    /// 只渲染**非阻断**的那几条（提交闸门要的那半张报告）。
    ///
    /// 拦下的那些走 [`Self::blocking_refusal`]（当场拒），剩下的（参数档位 / 显存 /
    /// 提示词风格 / 输出前缀 / 加载器目录）**只提醒** —— 但也要让模型看见，
    /// 否则它会以为"审核通过了 = 这份图没问题"。
    ///
    /// 一条都没有时返回 `None`：干净的图不该在提交结果里多出一句"审核通过"的噪声。
    pub fn render_non_blocking(&self) -> Option<String> {
        let mut out = String::new();
        for f in self.findings.iter().filter(|f| f.level != Level::Block) {
            out.push_str(&finding_block(f));
        }
        (!out.is_empty()).then_some(out)
    }

    /// 提交闸门要的那句话：有"必跑不了"的项就返回一段说明，否则 `None`。
    ///
    /// 只收 [`Level::Block`] —— 参数档位 / 显存 / 提示词风格那些**不许**拦
    /// （它们是"不推荐"，不是"跑不了"；拦了就是替用户做决定）。
    pub fn blocking_refusal(&self) -> Option<String> {
        let blocking = self.blocking();
        if blocking.is_empty() {
            return None;
        }
        let mut out = String::new();
        out.push_str(&format!(
            "**没有提交**：审核发现 {} 处\"跑不起来\"的问题（ComfyUI 验证阶段就会整片丢节点或直接报错）：\n",
            blocking.len()
        ));
        for f in blocking {
            out.push_str(&format!("\n· [{}] {}\n", f.check, f.title));
            out.push_str(&format!("  {}\n", f.detail));
            // 点出**是哪几个节点**：只说"图里还有 1 个纯前端节点"时，模型只能整份图重读一遍
            // 去猜是哪一个（2026-10-25 现场：它为此多花了 3 轮工具）。
            if !f.nodes.is_empty() {
                out.push_str(&format!("  · 涉及的节点：{}\n", f.nodes.join("、")));
            }
            if let Some(fix) = &f.fix {
                out.push_str(&format!("  → {fix}\n"));
            }
        }
        Some(out)
    }

    pub fn to_json(&self) -> Value {
        let mut findings: Vec<Value> = Vec::with_capacity(self.findings.len());
        for f in &self.findings {
            let mut o = Map::new();
            o.insert("level".into(), json!(f.level.wire()));
            o.insert("check".into(), json!(f.check));
            o.insert("title".into(), json!(f.title));
            o.insert("detail".into(), json!(f.detail));
            // `f.fix?.let { put(...) }`：Kotlin 手拼的是**省掉键**（不是写 null）
            if let Some(fix) = &f.fix {
                o.insert("fix".into(), json!(fix));
            }
            if !f.nodes.is_empty() {
                o.insert("nodes".into(), json!(f.nodes));
            }
            findings.push(Value::Object(o));
        }
        let mut out = Map::new();
        out.insert("nodeCount".into(), json!(self.node_count));
        out.insert("block".into(), json!(self.blocking().len()));
        out.insert("warn".into(), json!(self.count(Level::Warn)));
        out.insert("info".into(), json!(self.count(Level::Info)));
        out.insert("findings".into(), Value::Array(findings));
        Value::Object(out)
    }

    /// 给模型看的正文：结论先行，然后逐条。
    pub fn render(&self) -> String {
        if self.findings.is_empty() {
            return format!(
                "审核通过：{} 个节点，七项检查都没发现问题（前端节点 / 节点版本 / \
                 输出文件名 / 参数档位 / 显存 / 提示词风格 / 加载器目录）。\n\
                 （审核只看**图的结构**：模型在不在本机请用 comfy_fit_workflow；这份图的输入齐不齐\
                 用 comfy_check_inputs。）\n",
                self.node_count
            );
        }
        let mut out = String::new();
        out.push_str(&format!("审核结论（{} 个节点）：", self.node_count));
        out.push_str(&format!("**跑不起来 {} 处**、", self.blocking().len()));
        out.push_str(&format!("不建议 {} 处", self.count(Level::Warn)));
        if self.findings.iter().any(|f| f.level == Level::Info) {
            out.push_str(&format!("、另有 {} 条背景", self.count(Level::Info)));
        }
        out.push_str("。\n");
        for f in &self.findings {
            out.push_str(&finding_block(f));
        }
        out.push_str(
            "\n（审核**只报不改**：要摘节点用 `comfy_prune_workflow`，要改参数用 \
             `comfy_submit(overrides=…)`；我不动你的工作流。）\n",
        );
        out
    }
}

/// 一条结论渲染成的那一小块（[`Report::render`] / [`Report::render_non_blocking`] 共用一份，
/// 免得两处的措辞与记号漂移）。
fn finding_block(f: &Finding) -> String {
    let mark = match f.level {
        Level::Block => "🔴",
        Level::Warn => "🟠",
        Level::Info => "🔵",
    };
    let mut out = String::new();
    out.push_str(&format!("\n{mark} [{}] {}\n", f.check, f.title));
    out.push_str(&format!("  {}\n", f.detail));
    if !f.nodes.is_empty() {
        out.push_str(&format!("  涉及节点：{}\n", f.nodes.join("、")));
    }
    if let Some(fix) = &f.fix {
        out.push_str(&format!("  → {fix}\n"));
    }
    out
}

// ---------------------------------------------------------------------------
//  知识表（都是有出处的；拿不准的一律不写进来）
// ---------------------------------------------------------------------------

/// **纯前端**节点：ComfyUI 服务端根本没有这些类，图里带着它们 = 验证阶段整片失败。
///
/// rgthree 那一族来自 2026-09-22 的现场（`krea2SFWNSFWUncensoredImageTo_v10.json`：
/// 10 Label + Fast Groups Bypasser + 8 Bookmark + …共 20 个）。
/// `Anything Everywhere` 与显示节点由 [`is_broadcast`] / [`is_display`] 自己认得
/// （模式匹配），这里不重复列。
///
/// ⚠ `Fast Bypasser (rgthree)` 是 2026-10-17 补的：用户的 `krea2 i2i workflow.json` 里
/// 用的就是**单数**那个名字（rgthree 新旧版本的类名不同），而 `/object_info` 里查不到它 ——
/// 漏了这一条的表现是"这份工作流学不下来"，而屏幕上看不出为什么。
pub(crate) const KNOWN_FRONTEND_ONLY: [&str; 14] = [
    "Label (rgthree)",
    "Bookmark (rgthree)",
    "Fast Bypasser (rgthree)",
    "Fast Groups Bypasser (rgthree)",
    "Fast Groups Muter (rgthree)",
    "Image Comparer (rgthree)",
    "Context (rgthree)",
    "Context Big (rgthree)",
    "SetNode",
    "GetNode",
    "Reroute",
    "Note",
    // ComfyUI 自带的 Markdown 注释框（MOSS / Krea 官方模板里都有）：
    // 它和 `Note` 一样只活在前端。漏了它会被判成"本机没有这个节点类"，
    // 蓝图里就多一条**假 error**（MOSS 模板实测踩到）。
    "MarkdownNote",
    "PrimitiveNode",
];

/// **已知版本不兼容**的节点类（装了、但一跑就崩）。
///
/// 2026-09-22 实测：这两族在 `IS_CHANGED` 上签名对不上，验证阶段整片节点被丢，
/// `Prompt executed in 0.01 seconds`、零产物。**可随节点包更新而失效** ——
/// 所以文案里写的是"先更新节点包；更新完还报就摘掉"，而不是"永远别用"。
///
/// 用数组而不是 `HashMap` 是为了保住 Kotlin `mapOf` 的**插入序**（报出来的顺序要一致）。
const KNOWN_INCOMPATIBLE: [(&str, &str); 3] = [
    (
        "WidgetToString",
        "IS_CHANGED() missing 1 required positional argument",
    ),
    (
        "SystemNotification",
        "IS_CHANGED() missing 1 required keyword-only argument",
    ),
    (
        "SystemNotification|pysssss",
        "IS_CHANGED() missing 1 required keyword-only argument",
    ),
];

/// 认得出档位的模型（判据是**文件名里的词**，不是猜）。
struct ModelProfile {
    label: &'static str,
    /// `steps` 的闭区间（Kotlin 的 `IntRange`）。
    steps: (i32, i32),
    /// `cfg` 的闭区间（Kotlin 的 `ClosedFloatingPointRange<Double>`）。
    cfg: (f64, f64),
    /// 常用采样器（**顺序**要保住：报告里按 `/` 拼出来）。
    samplers: &'static [&'static str],
    note: &'static str,
}

const KREA2: ModelProfile = ModelProfile {
    label: "Krea 2（Turbo 档）",
    steps: (4, 8),
    cfg: (0.0, 1.5),
    samplers: &["euler", "er_sde", "dpmpp_2m", "dpmpp_2m_sde"],
    note: "官方 Turbo 部署参数是 8 步 / cfg 1 / mu 1.15（4-step 蒸馏 LoRA 时 4 步 / cfg 1）",
};

const ANIMA: ModelProfile = ModelProfile {
    label: "Anima",
    steps: (4, 32),
    cfg: (0.0, 4.5),
    samplers: &["er_sde", "euler", "euler_ancestral", "dpmpp_2m"],
    note: "官方推荐 er_sde / euler + cfg 1（合并底模常见 30 步 / cfg 4）",
};

/// 采样器节点（值里有 seed 与 steps / cfg 的那些）。
const SAMPLER_CLASSES: [&str; 6] = [
    "KSampler",
    "KSamplerAdvanced",
    "SamplerCustom",
    "SamplerCustomAdvanced",
    "KSamplerSelect",
    "RandomNoise",
];

/// 保存节点（值里有 filename_prefix 的那些）——**顺序**决定报告里列出来的次序。
const SAVE_PREFIX_KEYS: [&str; 3] = ["filename_prefix", "filename", "prefix"];

// ---------------------------------------------------------------------------
//  前端节点的两张表 + 三条模式（`FrontendNodes.kt` 的判据部分）
// ---------------------------------------------------------------------------

/// 只靠前端 JS 实现的广播型虚拟节点（**大小写敏感**，与 Kotlin 的 `in setOf(…)` 同）。
const BROADCAST_EXACT: [&str; 10] = [
    "Anything Everywhere",
    "Anything Everywhere?",
    "Anything Everywhere3",
    "Anything Everywhere3?",
    "Anything Everywhere4",
    "Anything Everywhere4?",
    "Anything Everywhere5",
    "Anything Everywhere5?",
    "Prompts Everywhere",
    "Seed Everywhere",
];

/// 只负责"看一眼"的显示节点（**大小写敏感**）。
const DISPLAY_VIRTUAL: [&str; 12] = [
    "PreviewImage",
    "PreviewAudio",
    "PreviewVideo",
    "Preview3D",
    "PreviewMask",
    "ShowText",
    "ShowText|pysssss",
    "Display Any",
    "DisplayAny",
    "DisplayString",
    "Display Int",
    "Display Float",
];

/// 这个类是不是"**服务端根本没有、只有 ComfyUI 前端才有**"的那一类。
///
/// 三条判据的**合取**（[`KNOWN_FRONTEND_ONLY`] 那张表 + [`is_broadcast`] + [`is_display`]）——
/// 它同时是两处的唯一真源：
///  * 这里挑"图里还有纯前端节点"（[`check_frontend_nodes`]）；
///  * 提交闸门那边（[`crate::workflow::workflow_submit::preflight_refusal`]）**跳过**这一档：
///    对它们说"让用户装/启用节点包"是错的方向（前端类装也装不上），
///    那句话必须由这里那条结论来说。
pub fn is_frontend_only(class_type: &str) -> bool {
    KNOWN_FRONTEND_ONLY.contains(&class_type) || is_broadcast(class_type) || is_display(class_type)
}

/// 这个类是不是"仅前端"的广播型虚拟节点（`FrontendNodes.isBroadcast`）。
pub fn is_broadcast(class_type: &str) -> bool {
    if BROADCAST_EXACT.contains(&class_type) {
        return true;
    }
    let lower = class_type.to_ascii_lowercase();
    // `^Anything\s*Everywhere` / `^AnythingEverywhere`（后者是前者在 `\s*` 取 0 时的特例，
    // 照抄两条：少一条就与 Kotlin 的模式表对不上号了）
    if let Some(rest) = lower.strip_prefix("anything") {
        if strip_java_whitespace(rest).starts_with("everywhere") {
            return true;
        }
    }
    // `Everywhere\b`（**没有前导 `\b`**：`xyzEverywhere` 在 Kotlin 那边也会命中）
    contains_then_word_boundary(&lower, "everywhere")
}

/// 这个类是不是"仅前端"的显示型虚拟节点（`FrontendNodes.isDisplay`）。
pub fn is_display(class_type: &str) -> bool {
    if DISPLAY_VIRTUAL.contains(&class_type) {
        return true;
    }
    let lower = class_type.to_ascii_lowercase();
    // `^Preview[A-Z]`（IGNORE_CASE → 大小写字母都算）
    if let Some(rest) = lower.strip_prefix("preview") {
        if rest
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false)
        {
            return true;
        }
    }
    // `^Show(Text|Anything|String|Int|Float|Image)`
    if let Some(rest) = lower.strip_prefix("show") {
        if ["text", "anything", "string", "int", "float", "image"]
            .iter()
            .any(|word| rest.starts_with(word))
        {
            return true;
        }
    }
    // `^Display\s?(Any|String|Int|Float|Text|Value)`
    if let Some(rest) = lower.strip_prefix("display") {
        let rest = strip_java_whitespace_at_most_one(rest);
        if ["any", "string", "int", "float", "text", "value"]
            .iter()
            .any(|word| rest.starts_with(word))
        {
            return true;
        }
    }
    false
}

/// Java 正则 `\s` 认得的那几个空白（**不是** Rust 的 `char::is_whitespace`：后者多收了
/// NBSP / U+2028 之类，会让 `Anything\u{a0}Everywhere` 在两边判得不一样）。
fn is_java_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r')
}

/// `\s*`。
fn strip_java_whitespace(text: &str) -> &str {
    text.trim_start_matches(is_java_whitespace)
}

/// `\s?`（最多吃掉一个）。
fn strip_java_whitespace_at_most_one(text: &str) -> &str {
    match text.chars().next() {
        Some(c) if is_java_whitespace(c) => &text[c.len_utf8()..],
        _ => text,
    }
}

/// Java 正则的 `\b`（ASCII 词边界，与 `RegexOption.IGNORE_CASE` 一样只折 ASCII）。
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `needle\b`：只判**尾部**词边界（模式里没有前导 `\b`）。
fn contains_then_word_boundary(text: &str, needle: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0usize;
    while let Some(offset) = text[from..].find(needle) {
        let end = from + offset + needle.len();
        let after_ok = end >= bytes.len() || !is_word_char(bytes[end] as char);
        if after_ok {
            return true;
        }
        from = from + offset + 1;
    }
    false
}

/// `\bword\b`（两边都要词边界）。
fn contains_word(text: &str, word: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0usize;
    while let Some(offset) = text[from..].find(word) {
        let start = from + offset;
        let end = start + word.len();
        let before_ok = start == 0 || !is_word_char(bytes[start - 1] as char);
        let after_ok = end >= bytes.len() || !is_word_char(bytes[end] as char);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

// ---------------------------------------------------------------------------
//  入口
// ---------------------------------------------------------------------------

/// 审一份 **API 格式**节点图。
///
/// * `object_info`：ComfyUI 的 `/object_info`（**`None` = ComfyUI 没跑**）。
///   为 `None` 时**不判**"本机没有这个节点类" —— 宁可少报，也不要凭"我没问到"下结论。
/// * `inventory`：本机模型清单（[`crate::comfy::comfy_models::inventory`]），用来判"加载器与目录不匹配"。
pub fn audit(
    graph: &Map<String, Value>,
    object_info: Option<&Map<String, Value>>,
    inventory: Option<&Inventory>,
) -> Report {
    let nodes = recognized_nodes(graph);
    let mut findings: Vec<Finding> = Vec::new();
    findings.extend(check_frontend_nodes(&nodes, object_info));
    findings.extend(check_node_versions(&nodes));
    findings.extend(check_output_name(graph));
    findings.extend(check_params(graph));
    findings.extend(check_vram(graph));
    findings.extend(check_prompt_style(graph));
    findings.extend(check_loader_folder(graph, inventory));
    Report {
        node_count: nodes.len(),
        findings,
    }
}

/// 图上"认得出来的节点"：`(id, 类名, 节点体)`，**保 JSON 文档顺序**。
///
/// 判据与 Kotlin 的 `graph.mapNotNull { … }` 逐字一致：值不是对象、或 `class_type` 是空白的，
/// 整条不算（`nodeCount` 数的就是这一批）。
fn recognized_nodes<'a>(
    graph: &'a Map<String, Value>,
) -> Vec<(String, String, &'a Map<String, Value>)> {
    let mut out: Vec<(String, String, &'a Map<String, Value>)> = Vec::new();
    for (id, raw) in graph {
        let Some(node) = raw.as_object() else {
            continue;
        };
        let class_type = node_str(node, "class_type").unwrap_or_default();
        if class_type.trim().is_empty() {
            continue;
        }
        out.push((id.clone(), class_type, node));
    }
    out
}

// ---------------------------------------------------------------------------
//  ① 前端外观节点
// ---------------------------------------------------------------------------

fn check_frontend_nodes(
    nodes: &[(String, String, &Map<String, Value>)],
    object_info: Option<&Map<String, Value>>,
) -> Vec<Finding> {
    // ⚠ **`/object_info` 里真的有这个类时，它就不是"服务端根本没有的类"。**
    //
    // 显示型那一族（`^Preview…` / `^Show…` / `^Display…`）最容易误判：ComfyUI 自带的
    // `PreviewImage` / `PreviewAny` 都是**真节点**（`output_node: true`），只是"看一眼、没下游"。
    // 2026-10-25 的现场就是它：`krea2 r2i` 里的 `55 PreviewAny` 被这张模式表判成纯前端节点，
    // 提交闸门当场按 `MODEL_NOT_LOADABLE` 拒 —— 而 ComfyUI 明明能跑它；模型只能一遍遍重读
    // 工作流、换着法子重写，永远过不去（用户看到的就是"AI 走了大量弯路还是没跑通"）。
    //
    // 广播型（`Anything Everywhere` 那一族）**不适用**这条：它们在本机被注册成**零输出桩**，
    // 在 `/object_info` 里查得到也照样跑不出东西（见 `workflow_frontend` 的文件头）。
    // `KNOWN_FRONTEND_ONLY` 那张表里的 rgthree 前端类则从来不会出现在 `/object_info`。
    let ui_only: Vec<&(String, String, &Map<String, Value>)> = nodes
        .iter()
        .filter(|(_, cls, _)| {
            if KNOWN_FRONTEND_ONLY.contains(&cls.as_str()) || is_broadcast(cls) {
                return true;
            }
            if is_display(cls) {
                // 拿不到 `/object_info` 时沿用静态判据（"不知道"不等于"没问题"）
                return object_info
                    .map(|info| !info.contains_key(cls.as_str()))
                    .unwrap_or(true);
            }
            false
        })
        .collect();
    let unknown: Vec<&(String, String, &Map<String, Value>)> = match object_info {
        None => Vec::new(),
        Some(info) => nodes
            .iter()
            .filter(|(_, cls, _)| {
                !info.contains_key(cls) && !KNOWN_FRONTEND_ONLY.contains(&cls.as_str())
            })
            .collect(),
    };

    let mut out: Vec<Finding> = Vec::new();
    if !ui_only.is_empty() {
        out.push(Finding {
            level: Level::Block,
            check: CHECK_FRONTEND,
            title: format!(
                "图里还有 {} 个纯前端节点（服务端根本没有这些类）",
                ui_only.len()
            ),
            detail: "这些类只存在于 ComfyUI **前端**（rgthree 的 Label / Bookmark / \
                     Fast Groups Bypasser、Set/Get、Anything Everywhere、显示节点…）。\
                     服务端拿到它们会在**验证阶段整片丢掉节点**，表现是跑完 0.01 秒、零产物。\
                     这次现场（`krea2SFWNSFWUncensoredImageTo_v10.json`）就是 20 个这类节点。"
                .to_string(),
            fix: Some(
                "让 `comfy_load_workflow` 读原文件（它会照前端行为等价改写），\
                 或用 `comfy_prune_workflow` 把这些节点摘掉并补上缺口。"
                    .to_string(),
            ),
            nodes: sorted_ids(ui_only.iter().map(|(id, _, _)| id.clone()).collect()),
        });
    }
    if !unknown.is_empty() {
        let listed = unknown
            .iter()
            .map(|(id, cls, _)| format!("{id} {cls}"))
            .collect::<Vec<_>>()
            .join("、");
        out.push(Finding {
            level: Level::Block,
            check: CHECK_UNKNOWN_NODE,
            title: format!("本机 ComfyUI 里没有这 {} 个节点类", unknown.len()),
            detail: format!("`/object_info` 里查不到它们，所以提交一定会被验证拒绝：{listed}。"),
            fix: Some(
                "要么装对应的自定义节点包（ComfyUI-Manager），要么把用到它们的分支摘掉\
                 （`comfy_prune_workflow(analyze=true)` 先看清楚）。"
                    .to_string(),
            ),
            nodes: sorted_ids(unknown.iter().map(|(id, _, _)| id.clone()).collect()),
        });
    }
    out
}

/// 节点 id 按**数字**升序（非数字排最后，`Int.MAX_VALUE` 那一档），同档保持原顺序
/// （Kotlin 的 `sortedBy { it.toIntOrNull() ?: Int.MAX_VALUE }` 是稳定排序）。
fn sorted_ids(ids: Vec<String>) -> Vec<String> {
    let mut items: Vec<(i64, String)> = ids
        .into_iter()
        .map(|id| (id.parse::<i64>().unwrap_or(i64::MAX), id))
        .collect();
    items.sort_by_key(|(key, _)| *key);
    items.into_iter().map(|(_, id)| id).collect()
}

// ---------------------------------------------------------------------------
//  ② 自定义节点版本不兼容
// ---------------------------------------------------------------------------

fn check_node_versions(nodes: &[(String, String, &Map<String, Value>)]) -> Vec<Finding> {
    let hits: Vec<&(String, String, &Map<String, Value>)> = nodes
        .iter()
        .filter(|(_, cls, _)| {
            KNOWN_INCOMPATIBLE
                .iter()
                .any(|(name, _)| *name == cls.as_str())
        })
        .collect();
    if hits.is_empty() {
        return Vec::new();
    }
    let listed = hits
        .iter()
        .map(|(id, cls, _)| {
            let reason = KNOWN_INCOMPATIBLE
                .iter()
                .find(|(name, _)| *name == cls.as_str())
                .map(|(_, reason)| *reason)
                .unwrap_or_default();
            format!("{id} {cls}（{reason}）")
        })
        .collect::<Vec<_>>()
        .join("；");
    vec![Finding {
        level: Level::Block,
        check: CHECK_NODE_VERSION,
        title: format!("有 {} 个节点与当前节点包版本不兼容", hits.len()),
        detail: format!(
            "{listed}。这些类**装了但一跑就崩**：验证阶段整片节点被丢，\
             `Prompt executed in 0.01 seconds`、零产物。"
        ),
        fix: Some(
            "先在 ComfyUI-Manager 里**更新这几个节点包**再试；更新后还报就摘掉这条链，\
             换等效节点（例如用 `easy showAnything` / `PreviewAny` 代替 `WidgetToString`）。"
                .to_string(),
        ),
        nodes: sorted_ids(hits.iter().map(|(id, _, _)| id.clone()).collect()),
    }]
}

// ---------------------------------------------------------------------------
//  ③ 输出文件名 + 固定 seed（那次白跑 78 秒的根因）
// ---------------------------------------------------------------------------

fn check_output_name(graph: &Map<String, Value>) -> Vec<Finding> {
    let mut prefixes: Vec<String> = Vec::new();
    for (id, raw) in graph {
        let Some(node) = raw.as_object() else {
            continue;
        };
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        for key in SAVE_PREFIX_KEYS {
            let value = inputs.get(key).and_then(primitive_text);
            // `if (!v.isNullOrBlank())`：全空白的也跳过
            if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
                prefixes.push(format!("{id}.{key} = {value}"));
            }
        }
    }
    let mut seeds: Vec<String> = Vec::new();
    for (id, raw) in graph {
        let Some(node) = raw.as_object() else {
            continue;
        };
        let class_type = node_str(node, "class_type").unwrap_or_default();
        if !SAMPLER_CLASSES.contains(&class_type.as_str()) {
            continue;
        }
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        for key in ["seed", "noise_seed"] {
            let Some(value) = inputs.get(key) else {
                continue;
            };
            if is_number_like(value) {
                seeds.push(format!(
                    "{id}.{key} = {}",
                    primitive_text(value).unwrap_or_default()
                ));
            }
        }
    }
    if prefixes.is_empty() || seeds.is_empty() {
        return Vec::new();
    }
    let mut nodes: Vec<String> = Vec::new();
    for entry in prefixes.iter().chain(seeds.iter()) {
        let node_id = entry.split('.').next().unwrap_or_default().to_string();
        if !nodes.contains(&node_id) {
            nodes.push(node_id);
        }
    }
    vec![Finding {
        level: Level::Warn,
        check: CHECK_OUTPUT_NAME,
        title: "输出前缀与种子都写死了 —— 每次跑都会落同一个文件名".to_string(),
        detail: format!(
            "保存节点：{}；采样器：{}。ComfyUI 会覆盖同名文件，本项目的捕获按 `prompt_id` + \
             文件 SHA-256 判重，于是**跑完 mediaCount: 0、提示\"与已有文件相同，未重复入库\"** \
             —— 等于白跑。（现场：`Krea2_spine` 沿用 seed 20260921 + 同前缀，78 秒后什么都没入库。）\
             注意**抽卡是常态**：用户要连出几张挑一张时，靠的就是\"**换 seed、不换前缀**\"来攒同一批图。",
            prefixes.join("、"),
            seeds.join("、")
        ),
        fix: Some(
            "看用户要什么：**抽卡**（\"再出几张\"\"这张不行\"）→ **保留 filename_prefix、只换 seed**\
             （用户说\"随机\"就把 seed 换成随机值，否则给一个新数字）；\
             **只跑一次定稿** → 换 seed 或换 prefix 都行。\
             `overrides={\"<采样器>.seed\":<新种子>}`（或 `{\"<保存节点>.filename_prefix\":\"<新前缀>\"}`）。"
                .to_string(),
        ),
        nodes,
    }]
}

// ---------------------------------------------------------------------------
//  ④ 参数档位是否匹配该模型
// ---------------------------------------------------------------------------

fn check_params(graph: &Map<String, Value>) -> Vec<Finding> {
    let mut model_names: Vec<String> = Vec::new();
    for raw in graph.values() {
        let Some(node) = raw.as_object() else {
            continue;
        };
        let class_type = node_str(node, "class_type").unwrap_or_default();
        if !class_type.to_ascii_lowercase().contains("loader") {
            continue;
        }
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        // `firstNotNullOfOrNull`：第一个**非 null** 的取值（空串也算取到了）
        let found = ["ckpt_name", "unet_name", "model_name", "lora_name"]
            .iter()
            .find_map(|key| inputs.get(*key).and_then(primitive_text));
        if let Some(found) = found {
            model_names.push(found);
        }
    }
    let Some(model_name) = model_names.into_iter().find(|name| !name.trim().is_empty()) else {
        return Vec::new();
    };

    let profile = if model_name.to_ascii_lowercase().contains("krea") {
        &KREA2
    } else if model_name.to_ascii_lowercase().contains("anima") {
        &ANIMA
    } else {
        // 不认识的模型**不判**（宁可不说，也不要给错方向）
        return Vec::new();
    };

    let mut out: Vec<Finding> = Vec::new();
    for (id, raw) in graph {
        let Some(node) = raw.as_object() else {
            continue;
        };
        let class_type = node_str(node, "class_type").unwrap_or_default();
        if !SAMPLER_CLASSES.contains(&class_type.as_str()) {
            continue;
        }
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        let steps = inputs.get("steps").and_then(int_or_null);
        let cfg = inputs.get("cfg").and_then(double_or_null);
        let sampler = inputs.get("sampler_name").and_then(primitive_text);
        let mut bad: Vec<String> = Vec::new();
        if let Some(steps) = steps {
            if steps < profile.steps.0 || steps > profile.steps.1 {
                bad.push(format!(
                    "steps = {steps}（该模型这一档是 {}~{}）",
                    profile.steps.0, profile.steps.1
                ));
            }
        }
        if let Some(cfg) = cfg {
            if cfg < profile.cfg.0 || cfg > profile.cfg.1 {
                bad.push(format!(
                    "cfg = {}（该模型这一档是 {}~{}）",
                    java_double(cfg),
                    java_double(profile.cfg.0),
                    java_double(profile.cfg.1)
                ));
            }
        }
        if let Some(sampler) = sampler {
            if !profile.samplers.contains(&sampler.as_str()) {
                bad.push(format!(
                    "sampler = {sampler}（常用 {}）",
                    profile.samplers.join("/")
                ));
            }
        }
        if !bad.is_empty() {
            out.push(Finding {
                level: Level::Warn,
                check: CHECK_PARAMS,
                title: format!("参数与 {model_name}（{}）的官方档位不符", profile.label),
                detail: format!("节点 {id}：{}。{}。", bad.join("；"), profile.note),
                fix: Some(
                    "按官方档位改（`comfy_submit(overrides=…)`），或者确认你就是要偏离这个档位。"
                        .to_string(),
                ),
                nodes: vec![id.clone()],
            });
        }
    }
    out
}

/// Kotlin `Double.toString()` 的形状（`0.0` 而不是 `0`）—— 报告正文里两边要一模一样。
///
/// Java 对整数值的 double 会保留 `.0`，而 Rust 的 `{}` 不会；科学计数法那一档
/// （`|x| ≥ 1e7`）在这里用不到（档位都是个位数），**不猜**、照 Rust 的写法走。
fn java_double(value: f64) -> String {
    if value.is_finite() && value == value.trunc() && value.abs() < 1e7 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

// ---------------------------------------------------------------------------
//  ⑤ 显存风险（只估、不拦）
// ---------------------------------------------------------------------------

fn check_vram(graph: &Map<String, Value>) -> Vec<Finding> {
    let mut max_pixels: i64 = 0;
    let mut has_upscale = false;
    let mut loras = 0usize;
    for raw in graph.values() {
        let Some(node) = raw.as_object() else {
            continue;
        };
        let class_type = node_str(node, "class_type").unwrap_or_default();
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        if class_type.contains("EmptyLatentImage") || class_type.contains("EmptySD3LatentImage") {
            let width = inputs.get("width").and_then(int_or_null).unwrap_or(0) as i64;
            let height = inputs.get("height").and_then(int_or_null).unwrap_or(0) as i64;
            if width * height > max_pixels {
                max_pixels = width * height;
            }
        }
        let lower = class_type.to_ascii_lowercase();
        if lower.contains("upscale") || lower.contains("imagescale") {
            has_upscale = true;
        }
        if lower.contains("lora") {
            loras += 1;
        }
    }
    let mut risks: Vec<String> = Vec::new();
    if max_pixels >= 2048 * 2048 {
        risks.push(format!("出图尺寸 {}MP（≥4MP）", max_pixels / 1_000_000));
    }
    if has_upscale {
        risks.push("带放大/重采样节点（放大那一步显存峰值最高）".to_string());
    }
    if loras >= 3 {
        risks.push(format!(
            "挂了 {loras} 个 LoRA（每个都要额外显存与显存带宽）"
        ));
    }
    if risks.is_empty() {
        return Vec::new();
    }
    vec![Finding {
        level: Level::Warn,
        check: CHECK_VRAM,
        title: "显存可能不够（本机实测是 RTX 4060 Laptop / 8GB）".to_string(),
        detail: format!(
            "风险点：{}。8GB 是小卡，这几个因素叠加时ComfyUI 会 OOM 或退化到共享内存（慢十倍以上）。",
            risks.join("；")
        ),
        fix: Some(
            "先在 `comfy_get_status` / `server_info` 看当前显存占用；真要跑就降一档\
             （尺寸、放大倍数、LoRA 数量三者择一），或加 `comfy_prune_workflow` 的 Sage Attention \
             补丁省显存。"
                .to_string(),
        ),
        nodes: Vec::new(),
    }]
}

// ---------------------------------------------------------------------------
//  ⑥ 提示词风格与模型是否匹配
// ---------------------------------------------------------------------------

fn check_prompt_style(graph: &Map<String, Value>) -> Vec<Finding> {
    let mut model_names: Vec<String> = Vec::new();
    for raw in graph.values() {
        let Some(node) = raw.as_object() else {
            continue;
        };
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        let found = ["ckpt_name", "unet_name"]
            .iter()
            .find_map(|key| inputs.get(*key).and_then(primitive_text));
        if let Some(found) = found {
            model_names.push(found);
        }
    }
    // ⚠ Kotlin 这里是 `firstOrNull()`（**没有** isNotBlank 过滤）——照抄
    let Some(model_name) = model_names.into_iter().next() else {
        return Vec::new();
    };

    let positives: Vec<String> = graph
        .values()
        .filter_map(|raw| {
            let node = raw.as_object()?;
            let class_type = node_str(node, "class_type").unwrap_or_default();
            if !matches!(
                class_type.as_str(),
                "CLIPTextEncode" | "CLIPTextEncodeSDXL" | "TextEncodeQwenImageEdit"
            ) {
                return None;
            }
            let inputs = node.get("inputs").and_then(Value::as_object)?;
            let text = inputs.get("text").and_then(primitive_text)?;
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        })
        .collect();
    if positives.is_empty() {
        return Vec::new();
    }
    // `maxByOrNull`：并列时取**先出现**的那个（严格 `<` 才换人）
    let Some(text) = positives
        .iter()
        .fold(
            None::<&String>,
            |best: Option<&String>, candidate| match best {
                Some(current) if current.len() >= candidate.len() => Some(current),
                _ => Some(candidate),
            },
        )
        .cloned()
    else {
        return Vec::new();
    };
    let commas = text.chars().filter(|c| *c == ',').count();
    let has_sentence = has_sentence(&text);
    let lower = model_name.to_ascii_lowercase();

    if lower.contains("krea") && commas >= 8 && !has_sentence {
        // `text.take(80)` 数的是 UTF-16 单元，这里按字符数（BMP 内等价）
        let head: String = text.chars().take(80).collect::<String>().replace('\n', " ");
        return vec![Finding {
            level: Level::Warn,
            check: CHECK_PROMPT_STYLE,
            title: "给 Krea 2 写的是 Danbooru tag 堆，而它要自然语言".to_string(),
            detail: format!(
                "这条正向提示词有 {commas} 个逗号、几乎没有成句结构（前 80 字：{head}…）。\
                 Krea 2 官方明确推荐 natural language，\"masterpiece, best quality\" 这类咒语没有意义。"
            ),
            fix: Some(
                "按 `krea-2` skill 的官方规则重写（主体+属性+动作 → 场景 → 构图 → 光线 → 媒介），\
                 tag 版留给 Anima；或者明确告诉用户你要用 tag 写法。"
                    .to_string(),
            ),
            nodes: Vec::new(),
        }];
    }
    if lower.contains("anima") && commas <= 2 && text.chars().count() > 200 {
        return vec![Finding {
            level: Level::Info,
            check: CHECK_PROMPT_STYLE,
            title: "给 Anima 写的是一整段自然语言".to_string(),
            detail: format!(
                "Anima 是 Danbooru tag 模型：外观/动作/表情这些关键特征写 tag 更稳，\
                 自然语言拿来补氛围。这条正向提示词 {commas} 个逗号、{} 字。",
                text.chars().count()
            ),
            fix: Some(
                "按 `anima-prompt` skill 转成 tag 为主（NL 只补氛围）；纯场景图可以照旧走\
                 `anima-scene-prompt`。"
                    .to_string(),
            ),
            nodes: Vec::new(),
        }];
    }
    Vec::new()
}

/// Kotlin 的 `Regex("""[.!?]|\bwith\b|\bwearing\b|\bstanding\b""", IGNORE_CASE)`。
fn has_sentence(text: &str) -> bool {
    if text.chars().any(|c| matches!(c, '.' | '!' | '?')) {
        return true;
    }
    let lower = text.to_ascii_lowercase();
    ["with", "wearing", "standing"]
        .iter()
        .any(|word| contains_word(&lower, word))
}

// ---------------------------------------------------------------------------
//  ⑦ 加载器与目录不匹配
// ---------------------------------------------------------------------------

fn check_loader_folder(graph: &Map<String, Value>, inventory: Option<&Inventory>) -> Vec<Finding> {
    let Some(inventory) = inventory else {
        return Vec::new();
    };
    if inventory.models.is_empty() {
        return Vec::new();
    }
    // 某个文件名在本机哪些目录里出现过（顺序 = 清单顺序，Kotlin 的 `toSet()` 是 LinkedHashSet）
    let folders_of = |name: &str| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for model in &inventory.models {
            let hit = ModelName::same(name, &model.name) || model.name.eq_ignore_ascii_case(name);
            if hit && !out.contains(&model.folder) {
                out.push(model.folder.clone());
            }
        }
        out
    };

    let mut out: Vec<Finding> = Vec::new();
    for (id, raw) in graph {
        let Some(node) = raw.as_object() else {
            continue;
        };
        let class_type = node_str(node, "class_type").unwrap_or_default();
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        // UNETLoader 只读 diffusion_models/（unet/），拿 checkpoints 里的文件一定报不存在
        let wanted: Option<(String, &'static str)> =
            if class_type.starts_with("UNETLoader") || class_type == "DiffusionModelLoader" {
                let value = inputs
                    .get("unet_name")
                    .or_else(|| inputs.get("model_name"))
                    .and_then(primitive_text)
                    .unwrap_or_default();
                Some((value, "diffusion_models"))
            } else if class_type.contains("CheckpointLoader") {
                let value = inputs
                    .get("ckpt_name")
                    .and_then(primitive_text)
                    .unwrap_or_default();
                Some((value, "checkpoints"))
            } else {
                None
            };
        let Some((file, expected)) = wanted else {
            continue;
        };
        if file.trim().is_empty() {
            continue;
        }
        let folders = folders_of(&file);
        if folders.is_empty() || folders.iter().any(|f| f.as_str() == expected) {
            continue;
        }
        let listed = folders
            .iter()
            .map(|f| format!("{f}/"))
            .collect::<Vec<_>>()
            .join("、");
        out.push(Finding {
            level: Level::Warn,
            check: CHECK_LOADER_FOLDER,
            title: format!("{class_type} 读的目录不对：{file} 不在 {expected}/ 里"),
            detail: format!(
                "本机这份文件在 {listed}，而 {class_type} 只认 {expected}/\
                 （枚举里看不到它 → 提交会报文件不存在）。"
            ),
            fix: Some(
                "换成该目录里真实存在的文件（`comfy_fit_workflow` 会给候选），\
                 或者换用读那个目录的加载器（例如 `CheckpointLoaderSimple` ← → `UNETLoader`）。"
                    .to_string(),
            ),
            nodes: vec![id.clone()],
        });
    }
    out
}

// ---------------------------------------------------------------------------
//  小工具
// ---------------------------------------------------------------------------

/// Kotlin `JsonObject.str(key)`：原始值 → 文本，全空白当没有。
fn node_str(node: &Map<String, Value>, key: &str) -> Option<String> {
    node.get(key)
        .and_then(primitive_text)
        .filter(|value| !value.trim().is_empty())
}

/// Kotlin 的 `contentOrNull`（数字给字面量、布尔给 `true` / `false`、null 给 `None`）。
fn primitive_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// 数值（整数）。JSON 里 `20` 是数字；前端有时写成 `"20"`，两种都认。
fn int_or_null(value: &Value) -> Option<i32> {
    primitive_text(value)?.trim().parse::<i32>().ok()
}

fn double_or_null(value: &Value) -> Option<f64> {
    primitive_text(value)?.trim().parse::<f64>().ok()
}

/// 固定种子（数字或数字串）；`"randomize"` 这种前端占位符不算。
fn is_number_like(value: &Value) -> bool {
    double_or_null(value).is_some()
}

// ---------------------------------------------------------------------------
//  工具入口
// ---------------------------------------------------------------------------

/// `comfy_audit_workflow`：审一份 API 格式节点图，**只报不改**。
///
/// ⚠ 节点定义表与模型清单的取法必须与 `comfy_fit_workflow` / `comfy_env_report` **同一份**
/// （先问 `/object_info` 再并上盘扫），否则三件工具会对同一台机器给出不同的清单 ——
/// 而"预检说能用、审核说没有"这种自相矛盾没法从界面上看出来。
pub async fn tool_audit_workflow(
    state: &AppState,
    arguments: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let graph =
        crate::workflow::workflow_fit::api_graph_of(arguments.get("graph")).ok_or_else(|| {
            ToolFailure::new(
                "INVALID_ARGUMENT",
                "graph 不是一份 API 格式节点图（应当形如 \
             {\"1\":{\"class_type\":\"KSampler\",\"inputs\":{…}}}）。\
             界面格式（nodes/links）请先用 comfy_load_workflow 读进库。",
            )
        })?;

    let config = crate::comfy::capture::capture_config(state).ok();
    let base = config
        .as_ref()
        .map(|c| c.comfy_url.trim().trim_end_matches('/').to_string())
        .unwrap_or_default();
    // 拿不到 /object_info 就**不判**"本机没有这个节点类"（`audit` 里那条纪律）
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
    let model_dirs: Vec<std::path::PathBuf> = desktop
        .as_ref()
        .map(|d| d.models_dirs.clone())
        .unwrap_or_default();
    let inventory = crate::comfy::comfy_models::inventory(
        object_info.as_ref(),
        Some(&project_root),
        &model_dirs,
    );

    let report = audit(&graph, object_info.as_ref(), Some(&inventory));
    Ok(ToolOutcome::new(report.render(), report.to_json()))
}

// ---------------------------------------------------------------------------
//  测试（`WorkflowAuditTest.kt` 的断言逐条移植）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
