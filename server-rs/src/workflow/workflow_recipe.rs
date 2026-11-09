//! **工作流配方**（P4 第 67 步）：把一份**具体**工作流归纳成
//! 「任务 + 骨架指纹 + 模型槽位 + 参数旋钮 + 输出」，落进 `workflow_recipes`，
//! 并让 AI 把它**绑到一个新模型上**。
//!
//! 逐条对照 `WorkflowRecipe.kt` / `WorkflowRecipeRepo.kt` / `WorkflowRecipeLearner.kt` /
//! `WorkflowRecipeBinder.kt` / `WorkflowRecipeRoutes.kt` 与 `ToolRegistry.kt` 里那三条工具。
//! 设计与论证在 `docs/workflow-recipe-design.md`（那是**为什么**），这里只写实现上的纪律：
//!
//! # 一、这是**归纳**，不是猜测
//!
//! 三层分解（设计 §2）全部踩在**代码里已有的事实**上：
//!
//!  - **模型槽位**直接复用 [`crate::comfy::comfy_models::requirements`] —— 那张"哪个类的哪个输入是
//!    一个模型文件名"的确定表（外加 `/object_info` 现算的枚举）。**这里不新建第二张角色表**；
//!    认不出的第三方加载器**不猜**（宁可漏报，也不要把 `model` 这种转发连线当成文件名）。
//!  - **骨架指纹**只由"执行相关节点类 + 类级连线"算出：节点 id、文件名、参数值**都不进指纹**
//!    （所以同一族工作流即使换了底模、换了节点编号，指纹仍然一样 —— 这就是"泛用"的可计算定义）。
//!  - **任务分类**按"输出与输入的位置"判（有视频输出 = video、有遮罩链 = inpaint、
//!    有参考图编辑链 = edit…），判据是显式类名集合，**不用子串瞎匹配**
//!    （`ResizeImageMaskNode` 里有 "Mask" 但它只是缩放，把它当 inpainting 就会骗到模型）。
//!
//! # 二、任务分类是**提示**，不是结论
//!
//! `inpaint` / `video` / `audio` 三档的判据是硬事实（遮罩链 / 视频输出 / 音频输出），
//! `edit` 与 `i2i` 的边界本来就是连续谱。所以认不出时回 [`Task::Unknown`] 并写进 `warnings`，
//! **不许硬塞一档**；`warnings` 里也永远有一句"按节点形状判的、可能不准"。
//!
//! # 三、配方**不是**可提交对象（红线 R10）
//!
//! 这里产出的只是**知识**：槽位（该改哪个键）+ 旋钮（能改什么）+ 骨架（这一族长什么样）。
//! 真正提交永远走 `comfy_submit(promptId=…, overrides=…)` 那条链路
//! （审批 → 提交 → 等 → `capture.captureRun()` 入库），配方既不生成图也不写用户的文件。
//!
//! # 四、纯函数、可单测
//!
//! 同一个 API 节点图进来，出去的结构逐字一样（没有时间、没有随机、没有 I/O）。
//! `_meta.title` 之类的界面信息只作为**可读性**带出去，不参与指纹与判定。
//!
//! # 五、学习与绑定**只有一份实现**
//!
//! [`learn_and_save`] / [`bind_and_report`] 是 AI 工具（`comfy_learn_workflow` /
//! `comfy_bind_recipe`）与 `POST /api/ai/workflow-recipes*` **共用的那两个函数**
//! （Kotlin 侧是 `Application.kt` 里那两个 lambda）。两处各写一遍必然漂移，
//! 而漂移的表现是"AI 学出来的配方和界面看到的不一样"（**不报错**）。
//!
//! # 六、最要紧的一条：**不许学一条缺了节点的骨架**
//!
//! 界面格式转 API 图时会**摘掉**它转换不了的节点（`unsupportedNodes`）并留下悬空输入。
//! 现场（2026-09-26 真机抽查）：用户有两份 Krea 2 图像编辑工作流，而当前跑着的实例里
//! 没有那几个 Krea2 自定义节点 —— 转换于是把整条编辑链摘掉了，19 个节点只剩 13 个。
//! 如果就这么学下去，配方会是一条**看着像、其实缺了编辑链**的骨架，而且**没有任何报错**。
//! 所以默认档是**拒学**（`tolerateUnsupported = false`，稳定码 `UNSUPPORTED_NODES`），
//! 只有显式带上它才学，且配方会带 `complete=false` 与 `gaps`。
//!
//! ⚠ **2026-09-27 的那条"有意与 Kotlin 不同"已经在 P4 第 68 步撤销**：`workflow_convert.rs`
//! 现在真的实现了「模拟用户行为」（[`crate::workflow::workflow_frontend`]），所以这里不再需要那句
//! "本后端还没做这一步"的补白 —— `emulateFrontend` 与 Kotlin 一样**真的改变转换结果**：
//! 开着时纯前端节点被等价改写（不进 `gaps`），关着时它们照旧留在 `unsupportedNodes` 里。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::infra::iso_time::iso_time;
use crate::library::repo::WriteError;
use crate::tools::policy::ToolPolicy;
use crate::tools::tools::{ToolFailure, ToolOutcome};
use crate::web::routes::{ApiError, AppState};

// ===========================================================================
//  一、解析核心（`WorkflowRecipe.kt`）
// ===========================================================================

/// 解析规则的版本号 —— **进指纹的输入**。
///
/// 判据（类集合 / 前端节点名单 / 分类规则）一改，同一个图的指纹就该变，
/// 否则库里会同时存在"用旧规则算的骨架"和"用新规则算的骨架"，`group by skeleton_hash`
/// 就会把两族混在一起，而**没有任何报错**。所以它进 sha256 的正文。
pub const VERSION: i64 = 1;

/// 旋钮值最长留多少字符（提示词可以很长；配方只用来"看出这是什么值"，不当作提交参数）。
pub const MAX_KNOB_VALUE: usize = 600;

/// 一份配方最多留几个旋钮（113 节点的工作流有几百个字面量，全列会淹掉模型要看的几条）。
pub const MAX_KNOBS: usize = 80;

/// 一条警告最长多少字（进工具结果与界面，太长会把结论挤掉）。
pub const MAX_WARNING_CHARS: usize = 300;

/// 这份工作流是干什么的（[`Task::wire`] 进库、进 `comfy_list_recipes(task=…)` 的过滤参数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    T2i,
    I2i,
    /// **参考图编辑**（instruction edit）：有参考图编辑链 —— Krea 2 的 `Krea2EditModelPatch` /
    /// `Krea2OstrisEditModelPatch` / `TextEncodeKrea2OstrisEdit*`、Flux Kontext 风格的多参考潜空间方法等。
    ///
    /// 它**优先于** [`Task::Inpaint`]：Krea 2 的 Identity Edit 同时带遮罩链（`ImageToMask` /
    /// `GrowMask`），但它的能力是"照参考图改" —— 把它归成 inpaint 的话，
    /// `comfy_list_recipes(task="edit")` 永远找不到它，而那正是用户说的"图像编辑工作流"。
    Edit,
    /// 遮罩局部重绘（`LoadImageMask` / `SetLatentNoiseMask` / `InpaintModelConditioning` …）。
    Inpaint,
    /// 只有放大模型、没有采样器。
    Upscale,
    Video,
    Audio,
    /// 认不出（空图 / 既不生成也不吃图）。**如实说，不硬塞**。
    Unknown,
}

impl Task {
    pub fn wire(self) -> &'static str {
        match self {
            Task::T2i => "t2i",
            Task::I2i => "i2i",
            Task::Edit => "edit",
            Task::Inpaint => "inpaint",
            Task::Upscale => "upscale",
            Task::Video => "video",
            Task::Audio => "audio",
            Task::Unknown => "unknown",
        }
    }

    pub fn parse(wire: &str) -> Option<Task> {
        [
            Task::T2i,
            Task::I2i,
            Task::Edit,
            Task::Inpaint,
            Task::Upscale,
            Task::Video,
            Task::Audio,
            Task::Unknown,
        ]
        .into_iter()
        .find(|t| t.wire() == wire)
    }
}

/// 旋钮的种类（只为让模型一眼看出"哪些是提示词、哪些是采样参数"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnobKind {
    /// 文本编码节点的正文（提示词）。
    Prompt,
    /// 图片 / 遮罩 / 视频文件名（`LoadImage.image`、`LoadImageMask.image`…）。
    Image,
    /// 采样参数（`steps` / `cfg` / `sampler_name` / `scheduler` / `denoise`…）。
    Sampling,
    /// 随机种子。
    Seed,
    /// 尺寸（`width` / `height` / `batch_size`…）。
    Size,
    /// **加载器上那些"模型族相关"的开关**（`CLIPLoader.type=krea2`、`UNETLoader.weight_dtype`、
    /// LoRA 权重…）。
    ///
    /// 单独一类的原因：它们不像文件名那样显眼，但换模型族时**照样要改**
    /// （`type=qwen_image` 与 `type=krea2` 不是一回事），漏掉它就会出现
    /// "底模换对了、文本编码器还是按老模型解"这种**不报错但结果不对**的情形。
    Loader,
    Other,
}

impl KnobKind {
    pub fn wire(self) -> &'static str {
        match self {
            KnobKind::Prompt => "prompt",
            KnobKind::Image => "image",
            KnobKind::Sampling => "sampling",
            KnobKind::Seed => "seed",
            KnobKind::Size => "size",
            KnobKind::Loader => "loader",
            KnobKind::Other => "other",
        }
    }

    pub fn parse(wire: Option<&str>) -> Option<KnobKind> {
        let wire = wire?;
        [
            KnobKind::Prompt,
            KnobKind::Image,
            KnobKind::Sampling,
            KnobKind::Seed,
            KnobKind::Size,
            KnobKind::Loader,
            KnobKind::Other,
        ]
        .into_iter()
        .find(|k| k.wire() == wire)
    }

    /// 旋钮分组在 [`Recipe::render`] 里的中文标签。
    fn label(self) -> &'static str {
        match self {
            KnobKind::Prompt => "提示词",
            KnobKind::Image => "图像/遮罩",
            KnobKind::Sampling => "采样",
            KnobKind::Seed => "种子",
            KnobKind::Size => "尺寸",
            KnobKind::Loader => "加载器参数（换模型族时也要看）",
            KnobKind::Other => "其它",
        }
    }

    /// 进指纹的排序位（Kotlin `KnobKind.entries.indexOf`）。
    fn order(self) -> usize {
        match self {
            KnobKind::Prompt => 0,
            KnobKind::Image => 1,
            KnobKind::Sampling => 2,
            KnobKind::Seed => 3,
            KnobKind::Size => 4,
            KnobKind::Loader => 5,
            KnobKind::Other => 6,
        }
    }
}

/// 一个模型槽位（= 一个"从磁盘加载模型文件"的输入，可换件）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// 角色/该放哪个目录：`checkpoints` / `diffusion_models` / `text_encoders` / `vae` / `loras` …
    pub role: String,
    pub node_id: String,
    pub class_type: String,
    pub input_name: String,
    pub file_name: String,
    /// 节点在文件里的标题（`_meta.title`，可读性用，不参与判定）
    pub title: Option<String>,
    /// `role` 是**猜的**：这个槽位来自 `/object_info` 现算的自定义加载器
    /// （`MODEL_INPUTS` 那张确定表里没有它），所以只知道"它要一个模型文件"，
    /// **不知道**该放哪个目录。
    ///
    /// 如实标出来而不是硬塞一个目录名：`UltralyticsDetectorProvider.model_name` 的真身是
    /// `models/ultralytics/bbox` 下的检测器权重，猜成 `diffusion_models` 会把人指到错的地方
    /// （真机探针上就这么显示过）。
    pub role_guessed: bool,
}

impl Slot {
    /// `comfy_submit(overrides=…)` 的键。
    pub fn override_key(&self) -> String {
        format!("{}.{}", self.node_id, self.input_name)
    }
}

/// 一个可覆盖的**字面量**输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Knob {
    pub node_id: String,
    pub class_type: String,
    pub input_name: String,
    pub kind: KnobKind,
    /// 当前值（数字/布尔按文本给；超长已按 [`MAX_KNOB_VALUE`] 截断）
    pub value: String,
    pub title: Option<String>,
}

impl Knob {
    pub fn override_key(&self) -> String {
        format!("{}.{}", self.node_id, self.input_name)
    }
}

/// 一个输出节点（图跑到哪儿结束）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub node_id: String,
    pub class_type: String,
    pub kind: String,
}

/// 归纳结果（**配方正文**）。
///
/// `skeleton_hash` 是"泛用"的键：同一族工作流（换了底模、换了节点编号、换了参数）指纹相同，
/// 于是 `comfy_list_recipes(task="edit")` 能把它们归到一族里让模型挑。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    pub task: Task,
    pub skeleton_hash: String,
    /// 执行相关节点类（**带重数**、已排序，进指纹）
    pub node_classes: Vec<String>,
    /// 类级连线（`下游类.输入 <- 上游类[槽位]`，带重数、已排序，进指纹）
    pub edges: Vec<String>,
    pub slots: Vec<Slot>,
    pub knobs: Vec<Knob>,
    pub outputs: Vec<Output>,
    /// 图里"从磁盘读一张图/一段视频"的节点个数（判 i2i 的依据，也是给模型看的规模）
    pub image_inputs: usize,
    pub has_mask: bool,
    pub node_count: usize,
    /// 旋钮是不是被 [`MAX_KNOBS`] 截断了
    pub knobs_truncated: bool,
    pub warnings: Vec<String>,
}

impl Recipe {
    /// 类的种类数（去重后的规模，报告里比节点数更能说明形状）。
    pub fn class_count(&self) -> usize {
        distinct(&self.node_classes).len()
    }

    /// 给模型看的正文。
    ///
    /// 刻意做成"一份能直接抄进 `comfy_submit(overrides=…)` 的清单"：
    /// 槽位在前（换模型就改这几个键）、旋钮按种类分组、输出在末尾。
    pub fn render(&self, name: Option<&str>) -> String {
        let mut out = String::new();
        out.push_str("【工作流配方】");
        out.push_str(name.unwrap_or("（未命名）"));
        out.push('\n');
        out.push_str("任务：");
        out.push_str(self.task.wire());
        if self.task == Task::Unknown {
            out.push_str("（**认不出**）");
        } else {
            out.push_str("（按节点形状判的）");
        }
        out.push_str("　骨架：");
        out.push_str(&self.skeleton_hash);
        out.push_str(&format!(
            "（{} 个类 / {} 条类级线）",
            self.node_classes.len(),
            self.edges.len()
        ));
        out.push('\n');

        out.push_str(&format!("模型槽位（{}）", self.slots.len()));
        if self.slots.is_empty() {
            out.push_str("：**一个都没认出**");
            out.push('\n');
        } else {
            out.push_str(" —— 换模型/换 LoRA 就改这几个键：\n");
            for slot in &self.slots {
                out.push_str("  · ");
                out.push_str(&slot.role);
                out.push_str("  ");
                out.push_str(&slot.override_key());
                out.push_str(" = ");
                out.push_str(&slot.file_name);
                if let Some(title) = &slot.title {
                    // Kotlin `!s.title.isNullOrBlank()`：空白标题也不算
                    if !title.trim().is_empty() {
                        out.push_str("　（");
                        out.push_str(title);
                        out.push('）');
                    }
                }
                out.push('\n');
            }
            let guessed = self.slots.iter().filter(|s| s.role_guessed).count();
            if guessed > 0 {
                out.push_str(&format!(
                    "  （其中 {guessed} 个的**目录名是猜的**：它们来自 /object_info 现算的自定义加载器，\
                     只知道那里要一个模型文件，不知道该放哪个目录 —— 别拿目录名当结论。）\n"
                ));
            }
        }

        if !self.knobs.is_empty() {
            out.push_str(&format!("参数旋钮（{}", self.knobs.len()));
            if self.knobs_truncated {
                out.push_str(&format!("，只列了前 {MAX_KNOBS} 个"));
            }
            out.push_str("）：\n");
            for kind in [
                KnobKind::Prompt,
                KnobKind::Image,
                KnobKind::Sampling,
                KnobKind::Seed,
                KnobKind::Size,
                KnobKind::Loader,
                KnobKind::Other,
            ] {
                let group: Vec<&Knob> = self.knobs.iter().filter(|k| k.kind == kind).collect();
                if group.is_empty() {
                    continue;
                }
                out.push_str("  · ");
                out.push_str(kind.label());
                out.push('：');
                let parts: Vec<String> = group
                    .iter()
                    .map(|k| {
                        let value = if k.value.chars().count() > 80 {
                            format!("{}…", take_chars(&k.value, 80))
                        } else {
                            k.value.clone()
                        };
                        format!("{}={}", k.override_key(), value)
                    })
                    .collect();
                out.push_str(&parts.join(" · "));
                out.push('\n');
            }
        }

        out.push_str("输出：");
        if self.outputs.is_empty() {
            out.push_str("**没有输出节点**（这张图跑完不会留下东西）");
        } else {
            let parts: Vec<String> = self
                .outputs
                .iter()
                .map(|o| format!("#{} {}（{}）", o.node_id, o.class_type, o.kind))
                .collect();
            out.push_str(&parts.join(" · "));
        }
        out.push('\n');
        out.push_str(&format!("图像输入：{} 个", self.image_inputs));
        out.push_str("　遮罩链：");
        out.push_str(if self.has_mask { "有" } else { "没有" });
        out.push_str(&format!("　节点数：{}", self.node_count));
        out.push('\n');

        if !self.warnings.is_empty() {
            out.push_str("提醒：\n");
            for warning in &self.warnings {
                out.push_str("  · ");
                out.push_str(warning);
                out.push('\n');
            }
        }
        out.trim_end().to_string()
    }

    /// 落库 / 进接口的 JSON 形状（键名稳定，界面与 AI 都按它取）。
    pub fn to_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("task".into(), json!(self.task.wire()));
        o.insert("skeletonHash".into(), json!(self.skeleton_hash));
        o.insert("recipeVersion".into(), json!(VERSION));
        o.insert("nodeCount".into(), json!(self.node_count));
        o.insert("classCount".into(), json!(self.class_count()));
        o.insert("edgeCount".into(), json!(self.edges.len()));
        o.insert("imageInputs".into(), json!(self.image_inputs));
        o.insert("hasMask".into(), json!(self.has_mask));
        o.insert("knobsTruncated".into(), json!(self.knobs_truncated));
        o.insert(
            "nodeClasses".into(),
            strings_json(&distinct(&self.node_classes)),
        );
        o.insert("edges".into(), strings_json(&self.edges));
        let slots: Vec<Value> = self
            .slots
            .iter()
            .map(|s| {
                let mut m = Map::new();
                m.insert("role".into(), json!(s.role));
                m.insert("nodeId".into(), json!(s.node_id));
                m.insert("classType".into(), json!(s.class_type));
                m.insert("inputName".into(), json!(s.input_name));
                m.insert("fileName".into(), json!(s.file_name));
                m.insert("overrideKey".into(), json!(s.override_key()));
                m.insert("roleGuessed".into(), json!(s.role_guessed));
                // `s.title?.let { put(...) }`：Kotlin 手拼的是**省掉键**（不是写 null）
                if let Some(title) = &s.title {
                    m.insert("title".into(), json!(title));
                }
                Value::Object(m)
            })
            .collect();
        o.insert("slots".into(), Value::Array(slots));
        let knobs: Vec<Value> = self
            .knobs
            .iter()
            .map(|k| {
                let mut m = Map::new();
                m.insert("nodeId".into(), json!(k.node_id));
                m.insert("classType".into(), json!(k.class_type));
                m.insert("inputName".into(), json!(k.input_name));
                m.insert("kind".into(), json!(k.kind.wire()));
                m.insert("value".into(), json!(k.value));
                m.insert("overrideKey".into(), json!(k.override_key()));
                if let Some(title) = &k.title {
                    m.insert("title".into(), json!(title));
                }
                Value::Object(m)
            })
            .collect();
        o.insert("knobs".into(), Value::Array(knobs));
        let outputs: Vec<Value> = self
            .outputs
            .iter()
            .map(|o| {
                let mut m = Map::new();
                m.insert("nodeId".into(), json!(o.node_id));
                m.insert("classType".into(), json!(o.class_type));
                m.insert("kind".into(), json!(o.kind));
                Value::Object(m)
            })
            .collect();
        o.insert("outputs".into(), Value::Array(outputs));
        o.insert("warnings".into(), strings_json(&self.warnings));
        Value::Object(o)
    }
}

// ---------------------------------------------------------------------------
//  判据表（全部是**显式类名**，不做子串猜测）
// ---------------------------------------------------------------------------

/// 没有执行语义、或只影响显示的节点。
///
/// 它们**不进指纹**：一份 113 节点的工作流里，`Note` / `Bookmark` / `Label` / `GetNode` / `SetNode`
/// 这类界面节点能占三分之一（实测：krea2SFWNSFW 那份 41 个类里有 6 类是这样、共 66 个节点）。
/// 把它们算进指纹，"同一族工作流"就永远不会相等 —— 而那正是配方要回答的问题。
///
/// ⚠ 这只是**兜底**：正常路径上界面节点已经在 [`crate::workflow::workflow_convert`] 那一步被展开/摘掉了。
const NON_EXECUTING: [&str; 13] = [
    "Note",
    "MarkdownNote",
    "Reroute",
    "PrimitiveNode",
    "GetNode",
    "SetNode",
    "Bookmark (rgthree)",
    "Label (rgthree)",
    "Fast Groups Bypasser (rgthree)",
    "Anything Everywhere",
    "Anything Everywhere3",
    "Seed (rgthree)",
    "easy showAnything",
];

/// 只显示、不产出产物的节点（`SaveImage` **不在**这里 —— 它是真输出）。
const DISPLAY_ONLY: [&str; 11] = [
    "PreviewImage",
    "PreviewAny",
    "MaskPreview+",
    "Image Comparer (rgthree)",
    "CR Simple Image Compare",
    "ImageCompare",
    "ShowText|pysssss",
    "easy showAnything",
    "WidgetToString",
    "SystemNotification|pysssss",
    "FancyTimerNode",
];

/// 从磁盘读一张图的节点（含遮罩图 —— 它也是一路图像输入）。
///
/// ⚠ 刻意**不含** `ImageOnlyCheckpointLoader`（它是模型加载器，不是"图从外部来"）
/// 也不含 `LoadAudio`（那是音频输入）：把它们算进来会把 t2i 误判成 i2i。
const IMAGE_LOADERS: [&str; 7] = [
    "LoadImage",
    "LoadImageMask",
    "LoadImageOutput",
    "LoadImagesFromDirectory",
    "VHS_LoadVideo",
    "LoadVideo",
    "LoadImageSetNode",
];

/// "这里有一张遮罩在决定重绘区"的显式节点。
const MASK_NODES: [&str; 12] = [
    "LoadImageMask",
    "ImageToMask",
    "MaskToImage",
    "SetLatentNoiseMask",
    "InpaintModelConditioning",
    "InpaintModelConditioningAdvanced",
    "GrowMask",
    "MaskBlur",
    "FeatherMask",
    "SolidMask",
    "MaskComposite",
    "DifferentialDiffusion",
];

/// **参考图编辑**链的显式节点。
///
/// 名单来自用户机器上真有的工作流（只读扫过，见设计 §5），**逐字对着真文件的
/// `class_type` 抄的** —— 第一版照着统计表写成了 `…Editx2`（那是"出现 2 次"的后缀），
/// 结果 Krea 2 的两份编辑工作流被归成了 `inpaint` / `i2i`（探针当场照出来的）：
/// - Krea 2：`Krea2EditModelPatch` / `Krea2OstrisEditModelPatch` / `Krea2EditGroundedEncode` /
///   `TextEncodeKrea2OstrisEdit` / `FluxKontextMultiReferenceLatentMethod`
/// - Flux Kontext / Qwen-Edit 一类：`ReferenceLatent` / `KontextSwitch` / `TextEncodeQwenImageEdit`
const EDIT_NODES: [&str; 9] = [
    "Krea2EditModelPatch",
    "Krea2OstrisEditModelPatch",
    "Krea2EditGroundedEncode",
    "TextEncodeKrea2OstrisEdit",
    "FluxKontextMultiReferenceLatentMethod",
    "ReferenceLatent",
    "KontextSwitch",
    "TextEncodeQwenImageEdit",
    "TextEncodeQwenImageEditPlus",
];

/// 输出节点 → 产物类别。
const OUTPUT_KINDS: [(&str, &str); 13] = [
    ("SaveImage", "image"),
    ("SaveImageWithMetadata", "image"),
    ("ImageSave", "image"),
    ("SaveVideo", "video"),
    ("VHS_VideoCombine", "video"),
    ("SaveWEBM", "video"),
    ("SaveAnimatedWEBP", "video"),
    ("SaveAnimatedPNG", "video"),
    ("SaveAudio", "audio"),
    ("SaveAudioAdvanced", "audio"),
    ("SaveAudioMP3", "audio"),
    ("SaveAudioOpus", "audio"),
    ("PreviewAudio", "audio"),
];

/// 一看就是视频链的类名（**子串**匹配是刻意的：第三方视频节点前缀换得很勤）。
const VIDEO_HINTS: [&str; 19] = [
    "SaveVideo",
    "VideoCombine",
    "VHS_",
    "WanImageToVideo",
    "WanVideo",
    "WanFirstLastFrame",
    "MiniMaxH3",
    "MiniMax",
    "HunyuanVideo",
    "LTXV",
    "LTXVideo",
    "Mochi",
    "CogVideo",
    "SVD_",
    "SVDImg2Vid",
    "ImageToVideo",
    "Kling",
    "Seedance",
    "Veo",
];

/// 音频链的类名子串。
const AUDIO_HINTS: [&str; 5] = [
    "SaveAudio",
    "PreviewAudio",
    "MiniMaxMusic",
    "MusicGen",
    "AudioEncoder",
];

/// 放大链（"没有采样器 + 有放大模型" = upscale）。
const UPSCALE_NODES: [&str; 7] = [
    "UpscaleModelLoader",
    "ImageUpscaleWithModel",
    "UpscaleImage",
    "UpscaleImageByUsingModel",
    "ImageScaleBy",
    "UltimateSDUpscale",
    "ImageScale",
];

/// 文本编码节点（这些类上的 `string` / `positive` 之类就是提示词）。
///
/// ⚠ `TextEncodeKrea2OstrisEditx2` 是 Kotlin 那边**留下来的一条死项** —— 它不是真类名
/// （那串 `x2` 是当时统计表里"出现 2 次"的后缀，见设计 §5.1 ①）。真类名
/// `TextEncodeKrea2OstrisEdit` 的正文走的是 [`knob_kind`] 的第一条规则（输入名叫 `text`），
/// 所以**行为上没有差别**；这里照抄它，是为了不在没有收益的地方与冻结源分叉。
const PROMPT_CLASSES: [&str; 14] = [
    "CLIPTextEncode",
    "CLIPTextEncodeSDXL",
    "CLIPTextEncodeSDXLRefiner",
    "CLIPTextEncodeFlux",
    "TextEncodeKrea2OstrisEditx2",
    "TextEncodeQwenImageEdit",
    "TextEncodeQwenImageEditPlus",
    "T5TextEncode",
    "TextInput_",
    "PrimitiveStringMultiline",
    "String Literal",
    "BNK_CLIPTextEncodeAdvanced",
    "smZ CLIPTextEncode",
    "easy text",
];

const SAMPLING_INPUTS: [&str; 17] = [
    "steps",
    "cfg",
    "sampler_name",
    "scheduler",
    "denoise",
    "guidance",
    "shift",
    "start_at_step",
    "end_at_step",
    "add_noise",
    "return_with_leftover_noise",
    "sigma_max",
    "sigma_min",
    "eta",
    "noise_type",
    "sampler",
    "sigmas",
];

const SEED_INPUTS: [&str; 4] = ["seed", "noise_seed", "rand_seed", "seed_num"];

const SIZE_INPUTS: [&str; 9] = [
    "width",
    "height",
    "batch_size",
    "resolution",
    "megapixels",
    "aspect_ratio",
    "target_width",
    "target_height",
    "scale_by",
];

const IMAGE_INPUTS: [&str; 8] = [
    "image", "images", "mask", "masks", "image1", "image2", "image_b", "video",
];

/// 采样器（判 upscale 用 + 报告里给模型看的"有没有采样"）。
///
/// Kotlin 那一串 `== "KSampler" || == "KSamplerAdvanced" || contains("SamplerCustom") ||
/// contains("KSampler") || contains("Sampler_") || == "SamplerCustom" ||
/// == "ClownsharKSampler_Beta" || contains("Sampler")` 里，**末尾那条包含前面全部**
/// （上面每一个都含 "Sampler"），所以只留末尾那条 —— 行为逐字等价，少一份会漂移的名单。
fn looks_like_sampler(class_type: &str) -> bool {
    class_type.contains("Sampler")
}

/// 加载器节点（它上面那些非文件输入 = [`KnobKind::Loader`]）。
fn looks_like_loader(class_type: &str) -> bool {
    // Kotlin `cls.contains("Loader") || cls.contains("Load") && !cls.contains("LoadImage")`
    // —— `&&` 比 `||` 紧，所以后一半要给 `LoadImage` 留一条生路
    class_type.contains("Loader")
        || (class_type.contains("Load") && !class_type.contains("LoadImage"))
}

/// 认得出的图片/视频后缀（`looks_like_file_name` 用）。
///
/// Kotlin 那张表里 `.gif` 出现了两次（无害的重复项），这里只留一份。
const FILE_EXTENSIONS: [&str; 18] = [
    ".png", ".jpg", ".jpeg", ".webp", ".bmp", ".gif", ".tiff", ".tif", ".exr", ".mp4", ".webm",
    ".mkv", ".mov", ".avi", ".wav", ".mp3", ".flac", ".ogg",
];

/// "这个字面量看起来像一个文件名"。
///
/// 只在**已知是文件语义的输入名**（[`IMAGE_INPUTS`]）上用它，所以判据可以宽松：
/// 带后缀，或者像 ComfyUI 的 input 路径（`clipspace/xxx.png [input]` / `pasted/xxx.png`）。
fn looks_like_file_name(value: &str) -> bool {
    if value.trim().is_empty() {
        return false;
    }
    let lower = value.to_lowercase();
    if lower.starts_with("clipspace/") || lower.starts_with("pasted/") {
        return true;
    }
    FILE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
        || lower.contains(".png [input]")
        || lower.contains(".jpg [input]")
        || lower.contains(".webp [input]")
}

// ---------------------------------------------------------------------------
//  归纳
// ---------------------------------------------------------------------------

/// 归纳时用的中间结构（不对外）。
struct Node {
    id: String,
    class_type: String,
    inputs: Map<String, Value>,
    title: Option<String>,
}

/// 归纳一份 **API 格式**节点图。
///
/// * `graph`：API 节点图（`{"<id>": {"class_type": …, "inputs": {…}}}`）。
///   界面格式（`nodes`/`links`）**不能**直接传进来 —— 先用 `comfy_load_workflow` /
///   [`crate::workflow::workflow_convert::to_api_graph`] 转。
/// * `model_inputs`：可选，`/object_info` 现算的"哪个类哪个输入是模型枚举"
///   （见 [`crate::comfy::comfy_models::enumerate_model_inputs`]）。为 `None` 时只用内置那张表，
///   并**如实写在 warnings 里** —— 自定义加载器的槽位会漏报，模型必须知道这一点。
pub fn analyze(
    graph: &Map<String, Value>,
    model_inputs: Option<&HashMap<String, HashSet<String>>>,
) -> Recipe {
    let mut nodes: Vec<Node> = Vec::new();
    for (id, raw) in graph {
        let Some(node) = raw.as_object() else {
            continue;
        };
        let Some(class_type) = node
            .get("class_type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|c| !c.trim().is_empty())
        else {
            continue;
        };
        let inputs = node
            .get("inputs")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let title = node
            .get("_meta")
            .and_then(Value::as_object)
            .and_then(|m| m.get("title"))
            .and_then(prim_content);
        nodes.push(Node {
            id: id.clone(),
            class_type,
            inputs,
            title,
        });
    }

    let exec: Vec<&Node> = nodes
        .iter()
        .filter(|n| !NON_EXECUTING.contains(&n.class_type.as_str()))
        .collect();
    let by_id: HashMap<&str, &Node> = exec.iter().map(|n| (n.id.as_str(), *n)).collect();

    // --- 骨架：执行相关类（带重数）+ 类级连线（带重数）---------------
    let fingerprint_classes: Vec<String> = exec
        .iter()
        .filter(|n| !DISPLAY_ONLY.contains(&n.class_type.as_str()))
        .map(|n| n.class_type.clone())
        .collect();
    let mut edges: Vec<String> = Vec::new();
    for node in &exec {
        // 纯显示节点的**出边**也不算：`Image Comparer` / `PreviewImage` 会挂到 SaveImage 上，
        // 把它们算进来，"多插了一个对比节点"就会换指纹（而它一个字都不影响这次跑的结果）
        if DISPLAY_ONLY.contains(&node.class_type.as_str()) {
            continue;
        }
        for (name, value) in &node.inputs {
            let Some(link) = value.as_array() else {
                continue;
            };
            let Some(upstream_id) = link.first().and_then(prim_content) else {
                continue;
            };
            let slot = link
                .get(1)
                .and_then(prim_content)
                .unwrap_or_else(|| "0".to_string());
            let Some(upstream) = by_id.get(upstream_id.as_str()) else {
                continue;
            };
            if DISPLAY_ONLY.contains(&upstream.class_type.as_str()) {
                continue;
            }
            edges.push(format!(
                "{}.{}<-{}[{}]",
                node.class_type, name, upstream.class_type, slot
            ));
        }
    }
    let mut sorted_classes = fingerprint_classes;
    sorted_classes.sort();
    edges.sort();

    let outputs: Vec<Output> = exec
        .iter()
        .filter_map(|node| {
            output_kind(&node.class_type).map(|kind| Output {
                node_id: node.id.clone(),
                class_type: node.class_type.clone(),
                kind: kind.to_string(),
            })
        })
        .collect();
    // 槽位两遍算：`model_inputs` 那一遍是**全量**（含 /object_info 现算的自定义加载器），
    // `None` 那一遍只走内置确定表 —— 差集就是"目录名是猜的"那些（见 [`Slot::role_guessed`]）
    let all_requirements = crate::comfy::comfy_models::requirements(graph, model_inputs);
    let known_slot_keys: HashSet<String> = crate::comfy::comfy_models::requirements(graph, None)
        .into_iter()
        .map(|r| r.override_key())
        .collect();
    let slot_keys: HashSet<String> = all_requirements.iter().map(|r| r.override_key()).collect();
    let slots: Vec<Slot> = all_requirements
        .into_iter()
        .map(|req| {
            let node = by_id.get(req.node_id.as_str());
            let override_key = req.override_key();
            Slot {
                role: req.folder,
                node_id: req.node_id,
                class_type: req.class_type,
                input_name: req.input_name,
                file_name: req.file_name,
                title: node.and_then(|n| n.title.clone()).or(req.title),
                role_guessed: !known_slot_keys.contains(&override_key),
            }
        })
        .collect();
    let image_inputs = exec
        .iter()
        .filter(|n| IMAGE_LOADERS.contains(&n.class_type.as_str()))
        .count();
    let has_mask = exec
        .iter()
        .any(|n| MASK_NODES.contains(&n.class_type.as_str()))
        || exec.iter().any(|n| n.inputs.contains_key("mask"));

    // 模型槽位的键不进旋钮：它们在 slots 里已经有一份（重复列只会让两边对不上时更难查）
    let mut all_knobs: Vec<Knob> = Vec::new();
    for node in &exec {
        for (name, value) in &node.inputs {
            if slot_keys.contains(&format!("{}.{}", node.id, name)) {
                continue;
            }
            let Some(text) = prim_content(value) else {
                continue;
            };
            all_knobs.push(Knob {
                node_id: node.id.clone(),
                class_type: node.class_type.clone(),
                input_name: name.clone(),
                kind: knob_kind(&node.class_type, name, &text),
                value: if text.chars().count() > MAX_KNOB_VALUE {
                    format!("{}…", take_chars(&text, MAX_KNOB_VALUE))
                } else {
                    text
                },
                title: node.title.clone(),
            });
        }
    }
    // 稳定排序：先按种类（提示词 / 图像 / 采样 / 尺寸 / 种子 / 加载器 / 其它），同种内保持文件顺序
    all_knobs.sort_by_key(|k| k.kind.order());
    let knobs_truncated = all_knobs.len() > MAX_KNOBS;
    let knobs: Vec<Knob> = all_knobs.into_iter().take(MAX_KNOBS).collect();

    let classes: Vec<String> = exec.iter().map(|n| n.class_type.clone()).collect();
    let task = classify(&classes, &outputs, image_inputs, has_mask);
    let mut warnings: Vec<String> = Vec::new();
    if nodes.is_empty() {
        warnings.push(
            "这张图里一个节点都没有（空图）：要么文件选错了，要么转换时被摘空了。".to_string(),
        );
    }
    if task == Task::Unknown && !nodes.is_empty() {
        warnings.push(
            "认不出这是什么任务（既没有输出节点、也没有图像输入）—— 配方里 task=unknown 是**如实回答**，\
             不要当成某一类去用。"
                .to_string(),
        );
    } else if task == Task::Edit || task == Task::I2i {
        warnings.push(format!(
            "task 是按节点形状判的（{}）：edit 与 i2i 的边界是连续谱，拿不准就按槽位与旋钮自己复核。",
            task.wire()
        ));
    }
    if model_inputs.is_none() {
        warnings.push(
            "拿不到 ComfyUI 的 /object_info：**自定义加载器**的模型槽位可能漏报\
             （内置那张表只覆盖常见节点）。要对齐模型就用 comfy_fit_workflow 再核一遍。"
                .to_string(),
        );
    } else if slots.is_empty() && exec.iter().any(|n| looks_like_loader(&n.class_type)) {
        warnings.push(
            "图里有加载器节点，但没有任何**已知**的模型槽位：可能是自定义加载器，别自己编文件名。"
                .to_string(),
        );
    }
    if knobs_truncated {
        warnings.push(format!(
            "旋钮只列了前 {MAX_KNOBS} 个（按种类优先），要看全就用 comfy_get_run / includeGraph 拿原图。"
        ));
    }
    if exec.is_empty() && !nodes.is_empty() {
        warnings.push("图里只有界面节点（Note / Reroute 之类），没有可执行内容。".to_string());
    }
    let warnings: Vec<String> = warnings
        .into_iter()
        .map(|w| take_chars(&w, MAX_WARNING_CHARS))
        .collect();

    Recipe {
        task,
        skeleton_hash: skeleton_hash(&sorted_classes, &edges),
        node_classes: sorted_classes,
        edges,
        slots,
        knobs,
        outputs,
        image_inputs,
        has_mask,
        node_count: nodes.len(),
        knobs_truncated,
        warnings,
    }
}

/// 骨架指纹：`sha256("v<规则版本>|<类集合>|<类级连线>")` 取前 16 位十六进制。
///
/// 刻意**不含**：节点 id、模型文件名、参数值、`_meta` 标题、纯界面/显示节点。
/// 含**重数**（两个 `LoraLoader` 与一个的骨架不是同一个形状）。
pub fn skeleton_hash(sorted_classes: &[String], sorted_edges: &[String]) -> String {
    let text = format!(
        "v{VERSION}|{}|{}",
        sorted_classes.join(","),
        sorted_edges.join(";")
    );
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// 任务分类（顺序即优先级，见 [`Task`] 各档注释）。
///
/// ⚠ 顺序不能随手改：`audio`/`video` 必须在最前（输出即事实），
/// `edit` 必须在 `inpaint` 前（Krea 2 的 Identity Edit 两条链都有）。
///
/// `has_mask` 与 [`Recipe::has_mask`] **同一个判据**（`analyze` 传同一个值进来）：
/// 两处各算一遍就会出现"报告说'遮罩链：有'、任务却判成 i2i"这种自相矛盾
/// （真机上实测到过：`MiaoMiao Harem.json` 就是这么被判成 i2i 的）。
pub fn classify(
    classes: &[String],
    outputs: &[Output],
    image_inputs: usize,
    has_mask: bool,
) -> Task {
    let set: HashSet<&str> = classes.iter().map(String::as_str).collect();
    let has_image_out = outputs.iter().any(|o| o.kind == "image");
    if outputs.iter().any(|o| o.kind == "audio")
        || classes
            .iter()
            .any(|c| AUDIO_HINTS.iter().any(|hint| c.contains(hint)))
    {
        return Task::Audio;
    }
    // 视频：**输出是事实**；只有"有视频节点 + 一个图像输出都没有"才用节点名兜底。
    // 这一条是刻意加的：MiniMax H3 这类视频模型也有"只出一张图"的工作流，
    // 光看节点名会把出图的工作流判成 video，模型就会按"要等视频"去用。
    if outputs.iter().any(|o| o.kind == "video")
        || (!has_image_out
            && classes
                .iter()
                .any(|c| VIDEO_HINTS.iter().any(|hint| c.contains(hint))))
    {
        return Task::Video;
    }
    if set.iter().any(|c| EDIT_NODES.contains(c)) {
        return Task::Edit;
    }
    if has_mask {
        return Task::Inpaint;
    }
    let has_sampler = classes.iter().any(|c| looks_like_sampler(c));
    if !has_sampler && set.iter().any(|c| UPSCALE_NODES.contains(c)) {
        return Task::Upscale;
    }
    if image_inputs >= 1 {
        return Task::I2i;
    }
    if !outputs.is_empty() {
        return Task::T2i;
    }
    Task::Unknown
}

fn output_kind(class_type: &str) -> Option<&'static str> {
    OUTPUT_KINDS
        .iter()
        .find(|(cls, _)| *cls == class_type)
        .map(|(_, kind)| *kind)
}

fn knob_kind(class_type: &str, input_name: &str, value: &str) -> KnobKind {
    // 叫 `text` / `prompt` 的输入一律当提示词 —— **不看节点类**：
    // 自定义编辑节点（`Krea2EditGroundedEncode.text`）的正文也在这种输入里，
    // 只按类名认的话这些工作流的提示词旋钮会**一个都列不出来**（真机探针照出来的）。
    if input_name == "text" || input_name == "prompt" {
        return KnobKind::Prompt;
    }
    if PROMPT_CLASSES.contains(&class_type)
        && ["string", "value", "positive", "negative"].contains(&input_name)
    {
        return KnobKind::Prompt;
    }
    if IMAGE_INPUTS.contains(&input_name) && looks_like_file_name(value) {
        return KnobKind::Image;
    }
    if SAMPLING_INPUTS.contains(&input_name) {
        return KnobKind::Sampling;
    }
    if SEED_INPUTS.contains(&input_name) || input_name.starts_with("seed") {
        return KnobKind::Seed;
    }
    if SIZE_INPUTS.contains(&input_name) {
        return KnobKind::Size;
    }
    if looks_like_loader(class_type) {
        return KnobKind::Loader;
    }
    KnobKind::Other
}

// ---------------------------------------------------------------------------
//  JSON 小工具（Kotlin 那边的 `JsonPrimitive.contentOrNull` 口径）
// ---------------------------------------------------------------------------

/// `JsonPrimitive.contentOrNull`：字符串 / 数字 / 布尔给字面量文本，`null` 与对象/数组给 `None`。
fn prim_content(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn strings_json(values: &[String]) -> Value {
    Value::Array(values.iter().map(|v| json!(v)).collect())
}

fn distinct(values: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in values {
        if !out.contains(v) {
            out.push(v.clone());
        }
    }
    out
}

/// 按**字符**截断（Kotlin 的 `take(n)` 数的是 UTF-16 单元，对 BMP 里的中文与 ASCII 等价）。
fn take_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

// ===========================================================================
//  二、仓储（`WorkflowRecipeRepo.kt`）
// ===========================================================================

/// 名字上限（与提示词标题同一量级）。
pub const MAX_NAME_CHARS: usize = 200;
/// 备注上限。
pub const MAX_NOTES_CHARS: usize = 2000;
/// 一次最多列几条（给模型看的，不是分页）。
pub const MAX_LIST: usize = 100;
/// 一份配方里 JSON 数组文本的上限（防止模型塞进一张 500 节点的图把库撑爆）。
pub const MAX_JSON_CHARS: usize = 200_000;

/// 进库 / 出库的形状（键名稳定，界面与 AI 都按它取）。
///
/// ⚠ 可空字段一律 `skip_serializing_if`（Kotlin 的 `explicitNulls = false`）——
/// 少了它界面上会多出一堆 `null` 键，`tests/shape_lint.rs` 也会红。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRecipeDto {
    pub id: i64,
    pub name: String,
    /// t2i / i2i / edit / inpaint / upscale / video / audio / unknown
    pub task: String,
    pub skeleton_hash: String,
    pub recipe_version: i64,
    /// file / prompt / graph
    pub source_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<i64>,
    pub node_count: i64,
    pub class_count: i64,
    pub image_inputs: i64,
    pub has_mask: bool,
    /// false = 这张图转换时有缺口（摘掉了节点 / 有悬空输入），**别当完整骨架用**
    pub complete: bool,
    pub slots: Value,
    pub knobs: Value,
    pub classes: Value,
    pub edges: Value,
    pub outputs: Value,
    pub gaps: Vec<String>,
    pub warnings: Vec<String>,
    /// 已验证绑定（T2）：这一族骨架配这些模型**真跑通过**的记录。
    ///
    /// 元素形如 `{model, role, overrideKey, runKey, at}`，只由 `comfy_submit` 的成功路径回写
    /// （见 [`record_verified`]）。老库里是 `[]` —— "以前的跑没留过证据"不能倒推成"验证过"。
    pub verified: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl WorkflowRecipeDto {
    /// 还原成 [`Recipe`]（`render()` 与"绑定新模型"都要用**带类型**的槽位 / 旋钮）。
    ///
    /// 为什么要还原而不是再存一份 render 文本：两份必然漂移，而漂移的表现是
    /// "界面上看到的配方与 AI 拿到的不一样"（不报错）。已知字段解析不出来时**如实回空**
    /// （一行坏数据不该让整个详情 500），缺口列表里已经带着"转换有缺口"这个事实。
    pub fn to_recipe(&self) -> Recipe {
        let array = |v: &Value| -> Vec<Value> { v.as_array().cloned().unwrap_or_default() };
        Recipe {
            task: Task::parse(&self.task).unwrap_or(Task::Unknown),
            skeleton_hash: self.skeleton_hash.clone(),
            node_classes: array(&self.classes)
                .iter()
                .filter_map(prim_content)
                .collect(),
            edges: array(&self.edges).iter().filter_map(prim_content).collect(),
            slots: array(&self.slots)
                .iter()
                .filter_map(|raw| {
                    let o = raw.as_object()?;
                    Some(Slot {
                        role: o.json_str("role")?,
                        node_id: o.json_str("nodeId")?,
                        class_type: o.json_str("classType")?,
                        input_name: o.json_str("inputName")?,
                        file_name: o.json_str("fileName")?,
                        title: o.json_str("title"),
                        // Kotlin `o.str("roleGuessed") == "true"`：JSON 布尔 true 的 content 就是 "true"
                        role_guessed: o.json_str("roleGuessed").as_deref() == Some("true"),
                    })
                })
                .collect(),
            knobs: array(&self.knobs)
                .iter()
                .filter_map(|raw| {
                    let o = raw.as_object()?;
                    Some(Knob {
                        node_id: o.json_str("nodeId")?,
                        class_type: o.json_str("classType")?,
                        input_name: o.json_str("inputName")?,
                        kind: KnobKind::parse(o.json_str("kind").as_deref())
                            .unwrap_or(KnobKind::Other),
                        value: o.json_str("value").unwrap_or_default(),
                        title: o.json_str("title"),
                    })
                })
                .collect(),
            outputs: array(&self.outputs)
                .iter()
                .filter_map(|raw| {
                    let o = raw.as_object()?;
                    Some(Output {
                        node_id: o.json_str("nodeId")?,
                        class_type: o.json_str("classType")?,
                        kind: o.json_str("kind").unwrap_or_default(),
                    })
                })
                .collect(),
            image_inputs: self.image_inputs.max(0) as usize,
            has_mask: self.has_mask,
            node_count: self.node_count.max(0) as usize,
            knobs_truncated: false,
            warnings: self.warnings.clone(),
        }
    }

    /// 给模型/界面看的正文（与 `comfy_learn_workflow` 当场回的那份**跑的是同一个函数**）。
    pub fn render_text(&self) -> String {
        self.to_recipe().render(Some(&self.name))
    }
}

/// `JsonObject.str(key)`（`(this[key] as? JsonPrimitive)?.contentOrNull`）。
trait JsonStr {
    fn json_str(&self, key: &str) -> Option<String>;
}

impl JsonStr for Map<String, Value> {
    fn json_str(&self, key: &str) -> Option<String> {
        self.get(key).and_then(prim_content)
    }
}

const COLS: &str = "id, name, task, skeleton_hash, recipe_version, source_kind, source_ref, \
     source_path, prompt_id, node_count, class_count, image_inputs, has_mask, complete, slots_json, \
     knobs_json, classes_json, edges_json, outputs_json, warnings_json, gaps_json, verified_json, \
     notes, created_at, updated_at";

/// 坏 JSON 按空数组处理：一条配方的一格坏掉不该让整个列表 500。
fn json_array(raw: Option<String>) -> Value {
    let Some(raw) = raw else {
        return Value::Array(Vec::new());
    };
    if raw.trim().is_empty() {
        return Value::Array(Vec::new());
    }
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Array(items)) => Value::Array(items),
        _ => Value::Array(Vec::new()),
    }
}

fn json_string_list(raw: Option<String>) -> Vec<String> {
    match json_array(raw) {
        Value::Array(items) => items.iter().filter_map(prim_content).collect(),
        _ => Vec::new(),
    }
}

fn row_to_dto(row: &Row<'_>) -> rusqlite::Result<WorkflowRecipeDto> {
    let created: Option<String> = row.get("created_at")?;
    let updated: Option<String> = row.get("updated_at")?;
    Ok(WorkflowRecipeDto {
        id: row.get("id")?,
        name: row.get::<_, Option<String>>("name")?.unwrap_or_default(),
        task: row
            .get::<_, Option<String>>("task")?
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string()),
        skeleton_hash: row
            .get::<_, Option<String>>("skeleton_hash")?
            .unwrap_or_default(),
        recipe_version: row
            .get::<_, Option<i64>>("recipe_version")?
            .unwrap_or(VERSION),
        source_kind: row
            .get::<_, Option<String>>("source_kind")?
            .filter(|k| !k.trim().is_empty())
            .unwrap_or_else(|| "file".to_string()),
        source_ref: row.get("source_ref")?,
        source_path: row.get("source_path")?,
        prompt_id: row.get("prompt_id")?,
        node_count: row.get::<_, Option<i64>>("node_count")?.unwrap_or(0),
        class_count: row.get::<_, Option<i64>>("class_count")?.unwrap_or(0),
        image_inputs: row.get::<_, Option<i64>>("image_inputs")?.unwrap_or(0),
        has_mask: row.get::<_, Option<i64>>("has_mask")?.unwrap_or(0) != 0,
        // Kotlin `boolOr("complete", true)`：老库里 NULL 当"完整"
        complete: row.get::<_, Option<i64>>("complete")?.unwrap_or(1) != 0,
        slots: json_array(row.get("slots_json")?),
        knobs: json_array(row.get("knobs_json")?),
        classes: json_array(row.get("classes_json")?),
        edges: json_array(row.get("edges_json")?),
        outputs: json_array(row.get("outputs_json")?),
        gaps: json_string_list(row.get("gaps_json")?),
        warnings: json_string_list(row.get("warnings_json")?),
        verified: json_array(row.get("verified_json")?),
        notes: row.get("notes")?,
        created_at: iso_time(created.as_deref()),
        updated_at: iso_time(updated.as_deref()),
    })
}

/// 列配方。
///
/// * `q`：关键字 —— 名字 / 骨架指纹 / 源路径 / 模型文件名（JSON 文本里搜）
/// * `task`：任务过滤（`t2i` / `edit` …），空白 = 不过滤（**先 trim + lowercase**）
/// * `skeleton_hash`：只看某一族（同一骨架）
/// * `only_complete`：`true` = 只列"没有转换缺口"的（默认 **false**：缺件的也要看得见，只是带上标记）
pub fn list(
    conn: &Connection,
    q: Option<&str>,
    task: Option<&str>,
    skeleton_hash: Option<&str>,
    only_complete: bool,
    limit: usize,
) -> rusqlite::Result<Vec<WorkflowRecipeDto>> {
    let mut where_clause: Vec<String> = Vec::new();
    let mut params_vec: Vec<Value> = Vec::new();
    if let Some(keyword) = q.map(str::trim).filter(|s| !s.is_empty()) {
        let like = format!("%{keyword}%");
        where_clause.push(
            "(name LIKE ? OR skeleton_hash LIKE ? OR source_ref LIKE ? OR source_path LIKE ? \
             OR slots_json LIKE ? OR notes LIKE ?)"
                .to_string(),
        );
        for _ in 0..6 {
            params_vec.push(json!(like));
        }
    }
    if let Some(task) = task.map(str::trim).filter(|s| !s.is_empty()) {
        where_clause.push("task = ?".to_string());
        params_vec.push(json!(task.to_lowercase()));
    }
    if let Some(hash) = skeleton_hash.map(str::trim).filter(|s| !s.is_empty()) {
        where_clause.push("skeleton_hash = ?".to_string());
        params_vec.push(json!(hash));
    }
    if only_complete {
        where_clause.push("complete = 1".to_string());
    }
    let mut sql = format!("SELECT {COLS} FROM workflow_recipes");
    if !where_clause.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_clause.join(" AND "));
    }
    sql.push_str(" ORDER BY updated_at DESC, id DESC LIMIT ?");
    params_vec.push(json!(limit.clamp(1, 500) as i64));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(params_vec.iter().map(sql_param)),
        row_to_dto,
    )?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// `serde_json::Value` → `rusqlite` 参数（LIST 的参数全是字符串 / 数字）。
fn sql_param(value: &Value) -> Box<dyn rusqlite::ToSql> {
    match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Box::new(i)
            } else {
                Box::new(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::Null => Box::new(Option::<String>::None),
        Value::Bool(b) => Box::new(if *b { 1i64 } else { 0i64 }),
        other => Box::new(prim_content(other).unwrap_or_default()),
    }
}

pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Option<WorkflowRecipeDto>> {
    conn.query_row(
        &format!("SELECT {COLS} FROM workflow_recipes WHERE id = ?1"),
        params![id],
        row_to_dto,
    )
    .optional()
}

pub fn get_by_source_ref(
    conn: &Connection,
    source_ref: &str,
) -> rusqlite::Result<Option<WorkflowRecipeDto>> {
    conn.query_row(
        &format!("SELECT {COLS} FROM workflow_recipes WHERE source_ref = ?1"),
        params![source_ref],
        row_to_dto,
    )
    .optional()
}

/// 同一族（同一骨架）的配方有几条 —— "这条骨架我学过几份"。
///
/// 不返回 `GROUP BY` 的大表：给模型看的是"有几条 + 那几条是谁"，
/// 而列出一族时本来就会按 `skeletonHash` 过滤。
pub fn count_by_skeleton(conn: &Connection, skeleton_hash: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM workflow_recipes WHERE skeleton_hash = ?1",
        params![skeleton_hash],
        |row| row.get(0),
    )
}

/// 进库用的字段（**不含** id / 时间戳：那两个由库定）。
#[derive(Debug, Clone, Default)]
pub struct Write {
    pub name: String,
    pub task: String,
    pub skeleton_hash: String,
    pub recipe_version: i64,
    pub source_kind: String,
    pub source_ref: Option<String>,
    pub source_path: Option<String>,
    pub prompt_id: Option<i64>,
    pub node_count: i64,
    pub class_count: i64,
    pub image_inputs: i64,
    pub has_mask: bool,
    pub complete: bool,
    pub slots_json: String,
    pub knobs_json: String,
    pub classes_json: String,
    pub edges_json: String,
    pub outputs_json: String,
    pub warnings_json: String,
    pub gaps_json: String,
    pub notes: Option<String>,
}

/// 幂等保存（同一份 `source_ref` 只有一行）。返回 `(id, created)`。
///
/// 用"先查后 UPDATE/INSERT"（与 Kotlin 同形）：**不是** `INSERT OR REPLACE`（R3，
/// 它会点亮 DELETE 触发器往 `_sync_log` 写假 delete），而且语义上也对 ——
/// 重新学一份工作流应该是"把这一行更新成刚学到的样子"，行 id 保持不变
/// （界面上的链接不会失效）。
pub fn save_row(conn: &Connection, w: &Write) -> Result<(i64, bool), WriteError> {
    let trimmed = w.name.trim();
    let name = if trimmed.is_empty() {
        "未命名工作流".to_string()
    } else {
        trimmed.to_string()
    };
    let name_len = name.chars().count();
    if name_len > MAX_NAME_CHARS {
        return Err(WriteError::Validation(format!(
            "配方名太长（{name_len} > {MAX_NAME_CHARS} 字符）"
        )));
    }
    if w.task.trim().is_empty() {
        return Err(WriteError::Validation(
            "task 不能为空（认不出就写 unknown，**不要**编一个）".to_string(),
        ));
    }
    if w.skeleton_hash.trim().is_empty() {
        return Err(WriteError::Validation("骨架指纹不能为空".to_string()));
    }
    for (what, value) in [
        ("slotsJson", &w.slots_json),
        ("knobsJson", &w.knobs_json),
        ("classesJson", &w.classes_json),
        ("edgesJson", &w.edges_json),
        ("outputsJson", &w.outputs_json),
        ("warningsJson", &w.warnings_json),
        ("gapsJson", &w.gaps_json),
    ] {
        let len = value.chars().count();
        if len > MAX_JSON_CHARS {
            return Err(WriteError::Validation(format!(
                "{what} 太大（{len} > {MAX_JSON_CHARS} 字符）"
            )));
        }
    }
    let notes = w
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string);
    if let Some(notes) = &notes {
        let len = notes.chars().count();
        if len > MAX_NOTES_CHARS {
            return Err(WriteError::Validation(format!(
                "备注太长（> {MAX_NOTES_CHARS} 字符）"
            )));
        }
    }

    // 有 prompt_id 时先确认它真的在（外键会拦，但那样报出来的是 500 + SQL 原文）
    if let Some(prompt_id) = w.prompt_id {
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM prompts WHERE id = ?1",
                params![prompt_id],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(WriteError::Validation(format!(
                "promptId={prompt_id} 在库里不存在（配方指向它没有意义）"
            )));
        }
    }

    let existing = match &w.source_ref {
        Some(reference) => get_by_source_ref(conn, reference)?,
        None => None,
    };
    if let Some(existing) = existing {
        conn.execute(
            "UPDATE workflow_recipes SET name=?, task=?, skeleton_hash=?, recipe_version=?, \
             source_kind=?, source_path=?, prompt_id=?, node_count=?, class_count=?, image_inputs=?, \
             has_mask=?, complete=?, slots_json=?, knobs_json=?, classes_json=?, edges_json=?, \
             outputs_json=?, warnings_json=?, gaps_json=?, notes=? WHERE id=?",
            params![
                name,
                w.task,
                w.skeleton_hash,
                w.recipe_version,
                w.source_kind,
                w.source_path,
                w.prompt_id,
                w.node_count,
                w.class_count,
                w.image_inputs,
                if w.has_mask { 1 } else { 0 },
                if w.complete { 1 } else { 0 },
                w.slots_json,
                w.knobs_json,
                w.classes_json,
                w.edges_json,
                w.outputs_json,
                w.warnings_json,
                w.gaps_json,
                notes,
                existing.id,
            ],
        )?;
        return Ok((existing.id, false));
    }

    conn.execute(
        "INSERT INTO workflow_recipes (name, task, skeleton_hash, recipe_version, source_kind, \
         source_ref, source_path, prompt_id, node_count, class_count, image_inputs, has_mask, \
         complete, slots_json, knobs_json, classes_json, edges_json, outputs_json, warnings_json, \
         gaps_json, notes) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        params![
            name,
            w.task,
            w.skeleton_hash,
            w.recipe_version,
            w.source_kind,
            w.source_ref,
            w.source_path,
            w.prompt_id,
            w.node_count,
            w.class_count,
            w.image_inputs,
            if w.has_mask { 1 } else { 0 },
            if w.complete { 1 } else { 0 },
            w.slots_json,
            w.knobs_json,
            w.classes_json,
            w.edges_json,
            w.outputs_json,
            w.warnings_json,
            w.gaps_json,
            notes,
        ],
    )?;
    Ok((conn.last_insert_rowid(), true))
}

pub fn delete(conn: &Connection, id: i64) -> Result<bool, WriteError> {
    Ok(conn.execute("DELETE FROM workflow_recipes WHERE id = ?1", params![id])? > 0)
}

/// [`write_of`] 的入参（Kotlin 那边是一串具名参数）。
pub struct WriteOf<'a> {
    pub name: &'a str,
    pub source_kind: &'a str,
    pub source_ref: Option<&'a str>,
    pub source_path: Option<&'a str>,
    pub prompt_id: Option<i64>,
    pub complete: bool,
    pub gaps: &'a [String],
    pub extra_warnings: &'a [String],
    pub notes: Option<&'a str>,
}

/// 把一份 [`Recipe`] 摊成 [`Write`] 要的那几个字段（JSON 片段一律由 `Recipe::to_json()` 现取）。
pub fn write_of(recipe: &Recipe, args: WriteOf<'_>) -> Write {
    let json = recipe.to_json();
    let encode = |key: &str| -> String {
        json.get(key)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "[]".to_string())
    };
    let warnings: Vec<String> = recipe
        .warnings
        .iter()
        .cloned()
        .chain(args.extra_warnings.iter().cloned())
        .collect();
    Write {
        name: args.name.to_string(),
        task: recipe.task.wire().to_string(),
        skeleton_hash: recipe.skeleton_hash.clone(),
        recipe_version: VERSION,
        source_kind: args.source_kind.to_string(),
        source_ref: args.source_ref.map(str::to_string),
        source_path: args.source_path.map(str::to_string),
        prompt_id: args.prompt_id,
        node_count: recipe.node_count as i64,
        class_count: recipe.class_count() as i64,
        image_inputs: recipe.image_inputs as i64,
        has_mask: recipe.has_mask,
        complete: args.complete,
        // 槽位 / 旋钮 / 输出是**对象数组**，按 JSON 原文存；类清单与边是**字符串数组**
        slots_json: encode("slots"),
        knobs_json: encode("knobs"),
        classes_json: encode("nodeClasses"),
        edges_json: encode("edges"),
        outputs_json: encode("outputs"),
        warnings_json: strings_json(&warnings).to_string(),
        gaps_json: strings_json(args.gaps).to_string(),
        notes: args.notes.map(str::to_string),
    }
}

// ===========================================================================
//  三、学习（`WorkflowRecipeLearner.kt`）
// ===========================================================================

/// 一份工作流文件最大读多少（与 [`crate::workflow::workflow_load::MAX_WORKFLOW_FILE_BYTES`] 同一档）。
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// 缺口清单最多列几条（工具结果会被截断，列表本身也不该淹掉结论）。
pub const MAX_GAPS: usize = 40;

/// 带出去的转换警告最多几条。
pub const MAX_CONVERT_WARNINGS: usize = 5;

/// 学习 / 绑定这条路上的失败。
///
/// ⚠ **有意与 Kotlin 不同**（2026-09-27）：分两档是**照 Kotlin 的 StatusPages 分的** ——
/// `AiException` 那一支回 `{error: 稳定码, detail: 人话}`，而仓储的 `require` 抛的
/// `IllegalArgumentException` 回 `{error: 人话}`（没有 detail）。合成一档就会让界面
/// 少一个人话、或者把一句人话显示成机器码。工具那一侧同样分两档（见 [`Self::to_tool_failure`]）。
#[derive(Debug, Clone)]
pub enum RecipeFailure {
    /// AI 域的稳定码（Kotlin `AiException`）。
    Ai { code: String, message: String },
    /// 仓储的 `require` 失败（400，`error` 就是那句人话）。
    Plain(String),
}

impl RecipeFailure {
    fn ai(code: &str, message: impl Into<String>) -> Self {
        RecipeFailure::Ai {
            code: code.to_string(),
            message: message.into(),
        }
    }

    fn plain(message: impl Into<String>) -> Self {
        RecipeFailure::Plain(message.into())
    }

    /// 端点那一侧：AI 错误给码 + detail，参数错误给一句人话（**与 Kotlin 逐字同形**）。
    pub fn to_api_error(&self) -> ApiError {
        match self {
            RecipeFailure::Ai { code, message } => ApiError::ai(code.clone(), message.clone()),
            RecipeFailure::Plain(message) => ApiError::bad_request(message.clone()),
        }
    }

    /// 工具那一侧。
    ///
    /// ⚠ **有意与 Kotlin 不同**（2026-09-27）：仓储的 400 在 Kotlin 里会落到工具包装层的
    /// 兜底分支（`TOOL_ERROR` + "工具执行失败：IllegalArgumentException（…）"），
    /// 这里按本仓库既有约定（第 46 步 `character_cards` 的 `api_failure`）翻成
    /// `INVALID_ARGUMENT` —— **人话一个字不变**，只是码更能让模型知道"是我参数给错了"。
    pub fn to_tool_failure(&self) -> ToolFailure {
        match self {
            RecipeFailure::Ai { code, message } => ToolFailure::new(code, message.clone()),
            RecipeFailure::Plain(message) => ToolFailure::new("INVALID_ARGUMENT", message.clone()),
        }
    }
}

/// 依赖（全是从外面注入的**读**入口 —— 这一层不碰策略、不碰网络）。
///
/// Kotlin 的 `objectInfo` 是个 lambda（每次调用现问一次）；Rust 侧换成**一个字段**：
/// `/object_info` 必须在 `async` 的那一层先取好（[`object_info_of`]），
/// 而 Kotlin 的每一次 `analyze` 也只取一次 —— 行为一致，少一层闭包。
/// 「拿到 / 拿不到」这件事本身就是 `Option`，`None` 的语义与 Kotlin 的 `null` 逐字相同：
/// 内置那张表仍然用，并在 `warnings` 里如实说"自定义加载器的槽位可能漏报"。
/// 三个回调的签名（字段一多，`Box<dyn Fn…>` 直接写在字段里读不动）。
type ApiGraphOf<'a> = Box<dyn Fn(i64) -> Option<Map<String, Value>> + Send + Sync + 'a>;
type PromptNameOf<'a> = Box<dyn Fn(i64) -> Option<String> + Send + Sync + 'a>;
type PromptIdByRunKey<'a> = Box<dyn Fn(&str) -> Option<i64> + Send + Sync + 'a>;

pub struct Deps<'a> {
    pub object_info: Option<Map<String, Value>>,
    pub api_graph_of: ApiGraphOf<'a>,
    pub prompt_name: PromptNameOf<'a>,
    pub find_prompt_id_by_run_key: PromptIdByRunKey<'a>,
}

impl<'a> Deps<'a> {
    /// 从运行中的 `state` 组装（工具与端点**共用这一份**）。
    pub fn for_state(state: &'a AppState, object_info: Option<Map<String, Value>>) -> Self {
        Self {
            object_info,
            api_graph_of: Box::new(move |prompt_id| {
                crate::comfy::capture::prompt_api_graph(state, prompt_id)
                    .ok()
                    .flatten()
                    .and_then(|v| v.as_object().cloned())
            }),
            prompt_name: Box::new(move |prompt_id| prompt_title(state, prompt_id)),
            find_prompt_id_by_run_key: Box::new(move |run_key| {
                crate::comfy::capture::find_run(state, run_key)
                    .ok()
                    .flatten()
                    .and_then(|run| run.prompt_id)
            }),
        }
    }
}

/// 取一次 ComfyUI 的 `/object_info`（拿不到回 `None`）。
///
/// 取法与 `comfy_fit_workflow` / `comfy_env_report` **同一份**（先看配置里的地址，再问一次）：
/// 两处各写一份就会出现"同一台机器，两个工具看到两份节点定义"。
///
/// 结果**过 [`crate::workflow::object_info_cache`]**：`/object_info` 是兆字节级响应，
/// 每开一次蓝图就重拉一遍会让"打开蓝图"比 ComfyUI 自己慢一个量级（用户 2026-10-18）。
/// 返回值是 `Arc`，调用方**不要再拷一份**。
pub async fn object_info_of(state: &AppState) -> Option<Arc<Map<String, Value>>> {
    let config = crate::comfy::capture::capture_config(state).ok()?;
    let base = config.comfy_url.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return None;
    }
    // 命中就立刻回（只加引用计数）—— "开蓝图不再等一两秒"的全部秘密。
    if let Some(hit) = state.object_info.get(&base) {
        return Some(hit);
    }
    let value = crate::comfy::comfy_client::get_json_with_timeout(
        &format!("{base}/object_info"),
        OBJECT_INFO_TIMEOUT_SECS,
    )
    .await
    .ok()?;
    // 用 `if let` 把 map **移动**出来。写成 `as_object().cloned()` 会白拷整棵树
    // （兆字节级、逐节点重新分配），正好把缓存省下来的时间还回去。
    let Value::Object(map) = value else {
        return None;
    };
    let map = Arc::new(map);
    // **只缓存成功**：拿不到就让下次重试（ComfyUI 可能刚起来，负缓存会让人以为它没开）
    state.object_info.put(&base, Arc::clone(&map));
    Some(map)
}

/// 取 `/object_info` 的超时。
///
/// 比通用的 20 秒短得多：**拿不到就退回"只做结构检查"**，而不是把界面卡 20 秒。
/// 蓝图在没有节点定义时照样能画、能报结构问题，只是覆盖面小一圈（界面已如实标注）。
const OBJECT_INFO_TIMEOUT_SECS: u64 = 8;

/// 按 id 取提示词标题（给配方起名用）。
fn prompt_title(state: &AppState, prompt_id: i64) -> Option<String> {
    state
        .with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT title FROM prompts WHERE id = ?1",
                    params![prompt_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten())
        })
        .ok()
        .flatten()
}

/// 一次学习请求（工具 / 端点把参数摊平到这里）。
///
/// `path` 进这一层时是**用户给的原文**（准入在 [`learn_and_save`] 那一步做，
/// 这一层不碰策略、也不读环境变量）。
#[derive(Debug, Clone, Default)]
pub struct LearnArgs {
    pub path: Option<String>,
    pub prompt_id: Option<i64>,
    pub graph: Option<Map<String, Value>>,
    pub name: Option<String>,
    pub tolerate_unsupported: bool,
    pub emulate_frontend: bool,
    pub notes: Option<String>,
}

/// 学习结果（还没落库）。
#[derive(Debug, Clone)]
pub struct Learned {
    pub recipe: Recipe,
    pub name: String,
    pub source_kind: String,
    pub source_ref: Option<String>,
    pub source_path: Option<String>,
    pub prompt_id: Option<i64>,
    /// false = 这张图转换时有缺口（见模块头那条纪律）
    pub complete: bool,
    pub gaps: Vec<String>,
    /// 格式：`ui` / `api`
    pub format: String,
    /// 归纳本身的警告 + 转换过程中的警告
    pub warnings: Vec<String>,
}

impl Learned {
    pub fn to_json(&self, id: Option<i64>) -> Value {
        let mut o = Map::new();
        if let Some(id) = id {
            o.insert("recipeId".into(), json!(id));
        }
        o.insert("name".into(), json!(self.name));
        o.insert("task".into(), json!(self.recipe.task.wire()));
        o.insert("skeletonHash".into(), json!(self.recipe.skeleton_hash));
        o.insert("complete".into(), json!(self.complete));
        o.insert("format".into(), json!(self.format));
        o.insert("sourceKind".into(), json!(self.source_kind));
        if let Some(reference) = &self.source_ref {
            o.insert("sourceRef".into(), json!(reference));
        }
        if let Some(path) = &self.source_path {
            o.insert("sourcePath".into(), json!(path));
        }
        if let Some(prompt_id) = self.prompt_id {
            o.insert("promptId".into(), json!(prompt_id));
        }
        o.insert("nodeCount".into(), json!(self.recipe.node_count));
        o.insert("slotCount".into(), json!(self.recipe.slots.len()));
        o.insert("knobCount".into(), json!(self.recipe.knobs.len()));
        o.insert("gaps".into(), strings_json(&self.gaps));
        o.insert("render".into(), json!(self.recipe.render(Some(&self.name))));
        o.insert("recipe".into(), self.recipe.to_json());
        Value::Object(o)
    }
}

/// 从本机工作流文件学（界面格式要 `/object_info` 才转得动）。
pub fn learn_file(
    path: &Path,
    name: Option<&str>,
    tolerate_unsupported: bool,
    emulate_frontend: bool,
    deps: &Deps,
) -> Result<Learned, RecipeFailure> {
    if !path.is_file() {
        return Err(RecipeFailure::ai(
            "NOT_FOUND",
            format!("不是文件：{}", path.display()),
        ));
    }
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Err(RecipeFailure::ai(
            "EMPTY_FILE",
            format!("文件是空的：{}", path.display()),
        ));
    }
    if size > MAX_FILE_BYTES {
        return Err(RecipeFailure::ai(
            "TOO_LARGE",
            format!(
                "工作流文件太大（{size} 字节 > {MAX_FILE_BYTES}）：{}",
                path.display()
            ),
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| {
        RecipeFailure::ai(
            "NOT_FOUND",
            format!("读不了文件（{e}）：{}", path.display()),
        )
    })?;
    let root: Map<String, Value> = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .ok_or_else(|| {
            RecipeFailure::ai(
                "NOT_A_WORKFLOW",
                format!(
                    "这份文件不是 ComfyUI 工作流 JSON（解析失败）：{}。\
                     需要 ComfyUI 保存的工作流文件（含 nodes/links），或「导出（API）」得到的节点图。",
                    path.display()
                ),
            )
        })?;

    let api_format = crate::workflow::workflow_convert::is_node_graph(&root);
    let ui_format = !api_format && crate::workflow::workflow_convert::is_ui_workflow(&root);
    if !api_format && !ui_format {
        return Err(RecipeFailure::ai(
            "NOT_A_WORKFLOW",
            format!(
                "这份 JSON 既不是 API 格式节点图，也不是界面格式工作流（没有 nodes / links）：{}",
                path.display()
            ),
        ));
    }

    let run_key = format!("file:{}", sha256_hex(&bytes));
    let mut gaps: Vec<String> = Vec::new();
    let mut convert_warnings: Vec<String> = Vec::new();
    // `/object_info` 只取一次：转换与"现算自定义加载器"用的是同一份
    // （两次 HTTP 取到的可能是两个版本，而"槽位少了几个"正是这种不一致的后果）
    let info = deps.object_info.as_ref();
    let graph: Map<String, Value> = if api_format {
        root.clone()
    } else {
        let Some(info2) = info else {
            return Err(RecipeFailure::ai(
                "COMFY_UNREACHABLE",
                "这是界面格式工作流，归纳它需要问 ComfyUI 要节点定义（/object_info），但连不上。\
                 先 `comfy_get_status` 看 ComfyUI 在不在跑；或者在 ComfyUI 里用「工作流菜单 → 导出（API）」\
                 存一份 API 格式的文件，再把那份路径给我。",
            ));
        };
        let converted =
            crate::workflow::workflow_convert::to_api_graph(&root, info2, emulate_frontend)
                .map_err(|e| {
                    RecipeFailure::ai(
                        "INVALID_ARGUMENT",
                        format!("转换工作流失败：{}", take_chars(&e, 400)),
                    )
                })?;
        gaps = distinct(
            &converted
                .unsupported_nodes
                .iter()
                .map(|class| format!("摘掉节点类：{class}"))
                .chain(
                    converted
                        .unresolved_inputs
                        .iter()
                        .map(|input| format!("悬空输入：{input}")),
                )
                .chain(
                    converted
                        .required_open_inputs
                        .iter()
                        .map(|input| format!("空着的必填输入：{input}")),
                )
                .collect::<Vec<String>>(),
        )
        .into_iter()
        .take(MAX_GAPS)
        .collect();
        convert_warnings = converted
            .warnings
            .iter()
            .take(MAX_CONVERT_WARNINGS)
            .cloned()
            .collect();
        if !gaps.is_empty() && !tolerate_unsupported {
            return Err(RecipeFailure::ai(
                "UNSUPPORTED_NODES",
                unsupported_nodes_message(&gaps),
            ));
        }
        if converted.graph.is_empty() {
            return Err(RecipeFailure::ai(
                "NO_API_GRAPH",
                format!("转换之后一个可提交的节点都没有：{}", path.display()),
            ));
        }
        converted.graph
    };

    let name2 = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            take_chars(&stem, MAX_NAME_CHARS)
        });
    let learned = analyze_learned(
        &graph,
        name2,
        "file",
        Some(run_key.clone()),
        Some(path.display().to_string()),
        info,
    );
    let mut warnings = learned.warnings.clone();
    for warning in convert_warnings {
        // Kotlin `(learned.warnings + convertWarnings).distinct()`：去重保序
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }
    Ok(Learned {
        prompt_id: (deps.find_prompt_id_by_run_key)(&run_key),
        complete: gaps.is_empty(),
        gaps,
        format: if ui_format {
            "ui".to_string()
        } else {
            "api".to_string()
        },
        warnings,
        ..learned
    })
}

/// `UNSUPPORTED_NODES` 那条"四个出口"的人话（**逐字**照抄 Kotlin，一个字都不许省：
/// 它是模型在"本机实例没装那个节点包"这个现场里唯一的指路牌）。
fn unsupported_nodes_message(gaps: &[String]) -> String {
    let listed = gaps
        .iter()
        .map(|gap| format!("  · {gap}"))
        .collect::<Vec<String>>()
        .join("\n");
    format!(
        "这份工作流转成可提交的节点图时**有缺口**，直接学下来会得到一条缺了链的骨架（而不会报错），\
         所以先停下：\n{listed}\n\
         先确认**本机这个实例**有没有这些节点类（`GET /object_info/<类名>`）：\
         如果它们是你自己装过的自定义节点（Krea2 / rgthree 这类），\
         那多半是**这个实例没装 / 没启用那个节点包**，或者实例的模型根没挂全 —— \
         **不是工作流坏了**，也不是让你去改它。\n\
         四个出口：① 让用户在 ComfyUI 里启停一次 / 换用装了那些节点的实例，再学一次；\
         ② 在 ComfyUI 里「导出（API）」，把那份 API 格式文件给我（零转换）；\
         ③ 确实只想学个大概：带 `tolerateUnsupported=true` 再来一次 —— \
         我会照样学，但配方上标 `complete=false` 与这些缺口，**不许**拿它当完整骨架用；\
         ④ 换一条不用那些节点的工作流。"
    )
}

/// 从库里已有的提示词学（那份 API 图在 `capture_runs.raw` 里）。
pub fn learn_prompt(
    prompt_id: i64,
    name: Option<&str>,
    deps: &Deps,
) -> Result<Learned, RecipeFailure> {
    let graph = (deps.api_graph_of)(prompt_id).ok_or_else(|| {
        RecipeFailure::ai(
            "NO_API_GRAPH",
            format!(
                "库里 id={prompt_id} 这条提示词没有 API 格式节点图（老数据 / 只导入了图片），学不了。\
                 可以换一条（`comfy_find_workflow` 里 runnable=true 的），或者把它的工作流文件路径给我。"
            ),
        )
    })?;
    let name2 = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .or_else(|| (deps.prompt_name)(prompt_id).filter(|n| !n.trim().is_empty()))
        .unwrap_or_else(|| format!("prompt-{prompt_id}"));
    Ok(Learned {
        prompt_id: Some(prompt_id),
        format: "api".to_string(),
        ..analyze_learned(
            &graph,
            take_chars(&name2, MAX_NAME_CHARS),
            "prompt",
            Some(format!("prompt:{prompt_id}")),
            None,
            deps.object_info.as_ref(),
        )
    })
}

/// 直接给一张 API 节点图（模型从别处复制来的）。
pub fn learn_graph(
    graph: &Map<String, Value>,
    name: Option<&str>,
    deps: &Deps,
) -> Result<Learned, RecipeFailure> {
    if !crate::workflow::workflow_convert::is_node_graph(graph) {
        return Err(RecipeFailure::ai(
            "INVALID_ARGUMENT",
            "graph 不是一份 API 格式节点图（应当形如 \
             {\"1\":{\"class_type\":\"KSampler\",\"inputs\":{…}}}）。界面格式（nodes/links）请给文件路径。",
        ));
    }
    let compact = serde_json::to_string(graph).unwrap_or_default();
    let reference = format!("graph:{}", sha256_hex(compact.as_bytes()));
    let name2 = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "节点图".to_string());
    Ok(Learned {
        format: "api".to_string(),
        ..analyze_learned(
            graph,
            take_chars(&name2, MAX_NAME_CHARS),
            "graph",
            Some(reference),
            None,
            deps.object_info.as_ref(),
        )
    })
}

/// 按参数分发（工具与端点共用；少一处分头写就少一处漂移）。
///
/// 三种来源**必须给且只给一个**：都给时会按 path → promptId → graph 的顺序取第一个 ——
/// 与 `comfy_submit`（promptId / workflowPath 二选一）同一个取向：**不猜调用方的意图**，
/// 而是按固定顺序取，并在返回值里带上 `source_kind` 让它自己看得见用的是哪一个。
pub fn learn(args: &LearnArgs, deps: &Deps) -> Result<Learned, RecipeFailure> {
    if let Some(path) = &args.path {
        return learn_file(
            Path::new(path),
            args.name.as_deref(),
            args.tolerate_unsupported,
            args.emulate_frontend,
            deps,
        );
    }
    if let Some(prompt_id) = args.prompt_id {
        return learn_prompt(prompt_id, args.name.as_deref(), deps);
    }
    if let Some(graph) = &args.graph {
        return learn_graph(graph, args.name.as_deref(), deps);
    }
    Err(RecipeFailure::ai(
        "INVALID_ARGUMENT",
        "要么给 path（本机工作流文件），要么给 promptId（库里已有 API 图的提示词），\
         要么给 graph（API 格式节点图）。",
    ))
}

/// 归纳这一步（三个入口共用；`complete` / `gaps` / `format` 由调用方覆盖）。
fn analyze_learned(
    graph: &Map<String, Value>,
    name: String,
    source_kind: &str,
    source_ref: Option<String>,
    source_path: Option<String>,
    info: Option<&Map<String, Value>>,
) -> Learned {
    let model_inputs = crate::comfy::comfy_models::enumerate_model_inputs(info);
    let model_inputs = if model_inputs.is_empty() {
        None
    } else {
        Some(&model_inputs)
    };
    let recipe = analyze(graph, model_inputs);
    Learned {
        warnings: recipe.warnings.clone(),
        recipe,
        name,
        source_kind: source_kind.to_string(),
        source_ref,
        source_path,
        prompt_id: None,
        complete: true,
        gaps: Vec::new(),
        format: "api".to_string(),
    }
}

/// 学 + 落库（工具与端点**共用这一个**）。返回 `(id, learned)`。
///
/// `args.path` 进来时是用户给的原文，**准入只在这一个地方做**（[`ToolPolicy::resolve_read`]，
/// 与 `comfy_load_workflow` 同一条白名单）—— 界面与 AI 都不能借这个入口读白名单以外的文件。
pub async fn learn_and_save(
    state: &AppState,
    policy: &ToolPolicy,
    mut args: LearnArgs,
) -> Result<(i64, Learned), RecipeFailure> {
    if let Some(raw) = args.path.take() {
        let path = policy
            .resolve_read(&raw)
            .map_err(|failure| RecipeFailure::ai(&failure.code, failure.message))?;
        args.path = Some(path.display().to_string());
    }
    // 这一层要**拥有一份**（`Deps` 借用它到异步结束），所以显式解开 Arc 拷一次；
    // 只有"学配方"这条冷路径会付这个拷贝，蓝图那条热路径直接吃 `Arc`。
    let object_info = object_info_of(state).await.map(|info| (*info).clone());
    let deps = Deps::for_state(state, object_info);
    let learned = learn(&args, &deps)?;
    let extra: Vec<String> = learned
        .warnings
        .iter()
        .filter(|warning| !learned.recipe.warnings.contains(warning))
        .cloned()
        .collect();
    let write = write_of(
        &learned.recipe,
        WriteOf {
            name: &learned.name,
            source_kind: &learned.source_kind,
            source_ref: learned.source_ref.as_deref(),
            source_path: learned.source_path.as_deref(),
            prompt_id: learned.prompt_id,
            complete: learned.complete,
            gaps: &learned.gaps,
            extra_warnings: &extra,
            notes: args.notes.as_deref(),
        },
    );
    let id = state
        .write(|conn| save_row(conn, &write))
        .map(|(id, _created)| id)
        .map_err(|error| RecipeFailure::plain(error.human().to_string()))?;
    Ok((id, learned))
}

/// 文件内容的 sha256（与 `comfy_load_workflow` 的 `run_key`、`GET /api/capture/workflows`
/// 的 `inLibrary` 判定**同一把钥匙**）。
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ===========================================================================
//  三·五、已验证绑定（T2，2026-11-09）
// ===========================================================================

/// 一份配方最多留几条验证记录。
///
/// 同一族骨架可能试过很多模型 / 很多槽位组合；这只是一份"哪个配哪个跑通过"的凭据，
/// 不是审计日志（真正的账在 `capture_runs` 与 `prompts`）。给模型看的列表最多带几条，
/// 所以留 40 条足够，也不会把一列长文本撑爆。
pub const MAX_VERIFIED: usize = 40;

/// 一条"这一族骨架配这个模型（这组槽位覆盖）**真跑通过**"的记录。
///
/// `workflow_recipes.verified_json` 的元素形状。**只由 `comfy_submit` 的成功路径回写**：
/// 失败 / 超时 / 取消绝不记 —— 那会把"试过"冒充成"跑通过"，而床上场景正是靠这一档
/// 决定"能不能直接开跑"（`docs/workflow-auto-select-plan.md` §1.2 第 ② 档）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    /// 绑上去的模型文件名（这次实际生效的那个值）
    pub model: String,
    /// 槽位角色（`diffusion_models` / `loras` …；`override_key` 才是准的，角色只帮人认）
    pub role: String,
    /// `comfy_submit(overrides=…)` 的键（`节点id.输入名`）
    pub override_key: String,
    /// 跑通那一次的 `capture_runs.run_key`（= ComfyUI 的 prompt_id）
    pub run_key: String,
    /// 跑通的时间（ISO-8601）；解析不出来就不出现
    pub at: Option<String>,
}

impl Verified {
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("model".into(), json!(self.model));
        m.insert("role".into(), json!(self.role));
        m.insert("overrideKey".into(), json!(self.override_key));
        m.insert("runKey".into(), json!(self.run_key));
        if let Some(at) = &self.at {
            m.insert("at".into(), json!(at));
        }
        Value::Object(m)
    }

    /// 从库里的 JSON 还原；缺 `overrideKey` 的一律**跳过**（没有键就说不出"哪个槽位验证过"）。
    pub fn from_json(raw: &Value) -> Option<Verified> {
        let o = raw.as_object()?;
        let model = o.json_str("model")?.trim().to_string();
        let override_key = o.json_str("overrideKey")?.trim().to_string();
        if model.is_empty() || override_key.is_empty() {
            return None;
        }
        Some(Verified {
            model,
            role: o.json_str("role").unwrap_or_default(),
            override_key,
            run_key: o.json_str("runKey").unwrap_or_default(),
            at: o.json_str("at"),
        })
    }

    /// 这段验证说的是不是"这个槽位配这个文件"。
    ///
    /// 模型名按 [`crate::comfy::comfy_models::ModelName::same`] 比（词根集合相等）——
    /// 与绑定那条判据**同一把尺子**：不然"我明明绑的同一个文件，它却说没验证过"。
    pub fn matches(&self, override_key: &str, model: &str) -> bool {
        self.override_key == override_key
            && crate::comfy::comfy_models::ModelName::same(&self.model, model)
    }
}

/// 把库里的 `verified_json`（已经解析成 `Value`）摊成结构体列表。
pub fn verified_of(value: &Value) -> Vec<Verified> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Verified::from_json).collect())
        .unwrap_or_default()
}

/// 读一条配方的已验证列表（绑定与列表两条路共用）。
pub fn verified_in(conn: &Connection, recipe_id: i64) -> rusqlite::Result<Vec<Verified>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT verified_json FROM workflow_recipes WHERE id = ?1",
            params![recipe_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    Ok(verified_of(&json_array(raw)))
}

/// 回写"已验证"（`comfy_submit` 的成功路径**唯一**的写入口）。
///
/// 三条纪律：
///  * **按 `(overrideKey, 模型名)` 去重**：同一个槽位配同一个文件跑第二遍只刷新 `runKey` / `at`，
///    不会一行一行堆起来；换了模型才是新的一条。
///  * **新的排在最前**（最近的验证最有用），只用 [`MAX_VERIFIED`] 条。
///  * **只增不改**别的槽位 / 别的模型 —— 用户跑一条别的组合不该抹掉之前的结论。
///
/// 返回真正写进去几条（0 = 没有新东西，配置也就不用写库）。
pub fn record_verified(
    conn: &Connection,
    recipe_id: i64,
    entries: &[Verified],
) -> rusqlite::Result<usize> {
    if entries.is_empty() {
        return Ok(0);
    }
    let existing_raw: Option<String> = conn
        .query_row(
            "SELECT verified_json FROM workflow_recipes WHERE id = ?1",
            params![recipe_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let mut list: Vec<Verified> = verified_of(&json_array(existing_raw));
    let mut written = 0usize;
    // **倒着插到最前**：`entries` 里靠前的在结果里也靠前（同一次提交里多个槽位时保持稳定）
    for entry in entries.iter().rev() {
        if entry.override_key.trim().is_empty() || entry.model.trim().is_empty() {
            continue;
        }
        if let Some(found) = list
            .iter_mut()
            .find(|item| item.matches(&entry.override_key, &entry.model))
        {
            // 同一个组合再跑通一次：只刷新凭据，不新增一行
            *found = entry.clone();
        } else {
            list.insert(0, entry.clone());
        }
        written += 1;
    }
    if written == 0 {
        return Ok(0);
    }
    list.truncate(MAX_VERIFIED);
    let encoded = Value::Array(list.iter().map(Verified::to_json).collect()).to_string();
    conn.execute(
        "UPDATE workflow_recipes SET verified_json = ?1 WHERE id = ?2",
        params![encoded, recipe_id],
    )?;
    Ok(written)
}

/// 给模型/界面看的一句话：这条配方在哪些模型上跑通过。
///
/// 空列表返回 `None`（**不出现**这一行，而不是写"没验证过"—— 那会把"没记录"说成"试过失败"）。
pub fn verified_summary(verified: &[Verified]) -> Option<String> {
    if verified.is_empty() {
        return None;
    }
    let parts: Vec<String> = verified
        .iter()
        .take(6)
        .map(|item| {
            let at = item
                .at
                .as_deref()
                .map(|at| format!("，{at}"))
                .unwrap_or_default();
            format!("{}={}{at}", item.override_key, item.model)
        })
        .collect();
    let more = if verified.len() > 6 {
        format!("（共 {} 条）", verified.len())
    } else {
        String::new()
    };
    Some(format!("已验证跑通：{}{more}", parts.join("、")))
}

// ===========================================================================
//  四、绑定（`WorkflowRecipeBinder.kt`）
// ===========================================================================

/// 一条候选最多给几个（与 [`crate::comfy::comfy_models::MAX_CANDIDATES`] 同量级）。
pub const MAX_CANDIDATES: usize = 3;
/// 报告里最多带几条旋钮键（提示词 / 采样 / 尺寸）。
pub const MAX_KNOB_HINTS: usize = 12;

/// 一次绑定请求：`file_name` 必填，定位槽位用 `override_key`（最准）或 `role`（按角色）。
///
/// 两者都不给时按**清单里的目录**推角色（"这个文件在哪类目录里，就绑这一类槽位"）——
/// 推不出来就如实报 [`Status::NotInRecipe`] 并列出槽位清单（**不猜**）。
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub file_name: String,
    pub role: Option<String>,
    pub override_key: Option<String>,
}

/// 一条绑定的结论（四档 + 三档"写不出来"的，一种都不许含糊）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 本机有同名文件**且当前实例能加载** —— 唯一可以自动绑的一档。
    Exact,
    /// 盘上有同名，但**这个实例枚举不到**（模型根没挂全）：先重启 / 修挂载，**别下载**。
    OnDiskOnly,
    /// 本机有更接近的（字面相似度 ≥ 阈值）：**先问用户**（那是他的模型）。
    Substitute,
    /// 本机没有：去 `find_model` / `civitai_search` 查，**不许编文件名**。
    Missing,
    /// 拿不到本机模型清单，没验证过：只有 `force=true` 才写（如实标"没验证"）。
    Unchecked,
    /// 这个名字**不是模型本身**，而是 ComfyUI / 工具链写在旁边的 sidecar。
    ///
    /// 真机探针（2026-09-26）抓到的：磁盘兜底清单里赫然列着 `anima-base-v1.0.metadata.json`
    /// —— 那是给同名 `.safetensors` 写的元数据。绑定时必须挡住：把一个 `.metadata.json`
    /// 写进 `ckpt_name`，ComfyUI 只会报"文件不存在"，而模型**明明就在盘上**。
    NotAModel,
    /// 这条配方里压根没有这个角色 / 这个键：报出它的槽位清单。
    NotInRecipe,
    /// 同一个角色有多个槽位（两条 LoRA），说不清绑哪个：让调用方点明 `override_key`。
    Ambiguous,
}

impl Status {
    pub fn wire(self) -> &'static str {
        match self {
            Status::Exact => "exact",
            Status::OnDiskOnly => "onDiskOnly",
            Status::Substitute => "substitute",
            Status::Missing => "missing",
            Status::Unchecked => "unchecked",
            Status::NotAModel => "notAModel",
            Status::NotInRecipe => "notInRecipe",
            Status::Ambiguous => "ambiguous",
        }
    }
}

/// 一条绑定的结论。
#[derive(Debug, Clone)]
pub struct Binding {
    pub override_key: Option<String>,
    pub role: Option<String>,
    pub from: Option<String>,
    pub to: String,
    pub status: Status,
    /// 人话：为什么是这个结论、下一步该做什么
    pub note: String,
    pub candidates: Vec<String>,
    /// 这个槽位配这个文件**已经跑通过**（`workflow_recipes.verified_json` 里有记录）。
    ///
    /// 它是"经验"不是"保证"：验证的是那个骨架 / 那次提交的图，不是"这一次一定成"。
    /// 所以只在文案与排序上用它，**不改变**能不能自动写覆盖键的判据（那仍然只有 `exact`）。
    pub verified: bool,
    /// 跑通那一次的时间（有记录时才有）
    pub verified_at: Option<String>,
}

impl Binding {
    /// 这条绑定能不能直接写进 overrides。
    pub fn applied(&self) -> bool {
        self.override_key.is_some()
    }
}

/// 一次绑定的报告（`WorkflowRecipeBinder.Report`）。
#[derive(Debug, Clone)]
pub struct Report {
    pub recipe_id: Option<i64>,
    pub recipe_name: String,
    pub task: String,
    pub skeleton_hash: String,
    /// 整条配方是不是"可以直接跑"（所有请求都 EXACT，且没有缺件）
    pub ready: bool,
    pub overrides: Vec<(String, String)>,
    pub bindings: Vec<Binding>,
    /// 没被这次请求动过的槽位（保持工作流里的原值）
    pub untouched: Vec<String>,
    /// 可覆盖的旋钮键（提示词 / 采样 / 尺寸），给模型拼下一次提交用
    pub knob_hints: Vec<String>,
    pub hints: Vec<String>,
    pub warnings: Vec<String>,
}

impl Report {
    pub fn to_json(&self) -> Value {
        let mut o = Map::new();
        if let Some(id) = self.recipe_id {
            o.insert("recipeId".into(), json!(id));
        }
        o.insert("recipeName".into(), json!(self.recipe_name));
        o.insert("task".into(), json!(self.task));
        o.insert("skeletonHash".into(), json!(self.skeleton_hash));
        o.insert("ready".into(), json!(self.ready));
        let mut overrides = Map::new();
        for (key, value) in &self.overrides {
            overrides.insert(key.clone(), json!(value));
        }
        o.insert("overrides".into(), Value::Object(overrides));
        let bindings: Vec<Value> = self
            .bindings
            .iter()
            .map(|b| {
                let mut m = Map::new();
                if let Some(key) = &b.override_key {
                    m.insert("overrideKey".into(), json!(key));
                }
                if let Some(role) = &b.role {
                    m.insert("role".into(), json!(role));
                }
                if let Some(from) = &b.from {
                    m.insert("from".into(), json!(from));
                }
                m.insert("to".into(), json!(b.to));
                m.insert("status".into(), json!(b.status.wire()));
                m.insert("applied".into(), json!(b.applied()));
                m.insert("verified".into(), json!(b.verified));
                if let Some(at) = &b.verified_at {
                    m.insert("verifiedAt".into(), json!(at));
                }
                m.insert("note".into(), json!(b.note));
                if !b.candidates.is_empty() {
                    m.insert("candidates".into(), strings_json(&b.candidates));
                }
                Value::Object(m)
            })
            .collect();
        o.insert("bindings".into(), Value::Array(bindings));
        o.insert("untouched".into(), strings_json(&self.untouched));
        o.insert("knobHints".into(), strings_json(&self.knob_hints));
        o.insert("hints".into(), strings_json(&self.hints));
        o.insert("warnings".into(), strings_json(&self.warnings));
        Value::Object(o)
    }

    /// 给模型看的正文（与 JSON 说的是同一件事，只是排成人话）。
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("【把配方绑到模型上】");
        out.push_str(&self.recipe_name);
        out.push_str("　任务：");
        out.push_str(&self.task);
        out.push_str("　骨架：");
        out.push_str(&self.skeleton_hash);
        out.push_str(if self.ready {
            "　→ 可以直接跑\n"
        } else {
            "　→ **还不能直接跑**\n"
        });
        for binding in &self.bindings {
            out.push_str("  · ");
            if binding.verified {
                // 已验证跑通的排在最前面读起来才顺：结论（能信到什么程度）放在绑定结果之前
                out.push_str("✔ 已验证跑通　");
            }
            out.push_str(&binding.to);
            out.push_str(" [");
            out.push_str(binding.status.wire());
            out.push_str("] ");
            if let Some(key) = &binding.override_key {
                out.push_str("→ ");
                out.push_str(key);
                out.push(' ');
            }
            if let Some(role) = &binding.role {
                out.push('（');
                out.push_str(role);
                out.push('）');
            }
            out.push('：');
            out.push_str(&binding.note);
            if let Some(at) = &binding.verified_at {
                out.push_str(&format!("（上次跑通：{at}）"));
            }
            out.push('\n');
            if !binding.candidates.is_empty() {
                out.push_str("      候选：");
                out.push_str(&binding.candidates.join("、"));
                out.push('\n');
            }
        }
        if !self.overrides.is_empty() {
            out.push_str("要写进 comfy_submit 的 overrides：");
            let parts: Vec<String> = self
                .overrides
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            out.push_str(&parts.join("、"));
            out.push('\n');
        } else {
            out.push_str("这次**一个键都不能改**（结论见上）\n");
        }
        if !self.untouched.is_empty() {
            out.push_str("没动的槽位：");
            out.push_str(&self.untouched.join("、"));
            out.push('\n');
        }
        if !self.knob_hints.is_empty() {
            out.push_str("还能覆盖的旋钮：");
            out.push_str(&self.knob_hints.join("、"));
            out.push('\n');
        }
        for hint in &self.hints {
            out.push_str("→ ");
            out.push_str(hint);
            out.push('\n');
        }
        for warning in &self.warnings {
            out.push_str("⚠ ");
            out.push_str(warning);
            out.push('\n');
        }
        out.trim_end().to_string()
    }
}

/// 绑一次（**只报不改**）。
///
/// `inventory`：本机模型清单；`None` = 拿不到（结论一律 [`Status::Unchecked`]，
/// 只有 `force` 才写键）。`force`：调用方**指名的文件**是否照写。
/// `verified`：这条配方的已验证记录（[`Verified`]）—— 只影响文案与提示，
/// **不改**"能不能自动写覆盖键"的判据（那仍然只有 `exact`）。
pub fn bind(
    recipe: &Recipe,
    requests: &[Request],
    inventory: Option<&crate::comfy::comfy_models::Inventory>,
    recipe_id: Option<i64>,
    recipe_name: &str,
    complete: bool,
    force: bool,
    verified: &[Verified],
) -> Report {
    let mut bindings: Vec<Binding> = Vec::new();
    let mut overrides: Vec<(String, String)> = Vec::new();
    let mut used_keys: HashSet<String> = HashSet::new();
    let mut warnings: Vec<String> = Vec::new();

    // "这个槽位配这个文件跑通过没有" —— 一条小查询，五次以内的线性查找不值得建索引
    let hit = |key: Option<&str>, file: &str| -> Option<Verified> {
        let key = key?;
        verified
            .iter()
            .find(|item| item.matches(key, file))
            .cloned()
    };

    for request in requests {
        match resolve_slot(recipe, request, inventory) {
            SlotLookup::NotFound(note) => bindings.push(Binding {
                override_key: None,
                role: request.role.clone(),
                from: None,
                to: request.file_name.clone(),
                status: Status::NotInRecipe,
                note,
                candidates: Vec::new(),
                verified: false,
                verified_at: None,
            }),
            SlotLookup::Ambiguous(role, keys) => bindings.push(Binding {
                override_key: None,
                role: Some(role),
                from: None,
                to: request.file_name.clone(),
                status: Status::Ambiguous,
                note: format!(
                    "这个角色在配方里有 {} 个槽位（{}）：说不清该绑哪个 —— 用 overrideKey 点明一个\
                     （例如 overrideKey=\"{}\"），或者分多次绑。**不要**把一个文件写给两个槽位。",
                    keys.len(),
                    keys.join("、"),
                    keys.first().cloned().unwrap_or_default()
                ),
                candidates: Vec::new(),
                verified: false,
                verified_at: None,
            }),
            SlotLookup::One { slot, assumed } => {
                let verdict = verify(&request.file_name, &slot.role, inventory);
                if assumed {
                    warnings.push(format!(
                        "没说是哪一类槽位：我按**底模**那一格（{}）来对。\
                         要绑的是别的角色（LoRA / VAE / 文本编码器）就带 role 或 overrideKey 重来。",
                        slot.override_key()
                    ));
                }
                // 只有两档写得出覆盖键：**同名且实例可见**（唯一能自动绑的一档），
                // 或者调用方显式 `force`（"我知道我在做什么"，最后仍由提交前预检守门）
                let can_apply = verdict.status == Status::Exact || force;
                let proven = hit(Some(&slot.override_key()), &request.file_name);
                let binding = Binding {
                    override_key: if can_apply {
                        Some(slot.override_key())
                    } else {
                        None
                    },
                    role: Some(slot.role.clone()),
                    from: Some(slot.file_name.clone()),
                    to: request.file_name.clone(),
                    status: verdict.status,
                    note: match (&proven, verdict.status) {
                        // 已验证 + 本机可见：这是整条链上最可信的一格，文案要说出来
                        (Some(_), Status::Exact) => {
                            "本机有同名文件，当前实例能加载 —— 而且这个组合**已经跑通过**，\
                             直接绑（只改提示词与 seed 就能开跑）。"
                                .to_string()
                        }
                        _ => verdict.note,
                    },
                    candidates: verdict.candidates,
                    verified: proven.is_some(),
                    verified_at: proven.as_ref().and_then(|item| item.at.clone()),
                };
                if binding.applied() {
                    let key = binding.override_key.clone().unwrap_or_default();
                    match overrides.iter_mut().find(|(k, _)| *k == key) {
                        // Kotlin `overrides[k] = v`：同键后者覆盖前者，**位置留在首次出现处**
                        Some(existing) => existing.1 = request.file_name.clone(),
                        None => overrides.push((key.clone(), request.file_name.clone())),
                    }
                    used_keys.insert(key);
                }
                if verdict.status == Status::OnDiskOnly {
                    warnings.push(format!(
                        "{} 盘上有、但**当前实例枚举不到**：改了同样报文件不存在 —— \
                         先让用户重启 ComfyUI / 修好模型根（**不要下载**）。",
                        request.file_name
                    ));
                }
                if force && verdict.status != Status::Exact && binding.applied() {
                    warnings.push(format!(
                        "{} 写的是**没验证过**的 {}（你要求 force）—— 提交前预检会再查一次能不能加载。",
                        binding.override_key.clone().unwrap_or_default(),
                        request.file_name
                    ));
                }
                bindings.push(binding);
            }
        }
    }

    let untouched: Vec<String> = recipe
        .slots
        .iter()
        .filter(|slot| !used_keys.contains(&slot.override_key()))
        .map(|slot| {
            format!(
                "{}（{}）= {}",
                slot.override_key(),
                slot.role,
                slot.file_name
            )
        })
        .collect();
    let mut knob_hints: Vec<String> = Vec::new();
    for knob in recipe.knobs.iter().filter(|knob| {
        matches!(
            knob.kind,
            KnobKind::Prompt
                | KnobKind::Sampling
                | KnobKind::Seed
                | KnobKind::Size
                | KnobKind::Image
        )
    }) {
        let key = knob.override_key();
        if !knob_hints.contains(&key) {
            knob_hints.push(key);
        }
    }
    knob_hints.truncate(MAX_KNOB_HINTS);

    let ready =
        !bindings.is_empty() && bindings.iter().all(|b| b.status == Status::Exact) && complete;
    let mut hints: Vec<String> = Vec::new();
    if !complete {
        hints.push(
            "这条配方 `complete=false`（学的时候这张图就缺件，见它的 gaps）：\
             **先把缺的节点类补齐**（本机实例装/启用那几个自定义节点）再来绑，别拿缺件的骨架开跑。"
                .to_string(),
        );
    }
    if bindings.iter().any(|b| !b.applied()) {
        hints.push(
            "先按上面的结论处理（问用户 / 重启实例 / 去找模型），**处理完再提交**；\
             现在带着这些键去提交会被提交前预检拦下来。"
                .to_string(),
        );
    }
    if recipe.slots.is_empty() {
        hints.push(
            "这条配方一个模型槽位都没认出来（可能是自定义加载器或纯 CPU 节点）：\
             换一条骨架，或者用 comfy_load_workflow + 手写 overrides。"
                .to_string(),
        );
    }
    let override_hint: Vec<String> = overrides
        .iter()
        .map(|(key, value)| format!("\"{key}\":\"{value}\""))
        .collect();
    // 经验档的措辞（T2）：**跑通过**与**没跑过**要给模型不同的开跑信心 ——
    // 前者就是床上场景"同任务 + 同模型族的最近一次成功"那一档，后者要它先自己确认。
    let applied: Vec<&Binding> = bindings.iter().filter(|b| b.applied()).collect();
    if !applied.is_empty() && applied.iter().all(|b| b.verified) {
        hints.push(
            "这组槽位覆盖**已经跑通过**：直接 comfy_submit 开跑，只改提示词与 seed，不要重搭工作流。"
                .to_string(),
        );
    } else if applied.iter().any(|b| !b.verified) {
        hints.push(
            "这组覆盖还没跑通过（第一跑）：提交前把易错项（尺寸 / 步数 / 提示词风格）自己核一遍；\
             跑通之后会自动记成已验证，下次同模型直接优先。"
                .to_string(),
        );
    }
    hints.push(format!(
        "绑完用 comfy_submit(promptId=…, overrides={{{}}}) 提交；\
         promptId 从 comfy_load_workflow(path=…) 拿（配方里那条源文件与它是同一把钥匙）。",
        override_hint.join("、")
    ));
    if recipe.slots.iter().any(|slot| slot.role_guessed) {
        warnings.push(
            "这条配方里有槽位的**目录名是猜的**（自定义加载器）：覆盖键是准的，目录名别当结论。"
                .to_string(),
        );
    }

    Report {
        recipe_id,
        recipe_name: recipe_name.to_string(),
        task: recipe.task.wire().to_string(),
        skeleton_hash: recipe.skeleton_hash.clone(),
        ready,
        overrides,
        bindings,
        untouched,
        knob_hints,
        hints,
        warnings,
    }
}

/// 槽位查找的三种结果（Kotlin `SlotLookup`）。
enum SlotLookup {
    One { slot: Slot, assumed: bool },
    Ambiguous(String, Vec<String>),
    NotFound(String),
}

/// "底模那一格"：`checkpoints` 或 `diffusion_models`。
///
/// 参数名叫 `model`、用户说的也是"换成这个底模"，所以**没说角色**时按它兜底 ——
/// 但兜底要**说出来**（[`SlotLookup::One`] 的 `assumed` → 报告里一条警告），
/// 不能让人以为"它自己知道我指的是哪一格"。
const BASE_ROLES: [&str; 2] = ["checkpoints", "diffusion_models"];

/// 槽位清单（人话），`NotFound` / `Ambiguous` 的提示里要用。
fn slot_list(recipe: &Recipe) -> String {
    if recipe.slots.is_empty() {
        "（一个都没有）".to_string()
    } else {
        recipe
            .slots
            .iter()
            .map(|slot| {
                format!(
                    "{}（{}）= {}",
                    slot.override_key(),
                    slot.role,
                    slot.file_name
                )
            })
            .collect::<Vec<String>>()
            .join("、")
    }
}

fn resolve_slot(
    recipe: &Recipe,
    request: &Request,
    inventory: Option<&crate::comfy::comfy_models::Inventory>,
) -> SlotLookup {
    if let Some(key) = &request.override_key {
        return match recipe.slots.iter().find(|slot| slot.override_key() == *key) {
            Some(slot) => SlotLookup::One {
                slot: slot.clone(),
                assumed: false,
            },
            None => SlotLookup::NotFound(format!(
                "这条配方里没有 `{key}` 这个槽位。它的槽位是：{}。\
                 （覆盖键写成 `节点id.输入名`；不确定就先用 comfy_list_recipes 看配方正文。）",
                slot_list(recipe)
            )),
        };
    }
    if let Some(role) = &request.role {
        let slots: Vec<&Slot> = recipe
            .slots
            .iter()
            .filter(|slot| slot.role.eq_ignore_ascii_case(role.trim()))
            .collect();
        return match slots.len() {
            0 => SlotLookup::NotFound(format!(
                "这条配方里没有 role={role} 的槽位。它的槽位是：{}。",
                slot_list(recipe)
            )),
            1 => SlotLookup::One {
                slot: slots[0].clone(),
                assumed: false,
            },
            _ => SlotLookup::Ambiguous(
                role.clone(),
                slots.iter().map(|slot| slot.override_key()).collect(),
            ),
        };
    }
    // 两者都没给：按清单里的**目录**推角色（推不出来再退回"底模那一格"）
    let found = inventory.and_then(|inv| {
        inv.models.iter().find(|model| {
            crate::comfy::comfy_models::ModelName::same(&model.name, &request.file_name)
        })
    });
    if let Some(found) = found {
        let slots: Vec<&Slot> = recipe
            .slots
            .iter()
            .filter(|slot| slot.role.eq_ignore_ascii_case(&found.folder))
            .collect();
        return match slots.len() {
            0 => SlotLookup::NotFound(mismatch_note(recipe, &found.folder, &request.file_name)),
            1 => SlotLookup::One {
                slot: slots[0].clone(),
                assumed: false,
            },
            _ => SlotLookup::Ambiguous(
                found.folder.clone(),
                slots.iter().map(|slot| slot.override_key()).collect(),
            ),
        };
    }
    // 清单里没有这个文件（或根本没有清单）：按"底模那一格"兜底 —— 参数名叫 model、
    // 用户说的也是"换成这个底模"。兜底**要标出来**，见 `BASE_ROLES` 的注释。
    let base: Vec<&Slot> = recipe
        .slots
        .iter()
        .filter(|slot| BASE_ROLES.contains(&slot.role.as_str()))
        .collect();
    if base.len() == 1 {
        return SlotLookup::One {
            slot: base[0].clone(),
            assumed: true,
        };
    }
    let roles: Vec<String> = distinct(
        &recipe
            .slots
            .iter()
            .map(|s| s.role.clone())
            .collect::<Vec<_>>(),
    );
    SlotLookup::NotFound(format!(
        "没说绑哪个槽位（既没有 role 也没有 overrideKey），而这个文件名在本机清单里也找不到，\
         所以我不知道它属于哪一类。这条配方的槽位是：{}。\
         要么带上 role（{}），要么带上 overrideKey。",
        slot_list(recipe),
        roles.join(" / ")
    ))
}

/// "目录对不上"时的那句解释（设计 §3.1 情形 B）。
///
/// 最常见的两种：整包 checkpoint ↔ 分离式 unet+clip+vae；LoRA 被放进 checkpoints。
/// 这两种都不是"改个文件名"能解决的，必须让模型知道**该换个方向**。
fn mismatch_note(recipe: &Recipe, requested_folder: &str, _file_name: &str) -> String {
    let roles: Vec<String> = distinct(
        &recipe
            .slots
            .iter()
            .map(|s| s.role.clone())
            .collect::<Vec<_>>(),
    );
    let base = format!(
        "这条配方里没有 `{requested_folder}` 这一类槽位（它有：{}）。",
        roles.join("、")
    );
    let has = |role: &str| roles.iter().any(|r| r == role);
    if requested_folder == "checkpoints" && has("diffusion_models") {
        return format!(
            "{base} 它用的是**分离式加载**（底模 + 文本编码器 + VAE 三个槽位），\
             而你给的是一个**整包 checkpoint** —— 覆盖键换不上去：要真接上得**改写骨架**\
             （增删节点 + 重连线，本轮不做）。两条路：① 找这个模型的**分离式**版本\
             （unet / text encoder / vae 分开的）；② 换一条**用 checkpoint 的**同任务骨架\
             （`comfy_list_recipes(task=…)` 里找）。"
        );
    }
    if requested_folder == "diffusion_models" && has("checkpoints") {
        return format!(
            "{base} 它用的是**整包 checkpoint**，而你给的是一个裸 unet / DiT。\
             分离式加载要自己补文本编码器与 VAE 两个节点（本轮不做骨架改写）：\
             ① 找这个模型的整包版本；② 换一条用 unet 的同任务骨架。"
        );
    }
    if requested_folder == "loras" && !has("loras") {
        return format!(
            "{base} LoRA 只能挂在 LoRA 槽位上：这条骨架没有 LoRA 位置（想加得改骨架）。"
        );
    }
    format!(
        "{base} 换一条有这一类槽位的骨架，或者用 overrideKey 直接指一个已存在的槽位\
         （**不要**把一个 {requested_folder} 的文件写进别的目录的槽位）。"
    )
}

struct Verdict {
    status: Status,
    note: String,
    candidates: Vec<String>,
}

/// 这是不是"模型旁边的元数据"而不是模型本身。
///
/// 只认两种**确定**的形状（宁可漏判，也不要把真模型当成 sidecar 挡掉）：
/// `*.metadata.json`（ComfyUI / 下载器写的同名元数据）与 `model_index.json`
/// （diffusers 那种"目录形态的模型"标记文件）。
fn looks_like_sidecar(name: &str) -> bool {
    let lower = name.to_lowercase().replace('\\', "/");
    let base = lower.rsplit('/').next().unwrap_or(&lower);
    base.ends_with(".metadata.json") || base == "model_index.json"
}

/// 这个文件名在本机能不能用（四档 + 没有清单时的 `unchecked`）。
///
/// 判据与 `comfy_fit_workflow` **同源**：同名且实例可见才算 `exact`；
/// 盘上有同名但实例看不到 → `onDiskOnly`（先重启/修挂载，别下载）。
fn verify(
    file_name: &str,
    role: &str,
    inventory: Option<&crate::comfy::comfy_models::Inventory>,
) -> Verdict {
    if looks_like_sidecar(file_name) {
        return Verdict {
            status: Status::NotAModel,
            note: format!(
                "「{file_name}」看着是 ComfyUI / 下载器写在模型旁边的 **sidecar 元数据**（不是模型本身）：\
                 真正的模型是同名的 `…safetensors` —— 用那个名字。"
            ),
            candidates: Vec::new(),
        };
    }
    let Some(inventory) = inventory else {
        return Verdict {
            status: Status::Unchecked,
            note:
                "拿不到本机模型清单（ComfyUI 没在跑 / 没找到 models 目录），**这个名字没验证过**：\
                   要照写就带 force=true，我会把它当'你指名要的文件'写进覆盖键\
                   （提交前预检还会再查一次）。"
                    .to_string(),
            candidates: Vec::new(),
        };
    };
    // sidecar 一律不进候选（见 `Status::NotAModel` 的注释：它会把真模型顶掉）
    let candidates: Vec<crate::comfy::comfy_models::Scored> = inventory
        .candidates_for(role, file_name)
        .into_iter()
        .filter(|candidate| !looks_like_sidecar(&candidate.model.name))
        .collect();
    let exact_visible = candidates.iter().find(|candidate| {
        crate::comfy::comfy_models::ModelName::same(file_name, &candidate.model.name)
            && candidate.model.visible_to_instance
    });
    if exact_visible.is_some() {
        return Verdict {
            status: Status::Exact,
            note: "本机有同名文件，当前实例能加载 —— 直接绑。".to_string(),
            candidates: Vec::new(),
        };
    }
    let exact_any = candidates.iter().find(|candidate| {
        crate::comfy::comfy_models::ModelName::same(file_name, &candidate.model.name)
    });
    if let Some(hit) = exact_any {
        let where_ = hit
            .model
            .path
            .clone()
            .unwrap_or_else(|| hit.model.name.clone());
        return Verdict {
            status: Status::OnDiskOnly,
            note: format!(
                "盘上有同名文件（{where_}），但**当前这个实例枚举不到它**：\
                 说明实例的模型根没挂全 —— 先重启 ComfyUI / 修好挂载，**不要下载**，\
                 也不要硬写这个键。"
            ),
            candidates: candidates
                .iter()
                .take(MAX_CANDIDATES)
                .map(|candidate| candidate.model.name.clone())
                .collect(),
        };
    }
    if !candidates.is_empty() {
        return Verdict {
            status: Status::Substitute,
            note: "本机没有同名的；下面这些是**字面相似**的候选（相似度只是词根对得上，\
                   **不代表架构兼容**）：换之前**先问用户**（那是他的模型）。"
                .to_string(),
            candidates: candidates
                .iter()
                .take(MAX_CANDIDATES)
                .map(|candidate| {
                    let size = candidate
                        .model
                        .size_gb()
                        .map(|gb| format!("，{gb:.1}GB"))
                        .unwrap_or_default();
                    format!("{}（{:.2}{size}）", candidate.model.name, candidate.score)
                })
                .collect(),
        };
    }
    let near: Vec<String> = inventory
        .near_misses_for(file_name, 3)
        .into_iter()
        .map(|model| model.name)
        .filter(|name| !looks_like_sidecar(name))
        .collect();
    let near_text = if near.is_empty() {
        String::new()
    } else {
        format!("（有几个名字沾边的：{}，但都不够像）", near.join("、"))
    };
    Verdict {
        status: Status::Missing,
        note: format!(
            "本机没有这个文件{near_text}：**绝对不要自己编一个文件名**。先去 `find_model` / \
             `civitai_search` 查它，再用 `ask_user_question` 让用户决定装 / 换骨架 / 放弃。"
        ),
        candidates: near,
    }
}

/// 报告里"这条配方有哪些槽位"的紧凑形状（工具列表用）。
pub fn slots_json(recipe: &Recipe) -> Value {
    Value::Array(
        recipe
            .slots
            .iter()
            .map(|slot| {
                let mut m = Map::new();
                m.insert("overrideKey".into(), json!(slot.override_key()));
                m.insert("role".into(), json!(slot.role));
                m.insert("fileName".into(), json!(slot.file_name));
                m.insert("roleGuessed".into(), json!(slot.role_guessed));
                Value::Object(m)
            })
            .collect(),
    )
}

/// 读一条配方 + 用**本机的**模型清单绑一次（工具与端点**共用这一个**）。
///
/// 清单取法与 `comfy_fit_workflow` / 提交前预检**同一份**（`/object_info` 与盘扫的并集）：
/// 两处判据分家就会出现"fit 说能用、bind 说没有"这种自相矛盾。
pub async fn bind_and_report(
    state: &AppState,
    recipe_id: i64,
    requests: &[Request],
    force: bool,
) -> Result<Option<Report>, RecipeFailure> {
    let row = state
        .with(|conn| get(conn, recipe_id))
        .map_err(|error| RecipeFailure::plain(error.human().to_string()))?;
    let Some(row) = row else { return Ok(None) };
    let object_info = object_info_of(state).await;
    let project_root = state.project_root();
    let desktop = crate::tools::mcp::desktop(None);
    let model_dirs: Vec<std::path::PathBuf> = desktop
        .as_ref()
        .map(|d| d.models_dirs.clone())
        .unwrap_or_default();
    let inventory = crate::comfy::comfy_models::inventory(
        object_info.as_deref(),
        Some(&project_root),
        &model_dirs,
    );
    Ok(Some(bind(
        &row.to_recipe(),
        requests,
        Some(&inventory),
        Some(row.id),
        &row.name,
        row.complete,
        force,
        &verified_of(&row.verified),
    )))
}

// ===========================================================================
//  五、三条工具（`ToolRegistry.kt` 那三条处理体）
// ===========================================================================

/// 参数取值：字符串（空白算没给）。
fn arg_str(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(prim_content)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 参数取值：整数（数字 / 数字字符串都认，`args.int(field)` 的对应物）。
fn arg_i64(arguments: &Value, key: &str) -> Option<i64> {
    arguments.get(key).and_then(|value| match value {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    })
}

/// 参数取值：布尔（`args.bool(field)`：JSON 布尔或**恰好** `"true"` / `"false"`）。
fn arg_bool(arguments: &Value, key: &str) -> Option<bool> {
    match arguments.get(key)? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// `comfy_learn_workflow`：把一份工作流学成配方并存进本机配方库。
///
/// 与 `POST /api/ai/workflow-recipes` **共用** [`learn_and_save`]（"只有一份实现"）。
pub async fn tool_learn_workflow(
    state: &AppState,
    policy: &ToolPolicy,
    arguments: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args = LearnArgs {
        // 路径准入在 `learn_and_save` 里做（Run 开始时快照的策略，见 AGENTS §10）
        path: arguments
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_string),
        prompt_id: arg_i64(arguments, "promptId"),
        graph: arguments.get("graph").and_then(Value::as_object).cloned(),
        name: arg_str(arguments, "name"),
        tolerate_unsupported: arg_bool(arguments, "tolerateUnsupported").unwrap_or(false),
        emulate_frontend: arg_bool(arguments, "emulateFrontend").unwrap_or(true),
        notes: arg_str(arguments, "notes"),
    };
    let (id, learned) = learn_and_save(state, policy, args)
        .await
        .map_err(|failure| failure.to_tool_failure())?;
    Ok(ToolOutcome::new(
        learned.recipe.render(Some(&learned.name)),
        learned.to_json(Some(id)),
    ))
}

/// `comfy_list_recipes`：列已经学过的配方（**只读**）。
pub fn tool_list_recipes(state: &AppState, arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let q = arg_str(arguments, "q");
    let task = arg_str(arguments, "task");
    let skeleton = arg_str(arguments, "skeleton");
    let only_complete = arg_bool(arguments, "onlyComplete").unwrap_or(false);
    let limit = arg_i64(arguments, "limit").unwrap_or(20).clamp(1, 100) as usize;
    let rows = state
        .with(|conn| {
            list(
                conn,
                q.as_deref(),
                task.as_deref(),
                skeleton.as_deref(),
                only_complete,
                limit,
            )
        })
        .map_err(|error| {
            ToolFailure::new("TOOL_ERROR", format!("数据库错误：{}", error.human()))
        })?;
    if rows.is_empty() {
        return Err(ToolFailure::new(
            "NOT_FOUND",
            format!(
                "还没有学过任何配方{}。先 `comfy_list_workflows` 看本机有哪些工作流文件，再用 \
                 comfy_learn_workflow(path=…) 学一份 —— 学过的会留在这里，下次换模型直接绑。",
                if q.is_some() || task.is_some() {
                    "匹配这些条件的"
                } else {
                    ""
                }
            ),
        ));
    }
    let mut text = format!("配方库：命中 {} 条\n", rows.len());
    for row in &rows {
        text.push('\n');
        text.push_str(&format!(
            "── [{}] {}{}\n",
            row.id,
            row.name,
            if row.complete {
                ""
            } else {
                "　⚠ **缺件（complete=false）**"
            }
        ));
        let slots: Vec<Value> = row.slots.as_array().cloned().unwrap_or_default();
        text.push_str(&format!(
            "   {} · 骨架 {} · 节点 {} · 槽位 {} · 来源 {}{}\n",
            row.task,
            row.skeleton_hash,
            row.node_count,
            slots.len(),
            row.source_ref
                .clone()
                .unwrap_or_else(|| row.source_kind.clone()),
            row.prompt_id
                .map(|id| format!(" · promptId={id}"))
                .unwrap_or_default()
        ));
        if !slots.is_empty() {
            let parts: Vec<String> = slots
                .iter()
                .map(|slot| {
                    let o = slot.as_object();
                    let role = o
                        .and_then(|o| o.json_str("role"))
                        .unwrap_or_else(|| "?".to_string());
                    let key = o
                        .and_then(|o| o.json_str("overrideKey"))
                        .unwrap_or_else(|| "?".to_string());
                    let file = o
                        .and_then(|o| o.json_str("fileName"))
                        .unwrap_or_else(|| "?".to_string());
                    format!("{key}({role})={file}")
                })
                .collect();
            text.push_str(&format!("   槽位：{}\n", parts.join("、")));
        }
        if !row.complete && !row.gaps.is_empty() {
            text.push_str(&format!(
                "   缺口：{}\n",
                row.gaps
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<String>>()
                    .join("；")
            ));
        }
        // T2：哪几个（槽位, 模型）跑通过 —— 换模型时它是"这一族配这个模型能不能直接开跑"的依据
        let verified = verified_of(&row.verified);
        if let Some(line) = verified_summary(&verified) {
            text.push_str("   ");
            text.push_str(&line);
            text.push('\n');
        }
        if let Some(path) = &row.source_path {
            text.push_str(&format!("   路径：{path}\n"));
        }
    }
    text.push('\n');
    text.push_str(
        "要换模型就直接 `comfy_bind_recipe(recipeId=<上面的 id>, model=\"新模型.safetensors\")` —— \
         它会算出该写哪几个 overrides，**只报不改**（真提交仍走 comfy_submit）。\n",
    );

    let recipes: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut o = Map::new();
            o.insert("id".into(), json!(row.id));
            o.insert("name".into(), json!(row.name));
            o.insert("task".into(), json!(row.task));
            o.insert("skeletonHash".into(), json!(row.skeleton_hash));
            o.insert("complete".into(), json!(row.complete));
            o.insert("nodeCount".into(), json!(row.node_count));
            o.insert("slots".into(), row.slots.clone());
            o.insert("gaps".into(), strings_json(&row.gaps));
            // 已验证列表原样带出去（键名与库里那几列一致：模型 / 角色 / 覆盖键 / runKey / 时间）
            let verified = verified_of(&row.verified);
            o.insert(
                "verified".into(),
                Value::Array(verified.iter().map(Verified::to_json).collect()),
            );
            if !verified.is_empty() {
                o.insert("verifiedCount".into(), json!(verified.len()));
            }
            if let Some(path) = &row.source_path {
                o.insert("sourcePath".into(), json!(path));
            }
            if let Some(reference) = &row.source_ref {
                o.insert("sourceRef".into(), json!(reference));
            }
            if let Some(prompt_id) = row.prompt_id {
                o.insert("promptId".into(), json!(prompt_id));
            }
            Value::Object(o)
        })
        .collect();
    let mut payload = Map::new();
    payload.insert("count".into(), json!(rows.len()));
    payload.insert("recipes".into(), Value::Array(recipes));
    Ok(ToolOutcome::new(text, Value::Object(payload)))
}

/// `comfy_bind_recipe`：把一条配方绑到一个（新）模型上（**只报不改**）。
pub async fn tool_bind_recipe(
    state: &AppState,
    arguments: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let recipe_id = arg_i64(arguments, "recipeId").ok_or_else(|| {
        ToolFailure::new(
            "INVALID_ARGUMENT",
            "缺少参数 recipeId（先用 comfy_list_recipes 拿）",
        )
    })?;
    let model = arg_str(arguments, "model").ok_or_else(|| {
        ToolFailure::new("INVALID_ARGUMENT", "缺少参数 model（要绑上去的模型文件名）")
    })?;
    let mut requests: Vec<Request> = vec![Request {
        file_name: model,
        role: arg_str(arguments, "role"),
        override_key: arg_str(arguments, "overrideKey"),
    }];
    if let Some(extra) = arguments.get("extra").and_then(Value::as_object) {
        for (role, value) in extra {
            let name = prim_content(value)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    ToolFailure::new(
                        "INVALID_ARGUMENT",
                        format!("extra 里的 {role} 不是字符串文件名"),
                    )
                })?;
            requests.push(Request {
                file_name: name,
                role: Some(role.clone()),
                override_key: None,
            });
        }
    }
    let report = bind_and_report(
        state,
        recipe_id,
        &requests,
        arg_bool(arguments, "force").unwrap_or(false),
    )
    .await
    .map_err(|failure| failure.to_tool_failure())?
    .ok_or_else(|| {
        ToolFailure::new(
            "NOT_FOUND",
            format!(
                "配方库里没有 id={recipe_id}。先用 comfy_list_recipes 看有哪些\
                     （也可以用 comfy_learn_workflow 现学一份）。"
            ),
        )
    })?;
    Ok(ToolOutcome::new(report.render(), report.to_json()))
}

// ===========================================================================
//  六、HTTP 端点（`WorkflowRecipeRoutes.kt`）
// ===========================================================================

/// ```text
///  GET    /api/ai/workflow-recipes           列表（?q= &task= &skeleton= &complete=true &limit=）
///  POST   /api/ai/workflow-recipes           学一份工作流（path / promptId / graph 三选一）
///  GET    /api/ai/workflow-recipes/{id}      详情（含配方正文与 render 文本）
///  POST   /api/ai/workflow-recipes/{id}/bind 把配方绑到一个（新）模型上（**只报不改**）
///  DELETE /api/ai/workflow-recipes/{id}      删除
/// ```
///
/// 路径挂在 `/api/ai/` 下、错误体是 `{error, detail}`（AI 域口径）：读写它的两方都是 AI 域的东西
/// —— 界面（`AiApiClient`，先读 `detail`）与 AI 工具。
///
/// ⚠ 学习与绑定都走 [`learn_and_save`] / [`bind_and_report`]（与 AI 工具**同一份实现**）；
/// 文件路径走 [`ToolPolicy::resolve_read`]（与 `comfy_load_workflow` 同一条准入）。
pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route(
            "/api/ai/workflow-recipes",
            axum::routing::get(list_recipes).post(create_recipe),
        )
        .route(
            "/api/ai/workflow-recipes/{id}",
            axum::routing::get(get_recipe).delete(delete_recipe),
        )
        .route(
            "/api/ai/workflow-recipes/{id}/bind",
            axum::routing::post(bind_recipe),
        )
}

/// `POST /api/ai/workflow-recipes` 的请求体（`WorkflowRecipeLearnInput`）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkflowRecipeLearnInput {
    /// 本机工作流文件路径（界面格式也行 —— 会用 ComfyUI 的 `/object_info` 转）
    pub path: Option<String>,
    /// 或者：库里已有 API 图的提示词 id
    pub prompt_id: Option<i64>,
    /// 或者：直接给一张 API 节点图
    pub graph: Option<Value>,
    pub name: Option<String>,
    /// 转换有缺口时是否照样学（默认 **false = 拒学**）
    pub tolerate_unsupported: bool,
    /// 界面格式转换时是否照 ComfyUI 前端行为等价改写（默认 true，与 `comfy_load_workflow` 一致）。
    ///
    /// ⚠ 它的默认值**必须是 true**（Kotlin `WorkflowRecipeLearnInput` 那条写的就是 `= true`）：
    /// `#[serde(default)]` 对 `bool` 是 `false`，不显式指定就会让"界面没传这个字段"变成
    /// "一个字都不改"——而那与工具那条路（`unwrap_or(true)`）**行为不一致**，且不报错。
    #[serde(default = "yes")]
    pub emulate_frontend: bool,
    pub notes: Option<String>,
}

/// `emulateFrontend` 的缺省值（见上面那条注释：不能靠 `bool` 的 `false`）。
fn yes() -> bool {
    true
}

/// 详情响应：配方本身 + 给模型看的那段正文（一个渲染实现，两处共用）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRecipeDetailDto {
    pub recipe: WorkflowRecipeDto,
    pub render: String,
}

/// 绑定的请求体（与 `comfy_bind_recipe` 工具的参数一一对应）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkflowRecipeBindInput {
    /// 要绑上去的模型文件名
    pub model: String,
    /// 可选：绑到哪一类槽位（不给就按清单里该文件所在的目录推）
    pub role: Option<String>,
    /// 可选：直接点名槽位的覆盖键（最准）
    pub override_key: Option<String>,
    /// 默认 false：没验证过的名字不写进覆盖键
    pub force: bool,
    /// 可选：还要一起绑的其它槽位，键是 role。
    ///
    /// 用 `Map`（`preserve_order` 下是插入序）而不是 `BTreeMap`：Kotlin 那边是
    /// `Map<String, String>`，绑定报告里的 `bindings[]` 顺序照 JSON 里的书写顺序 ——
    /// 排序过一遍就与 Kotlin 的报告不是同一个顺序了（不报错，只是对不上）。
    pub extra: Map<String, Value>,
}

/// 绑定响应：报告 JSON + 给模型/界面看的那段正文。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRecipeBindDto {
    pub recipe_id: i64,
    pub report: Value,
    pub render: String,
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    q: Option<String>,
    task: Option<String>,
    skeleton: Option<String>,
    complete: Option<String>,
    limit: Option<String>,
}

/// 查询参数里的布尔（Kotlin `Parameters.bool`：`1/true/yes` 与 `0/false/no`，大小写不敏感）。
fn query_bool(raw: Option<&str>) -> Option<bool> {
    match raw?.to_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

/// 查询参数里的字符串（空白 = 没给）。
fn query_str(raw: Option<&String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

async fn list_recipes(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Query(query): axum::extract::Query<ListQuery>,
) -> Result<axum::Json<Vec<WorkflowRecipeDto>>, ApiError> {
    // Kotlin `p.int("limit", MAX_LIST)`：解析不出来就用默认值（不报错）
    let limit = query
        .limit
        .as_deref()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(MAX_LIST);
    let rows = state.with(|conn| {
        list(
            conn,
            query_str(query.q.as_ref()).as_deref(),
            query_str(query.task.as_ref()).as_deref(),
            query_str(query.skeleton.as_ref()).as_deref(),
            query_bool(query.complete.as_deref()) == Some(true),
            limit,
        )
    })?;
    Ok(axum::Json(rows))
}

async fn create_recipe(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::Json(input): axum::Json<WorkflowRecipeLearnInput>,
) -> Result<(axum::http::StatusCode, axum::Json<Value>), ApiError> {
    let policy = ToolPolicy::load(&state)?;
    let args = LearnArgs {
        path: input.path,
        prompt_id: input.prompt_id,
        graph: input.graph.as_ref().and_then(Value::as_object).cloned(),
        name: input.name,
        tolerate_unsupported: input.tolerate_unsupported,
        emulate_frontend: input.emulate_frontend,
        notes: input.notes,
    };
    let (id, learned) = learn_and_save(&state, &policy, args)
        .await
        .map_err(|failure| failure.to_api_error())?;
    Ok((
        axum::http::StatusCode::CREATED,
        axum::Json(learned.to_json(Some(id))),
    ))
}

async fn get_recipe(
    axum::extract::State(state): axum::extract::State<AppState>,
    crate::web::routes::Id(id): crate::web::routes::Id,
) -> Result<axum::Json<WorkflowRecipeDetailDto>, ApiError> {
    let row = state
        .with(|conn| get(conn, id))?
        .ok_or_else(|| ApiError::not_found(format!("配方 {id} 不存在")))?;
    // 详情里带上 render 正文：与 comfy_learn_workflow 当场回的那份**跑的是同一个函数**
    let render = row.render_text();
    Ok(axum::Json(WorkflowRecipeDetailDto {
        recipe: row,
        render,
    }))
}

async fn delete_recipe(
    axum::extract::State(state): axum::extract::State<AppState>,
    crate::web::routes::Id(id): crate::web::routes::Id,
) -> Result<axum::Json<Value>, ApiError> {
    let deleted = state.write(|conn| delete(conn, id))?;
    if !deleted {
        return Err(ApiError::not_found(format!("配方 {id} 不存在")));
    }
    let mut out = Map::new();
    out.insert("deleted".into(), json!(id));
    Ok(axum::Json(Value::Object(out)))
}

async fn bind_recipe(
    axum::extract::State(state): axum::extract::State<AppState>,
    crate::web::routes::Id(id): crate::web::routes::Id,
    axum::Json(input): axum::Json<WorkflowRecipeBindInput>,
) -> Result<axum::Json<WorkflowRecipeBindDto>, ApiError> {
    let mut requests: Vec<Request> = vec![Request {
        file_name: input.model.clone(),
        role: input.role.clone(),
        override_key: input.override_key.clone(),
    }];
    for (role, value) in &input.extra {
        // 值必须是字符串文件名（与工具那条判据同一句话：空白 / 对象都不行）
        let name = prim_content(value)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ApiError::ai(
                    "INVALID_ARGUMENT",
                    format!("extra 里的 {role} 不是字符串文件名"),
                )
            })?;
        requests.push(Request {
            file_name: name,
            role: Some(role.clone()),
            override_key: None,
        });
    }
    let report = bind_and_report(&state, id, &requests, input.force)
        .await
        .map_err(|failure| failure.to_api_error())?
        .ok_or_else(|| ApiError::not_found(format!("配方 {id} 不存在")))?;
    let render = report.render();
    Ok(axum::Json(WorkflowRecipeBindDto {
        recipe_id: id,
        report: report.to_json(),
        render,
    }))
}

// ===========================================================================
//  七、测试（`WorkflowRecipeTest.kt` + `WorkflowRecipeRepoTest.kt` +
//  `WorkflowRecipeBinderTest.kt` 的断言逐条移植）
// ===========================================================================

#[cfg(test)]
mod tests;
