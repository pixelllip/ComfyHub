//! 「这个模型去哪儿拿」—— `find_model`（Kotlin `FindModel.kt` + `ToolRegistry.kt` 里那条
//! 处理体与三段渲染的逐条对照）。
//!
//! # 需求原文（`docs/bug and suggestion/bug-and-suggestion-9.22.md` 的 A 表与 K 节）
//!
//! > "加入 modelscope、huggingface 的查询功能，并与 civitai 合并成'定向查询' tool；
//! >  **查到的贴链接，由人工下载**"
//!
//! 决策记录（同文件 D 表第二行）：
//!
//! > | 定向查询 | A：**一条 `find_model`**，内部按 Civitai → HF → ModelScope 顺序查，**逐个报可达性** |
//! > | 能不能代下 | A：**绝不代下**（只给链接 + sha256 + 目标目录）；`download_model` 保持 deny |
//!
//! # 五条不能含糊的纪律
//!
//!  1. **绝不代下**。这个文件里**没有任何**"把模型文件写进磁盘"的代码路径：
//!     [`HttpModelFinder`] 只发**检索**请求，响应体读进内存、取出元数据之后立刻丢掉。
//!  2. **先查本机，再查外网**（顺序纪律）。同名模型已经在本机时，`search` 在 `online`
//!     那一层就返回了 —— **一个外部请求都不发**，并把三个源标成 [`Reachability::Skipped`]。
//!     约束依据：模型要求下载一个**本来就在硬盘上**的
//!     4.18GB 文件；只要有一个环节判断错了，这条路就会重演。
//!  3. **每个源都要报可达性**，哪怕它一条都没返回（"没查到"里有一部分是"没查成"，
//!     两件事必须分开说）。
//!  4. **sha256 明写用途**：它是"手动下载后，判断盘上那份是否同一模型"的依据。
//!     字面相似度**不足以**确认身份（`sd_xl_base_1.0` 与 `sd_xl_refiner_1.0` 能打到 0.63 分
//!     却是两份不同的模型）。上游没给就留空并说明，绝不编一个 64 位十六进制。
//!  5. **只读**。工具声明 `mutating = false`，走 `web` 类别（查询词会离开本机，
//!     与 `web_search` / `civitai_search` 共用同一个联网预算）。
//!
//! # 与 Kotlin 的差异（如实记下，行为上一处不改）
//!
//! * Kotlin 的 `HttpModelFinder.civitai` 开头那道 `civitai ?: Failed(CIVITAI_DISABLED)`
//!   在 Rust 侧的工具路径上**不会发生**：`tool_find_model` 总是构造出 Civitai 客户端
//!   （凭据解不开时它只是"没有登录态"，与 Civitai 自己的语义一致）。所以那一档只留给
//!   单测（`civitai: None`），错误文案照抄。
//! * Kotlin 用 `catch (e: Exception)` 兜住"尚未发起请求即异常"（解析 / 本地 bug）。
//!   Rust 侧所有失败都是 [`ToolFailure`] / [`SourceOutcomeError`]，没有可捕获的裸异常，
//!   所以那两档 `"查询失败（…）：这是本机的处理出了问题"` 没有对应物。
//! * Kotlin 的 `ModelFinder` 是个**窄接口**（单测塞假件）。Rust 用**真 HTTP + 进程内假
//!   服务**来钉同样的事实（"本机命中就一个请求都不发"改成数假服务收到几个请求），
//!   所以这里没有 trait —— 少一个 trait 就少一处"实现与接口漂移"的可能。

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::comfy::comfy_models::{Inventory, LocalModel, ModelName, Source};
use crate::tools::civitai::CivitaiClient;
use crate::tools::tools::{
    java_error_name, outbound_error_kind, str_arg, take_utf16, url_encode, OutboundError,
    ToolFailure, ToolOutcome,
};
use crate::web::routes::AppState;

/// 一次最多给几条候选（跨源合并之后的数字）。
pub const DEFAULT_LIMIT: i64 = 8;
pub const MAX_LIMIT: i64 = 20;

/// 每个源最多问几条（HF 每条还要多打一次目录接口，所以比 [`MAX_LIMIT`] 小得多）。
pub const PER_SOURCE_LIMIT: i64 = 4;

pub const CONNECT_TIMEOUT_SECONDS: u64 = 5;
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 20;
pub const MAX_TIMEOUT_SECONDS: u64 = 60;

/// 认得出来的**权重文件**后缀。
///
/// 与 [`crate::comfy::comfy_models`] 那张表**故意不一样**：那边是"ComfyUI 能加载什么"
/// （含 `.yaml` / `.json`，因为 CLIP 那类需要配置文件），这里是"需要下载的文件大小"——
/// 把 `.json` 当成"模型文件"报到候选里只会让人莫名其妙。
pub const MODEL_EXTS: [&str; 8] = [
    "safetensors",
    "ckpt",
    "pt",
    "pth",
    "bin",
    "gguf",
    "sft",
    "onnx",
];

/// HF 令牌在凭据库里的引用名（受管凭据库 → `HF_TOKEN` 环境变量兜底）。
pub const HF_CREDENTIAL_REF: &str = "huggingface";

pub const HF_BASE_URL: &str = "https://huggingface.co";
pub const MODELSCOPE_BASE_URL: &str = "https://modelscope.cn";
pub const ENV_HF_BASE_URL: &str = "COMFYHUB_HF_BASE_URL";
pub const ENV_MODELSCOPE_BASE_URL: &str = "COMFYHUB_MODELSCOPE_BASE_URL";

/// 结果中"该放哪"的基准目录（相对路径，绝对路径随机器而变）。
pub const INSTALL_ROOT_HINT: &str = "ComfyUI 的 models\\ 目录";

/// 严格的 sha256 形状判定（64 位十六进制）—— 上游结构变了也不会把别的东西当成 hash 报出去。
///
/// ⚠ 这条也被 [`crate::tools::civitai`] 用来判定 `hashes.SHA256`（Kotlin 那边是两份实现，
/// 判据逐字相同；这里只留一份，免得两处漂移）。
pub fn is_sha256(raw: Option<&str>) -> bool {
    match raw {
        Some(value) => value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()),
        None => false,
    }
}

/// 三个模型源，声明顺序即查询顺序（Civitai → HuggingFace → ModelScope）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSource {
    Civitai,
    HuggingFace,
    ModelScope,
}

impl ModelSource {
    pub const ALL: [ModelSource; 3] = [
        ModelSource::Civitai,
        ModelSource::HuggingFace,
        ModelSource::ModelScope,
    ];

    pub fn wire(self) -> &'static str {
        match self {
            ModelSource::Civitai => "civitai",
            ModelSource::HuggingFace => "huggingface",
            ModelSource::ModelScope => "modelscope",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ModelSource::Civitai => "Civitai",
            ModelSource::HuggingFace => "HuggingFace",
            ModelSource::ModelScope => "ModelScope",
        }
    }
}

/// 一个源**这一问**的可达性。
///
/// 报的是"这次这一问"的事实，不是"这台机器永久的状态"：同一个源上一问 200、这一问超时
/// 是完全可能的（代理挂了、对方限流）。所以每条都带一句 detail，而不是只给一个枚举值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reachability {
    Ok,
    NeedsProxy,
    NeedsLogin,
    Gated,
    Error,
    Skipped,
}

impl Reachability {
    pub fn wire(self) -> &'static str {
        match self {
            Reachability::Ok => "ok",
            Reachability::NeedsProxy => "needs_proxy",
            Reachability::NeedsLogin => "needs_login",
            Reachability::Gated => "gated",
            Reachability::Error => "error",
            Reachability::Skipped => "skipped",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Reachability::Ok => "正常",
            Reachability::NeedsProxy => "本机直连不通，要走出网代理（127.0.0.1:7890）",
            Reachability::NeedsLogin => "需要登录 / 令牌",
            Reachability::Gated => "需要在网页上同意条款（gated）",
            Reachability::Error => "查询失败",
            Reachability::Skipped => "本次没去查（先命中了本机）",
        }
    }
}

/// 统一后的模型类型 —— "该放进 `models\` 下哪个目录"就靠它推（[`ModelCandidate::install_into`]）。
///
/// 认不出来时是 [`ModelKind::Unknown`]（**不是** `Checkpoint`）：宁可说"不知道放哪"，
/// 也不推测目录 —— 推测错误的后果是把 13GB 的 LoRA 下载到 `checkpoints\`，
/// 而 ComfyUI 那边照旧看不到它。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    Checkpoint,
    Diffusion,
    Lora,
    Vae,
    Controlnet,
    Embedding,
    Upscaler,
    TextEncoder,
    ClipVision,
    Unknown,
}

impl ModelKind {
    pub fn wire(self) -> &'static str {
        match self {
            ModelKind::Checkpoint => "checkpoint",
            ModelKind::Diffusion => "diffusion",
            ModelKind::Lora => "lora",
            ModelKind::Vae => "vae",
            ModelKind::Controlnet => "controlnet",
            ModelKind::Embedding => "embedding",
            ModelKind::Upscaler => "upscaler",
            ModelKind::TextEncoder => "text_encoder",
            ModelKind::ClipVision => "clip_vision",
            ModelKind::Unknown => "unknown",
        }
    }

    /// 该放进 `models\` 下的哪个目录；`None` = 认不出来，**不猜**。
    pub fn folder(self) -> Option<&'static str> {
        match self {
            ModelKind::Checkpoint => Some("checkpoints"),
            ModelKind::Diffusion => Some("diffusion_models"),
            ModelKind::Lora => Some("loras"),
            ModelKind::Vae => Some("vae"),
            ModelKind::Controlnet => Some("controlnet"),
            ModelKind::Embedding => Some("embeddings"),
            ModelKind::Upscaler => Some("upscale_models"),
            ModelKind::TextEncoder => Some("text_encoders"),
            ModelKind::ClipVision => Some("clip_vision"),
            ModelKind::Unknown => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ModelKind::Checkpoint => "底模 / Checkpoint",
            ModelKind::Diffusion => "扩散模型（裸 UNET / DiT）",
            ModelKind::Lora => "LoRA / LyCORIS",
            ModelKind::Vae => "VAE",
            ModelKind::Controlnet => "ControlNet",
            ModelKind::Embedding => "Textual Inversion（embedding）",
            ModelKind::Upscaler => "放大模型",
            ModelKind::TextEncoder => "文本编码器（CLIP / T5）",
            ModelKind::ClipVision => "CLIP Vision",
            ModelKind::Unknown => "类型认不出来",
        }
    }

    /// Civitai 的 `type` 值（官方拼写，见 [`crate::tools::civitai::TYPES`]）。
    pub fn from_civitai(raw: Option<&str>) -> ModelKind {
        match raw.map(|r| r.trim().to_lowercase()).as_deref() {
            Some("checkpoint") => ModelKind::Checkpoint,
            Some("lora") | Some("locon") | Some("dora") => ModelKind::Lora,
            Some("vae") => ModelKind::Vae,
            Some("controlnet") => ModelKind::Controlnet,
            Some("textualinversion") => ModelKind::Embedding,
            Some("upscaler") => ModelKind::Upscaler,
            _ => ModelKind::Unknown,
        }
    }

    /// HF 的 `pipeline_tag` / ModelScope 的 `Tasks`。
    ///
    /// 只映射**确定**的那几种：这两个源给的是"这个仓库是干什么的"，不是 ComfyUI 那种精确的
    /// 加载器类型。`text-to-image` 落 [`ModelKind::Diffusion`]（实际上更可能是整包 checkpoint）
    /// —— 所以渲染时**额外**看仓库路径里有没有 `lora` / `vae` / `diffusion_models` 这类线索
    /// （见 [`ModelKind::from_path_hint`]），那比 pipeline_tag 准。
    pub fn from_task(raw: Option<&str>) -> ModelKind {
        match raw.map(|r| r.trim().to_lowercase()).as_deref() {
            Some("text-to-image")
            | Some("image-to-image")
            | Some("unconditional-image-generation")
            | Some("text-to-video")
            | Some("image-to-video")
            | Some("any-to-any")
            | Some("diffusers") => ModelKind::Diffusion,
            Some("lora") | Some("peft") => ModelKind::Lora,
            _ => ModelKind::Unknown,
        }
    }

    /// 从仓库名 / 文件路径里的目录名反推该放哪（HF 与 ModelScope 的兜底）。
    ///
    /// 判据是**路径里真的出现了那个目录名**（`split_files/diffusion_models/xxx.safetensors`、
    /// `.../loras/...`），不是猜 —— 认不出来就返回 `None`，让报告说"放哪说不准"。
    ///
    /// ⚠ **仓库名本身也算路径的一段**（调用方把 `repoId + "/" + 文件名` 传进来）。
    /// 实测：`Omnico/Krea2_turbo_diff_loras` 这个仓库里的文件叫 `Moody5_r256.safetensors`
    /// —— 光看文件名只能推 ?，而它明明写着 loras。
    pub fn from_path_hint(path: Option<&str>) -> Option<ModelKind> {
        let segments: Vec<String> = path
            .unwrap_or_default()
            .replace('\\', "/")
            .split('/')
            .map(|s| s.to_lowercase())
            .collect();
        let any = |wanted: &[&str]| segments.iter().any(|s| wanted.contains(&s.as_str()));
        if segments.iter().any(|s| {
            s.split(['-', '_', '.'])
                .any(|part| part == "loras" || part == "lora")
        }) {
            return Some(ModelKind::Lora);
        }
        if any(&["diffusion_models", "unet", "dit"]) {
            return Some(ModelKind::Diffusion);
        }
        if any(&["vae"]) {
            return Some(ModelKind::Vae);
        }
        if any(&["controlnet"]) {
            return Some(ModelKind::Controlnet);
        }
        if any(&["embeddings"]) {
            return Some(ModelKind::Embedding);
        }
        if any(&["upscale_models"]) {
            return Some(ModelKind::Upscaler);
        }
        if any(&["text_encoders"]) {
            return Some(ModelKind::TextEncoder);
        }
        if any(&["clip_vision"]) {
            return Some(ModelKind::ClipVision);
        }
        if any(&["checkpoints"]) {
            return Some(ModelKind::Checkpoint);
        }
        None
    }

    /// 从**文件名本身**反推（比仓库名更硬）。
    ///
    /// 现场（2026-09-22 实测）：`Beinsezii/Krea-2-Turbo-Projector-Scale-LoRA-Diffusers`
    /// 里那个 `<1MB` 的 `pytorch_lora_weights.safetensors` 其实就是个 LoRA ——
    /// 它的文件名是 HuggingFace diffusers 训练脚本的固定产物名，不是模型作者起的。
    pub fn from_file_name(file_name: Option<&str>) -> Option<ModelKind> {
        let base = file_name
            .unwrap_or_default()
            .replace('\\', "/")
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_lowercase();
        if base.contains("lora") {
            return Some(ModelKind::Lora);
        }
        if base.contains("controlnet") || base.contains("control_net") {
            return Some(ModelKind::Controlnet);
        }
        if base.contains("vae") || base == "ae.safetensors" {
            return Some(ModelKind::Vae);
        }
        if base.contains("clip_vision") {
            return Some(ModelKind::ClipVision);
        }
        if base.contains("text_encoder") {
            return Some(ModelKind::TextEncoder);
        }
        if base.contains("embedding") {
            return Some(ModelKind::Embedding);
        }
        None
    }

    /// 把三条线索按可信度排一遍：**文件名 → 路径/仓库名 → 源的 pipeline_tag / Tasks**。
    ///
    /// 顺序不能反：`pipeline_tag=text-to-image` 说的是"这个仓库干什么用"，
    /// 它**说不出**"该放进 ComfyUI 的哪个目录"（diffusion / checkpoint / LoRA 都可能）。
    pub fn infer(file_name: Option<&str>, path: Option<&str>, task: Option<&str>) -> ModelKind {
        ModelKind::from_file_name(file_name)
            .or_else(|| ModelKind::from_path_hint(path))
            .unwrap_or_else(|| ModelKind::from_task(task))
    }

    /// 这个仓库是不是 **diffusers 目录格式**（一堆子目录 + `model_index.json`）。
    ///
    /// 判据来自仓库自己声明的 `library_name` / `tags`，不是我们猜的。为什么要单列：
    /// diffusers 格式**没有**"一个能放进 `diffusion_models\` 的 safetensors" ——
    /// 里面的 `transformer/`、`vae/` 是分片的，而且都是**文件夹**。
    pub fn is_diffusers_format(library_name: Option<&str>, tags: &[String]) -> bool {
        library_name
            .map(|l| l.eq_ignore_ascii_case("diffusers"))
            .unwrap_or(false)
            || tags.iter().any(|t| t.eq_ignore_ascii_case("diffusers"))
    }
}

/// 统一后的一个候选。
///
/// 字段就是 A 表第 9 条点名要能对齐的那几个：
/// 名字 / 类型 / 底模 / 触发词 / 体积 / 直链 / **sha256** / **该放哪个目录** / 来源。
#[derive(Debug, Clone)]
pub struct ModelCandidate {
    pub name: String,
    pub kind: ModelKind,
    /// 底模（Civitai 的 `baseModel` / HF 的 `base_model:*` 标签 / ModelScope 的 `BaseModel`）。
    pub base_model: Option<String>,
    pub trained_words: Vec<String>,
    pub size_bytes: Option<i64>,
    /// 直链（**供手动下载**的 URL）。
    pub download_url: Option<String>,
    /// 模型页（简介 / 许可 / 触发词在那边）。
    pub page_url: Option<String>,
    pub sha256: Option<String>,
    pub file_name: Option<String>,
    /// 这一条的主来源。
    pub source: ModelSource,
    /// 这条候选在哪些源上出现过（去重合并之后的并集，顺序与查询顺序一致）。
    pub sources: Vec<ModelSource>,
    /// 简介（只有 Civitai 会给；别的源留 `None` —— 不编）。
    pub description: Option<String>,
    pub downloads: Option<i64>,
    /// 一句**必须在结果中给出**的提醒（拿不到类型 / 格式不匹配时才有）。
    pub note: Option<String>,
}

impl ModelCandidate {
    /// **应放入哪个目录**（相对于 ComfyUI 的 `models\`）。
    ///
    /// 拿不到类型时是 `None`：工具会明写"该放 `models\` 下哪个子目录说不准，
    /// 由工作流中的加载器判断"——优于推测。
    pub fn install_into(&self) -> Option<&'static str> {
        self.kind.folder()
    }

    /// 给人看的体积（与 [`crate::tools::civitai`] 的 `size_label` 同一套分档）。
    ///
    /// `%.1f` 而不是 `%.0f`：实测有个 `pytorch_lora_weights.safetensors` 只有几 MB，
    /// 用整数会显示为 **"0 MB"**，看起来像"空文件"，会被理解为未获取到内容。
    pub fn size_label(&self) -> Option<String> {
        self.size_bytes.map(|bytes| {
            let gb = bytes as f64 / 1024.0 / 1024.0 / 1024.0;
            if gb >= 1.0 {
                format!("{gb:.2} GB")
            } else {
                format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
            }
        })
    }
}

/// 一个源的产出。**失败也要有位置** —— 这正是"逐个报可达性"那条纪律的实现方式。
#[derive(Debug, Clone)]
pub enum SourceOutcome {
    Ok {
        source: ModelSource,
        candidates: Vec<ModelCandidate>,
        detail: String,
    },
    Failed {
        source: ModelSource,
        state: Reachability,
        detail: String,
    },
}

impl SourceOutcome {
    pub fn source(&self) -> ModelSource {
        match self {
            SourceOutcome::Ok { source, .. } | SourceOutcome::Failed { source, .. } => *source,
        }
    }

    pub fn state(&self) -> Reachability {
        match self {
            SourceOutcome::Ok { .. } => Reachability::Ok,
            SourceOutcome::Failed { state, .. } => *state,
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            SourceOutcome::Ok { detail, .. } | SourceOutcome::Failed { detail, .. } => detail,
        }
    }

    pub fn candidates(&self) -> &[ModelCandidate] {
        match self {
            SourceOutcome::Ok { candidates, .. } => candidates,
            SourceOutcome::Failed { .. } => &[],
        }
    }
}

/// 一次查询的入参。
#[derive(Debug, Clone)]
pub struct ModelQuery {
    pub text: String,
    /// 期望的模型类型（工具据此再写一句"该放哪"）；`None` = 不限。
    ///
    /// 它**不改变查询词**：三个源的检索接口都只吃关键词。
    pub kind: Option<ModelKind>,
    pub limit: i64,
    /// 要不要出网。`false` = **本机优先那条路的产物**。
    pub online: bool,
    /// 本机清单（`/object_info` ∪ 盘扫）—— 刻意由调用方传进来、**不在这里读环境**，
    /// 单测才不会被开发机的真实 ComfyUI 目录影响。
    pub inventory: Option<Inventory>,
}

/// 一次查询的结果（**三个源一条不少**）。
#[derive(Debug)]
pub struct ModelFinderResult {
    pub query: String,
    /// 三个源的产出，顺序固定 = [`ModelSource::ALL`]。
    pub sources: Vec<SourceOutcome>,
    /// 合并去重后的候选（跨源同名的合一条，`sources` 列全）。
    pub candidates: Vec<ModelCandidate>,
    /// 本机命中的那些（本地短路的依据）。
    pub local_matches: Vec<LocalModel>,
    /// 本机"名字沾边但不是一个东西"的那些 —— 用来挡"我明明有这个模型"那类误判。
    pub local_near_misses: Vec<String>,
    /// 本机清单的来源（报告里要能说清"本机清单是哪来的"）。
    pub local_source: Source,
    /// 这次到底出没出网（本地短路时为 false）。
    pub online: bool,
    pub notes: Vec<String>,
}

impl ModelFinderResult {
    /// 本机已经有同名的了 → 工具会照着这条说"不用去下"。
    pub fn local_hit(&self) -> bool {
        !self.local_matches.is_empty()
    }
}

/// 一个源报错时**由它自己说清是什么状态**（而不是让上层去猜代理还是鉴权）。
#[derive(Debug)]
struct SourceOutcomeError {
    state: Reachability,
    detail: String,
}

impl SourceOutcomeError {
    fn new(state: Reachability, detail: impl Into<String>) -> Self {
        Self {
            state,
            detail: detail.into(),
        }
    }
}

/// 各家真实的检索实现（会打 HTTP，全部走出网路由）。
pub struct HttpModelFinder {
    /// Civitai 的客户端**直接复用**已经上线的那一个（[`CivitaiClient`]）——
    /// 域名切换 / 令牌 / 错误映射 / 代理选择都在它里面，这里一行都不重复实现。
    pub civitai: Option<CivitaiClient>,
    /// HF 令牌（gated / 私有仓库要用）；拿不到就是匿名。**绝不写进日志或结果**。
    pub hf_token: Option<String>,
    pub hf_base: String,
    pub ms_base: String,
    pub timeout: Duration,
    /// 每个源最多问几条（[`PER_SOURCE_LIMIT`]）。
    pub per_source: i64,
}

impl HttpModelFinder {
    /// 按环境变量与凭据库造一个（工具路径用的就是它）。
    pub fn from_state(state: &AppState) -> Result<Self, ToolFailure> {
        Ok(Self {
            civitai: Some(CivitaiClient::from_env(state)?),
            hf_token: resolve_hf_token(state),
            hf_base: crate::tools::tools::env_or(ENV_HF_BASE_URL, HF_BASE_URL),
            ms_base: crate::tools::tools::env_or(ENV_MODELSCOPE_BASE_URL, MODELSCOPE_BASE_URL),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
            per_source: PER_SOURCE_LIMIT,
        })
    }

    /// 查一次（**先查本机，再查外网**）。
    pub async fn search(&self, query: ModelQuery) -> Result<ModelFinderResult, ToolFailure> {
        let text = query.text.trim().to_string();
        if text.is_empty() {
            return Err(ToolFailure::new("INVALID_ARGUMENT", "缺少搜索词 query"));
        }

        // ① 先查本机（固定顺序）。命中时**不发送任何外部请求**。
        let hit = self.local_hit(&text, query.inventory.as_ref());
        if !query.online || hit.is_some() {
            let local: Vec<LocalModel> = hit.clone().into_iter().collect();
            let near_misses = self.near_misses(&text, &query, hit.as_ref());
            let notes = if local.is_empty() {
                vec!["本次只查了本机（调用方关掉了出网）。".to_string()]
            } else {
                vec!["本机已有同名文件，**没有**去外网搜索（先查本机、再查外网）。".to_string()]
            };
            return Ok(ModelFinderResult {
                query: text,
                sources: ModelSource::ALL
                    .iter()
                    .map(|source| skipped(*source, !local.is_empty()))
                    .collect(),
                candidates: Vec::new(),
                local_matches: local,
                local_near_misses: near_misses,
                local_source: query
                    .inventory
                    .as_ref()
                    .map(|inv| inv.source)
                    .unwrap_or(Source::None),
                online: false,
                notes,
            });
        }

        // ② 三个源按固定顺序查；**单个源失败不影响其它源**（"逐个报可达性"的前提）
        let outcomes = vec![
            self.civitai(&text, query.limit).await,
            self.hugging_face(&text).await,
            self.model_scope(&text).await,
        ];
        let mut merged = merge(
            outcomes
                .iter()
                .flat_map(|outcome| outcome.candidates().iter().cloned())
                .collect(),
        );
        merged.truncate(query.limit.clamp(1, MAX_LIMIT) as usize);

        let mut notes: Vec<String> = Vec::new();
        if merged.is_empty() && !outcomes.iter().any(|o| o.state() == Reachability::Ok) {
            notes.push(
                "三个源这次都没查到东西（原因见每个源那一行的状态）——\
                 如实告诉用户，不要编模型名。"
                    .to_string(),
            );
        }
        if !merged.is_empty() && !merged.iter().any(|c| c.sha256.is_some()) {
            notes.push(
                "本次没有一条带 sha256（上游没给）——那几份下载完**没法用 sha256 校验**，\
                 如实说明这一点，不要拿字面相似度当「同一个模型」的证据。"
                    .to_string(),
            );
        }

        let near_misses = self.near_misses(&text, &query, None);
        Ok(ModelFinderResult {
            query: text,
            sources: outcomes,
            candidates: merged,
            local_matches: Vec::new(),
            local_near_misses: near_misses,
            local_source: query
                .inventory
                .as_ref()
                .map(|inv| inv.source)
                .unwrap_or(Source::None),
            online: true,
            notes,
        })
    }

    /// 缺模型时把"沾边但不够像"的本机文件报出来（那是"我明明有"误判的来源）。
    ///
    /// ⚠ Kotlin 的 `nearMissesFor(folder, wanted)` 收了一个 `folder`，**函数体里却一个
    /// 字都没读它**；Rust 侧的 [`Inventory::near_misses_for`] 干脆不收这个参数。
    /// 行为完全一致，所以这里那一步"取 folder"只是"清单里没有就早退"的守卫。
    fn near_misses(&self, text: &str, query: &ModelQuery, hit: Option<&LocalModel>) -> Vec<String> {
        let Some(inv) = query.inventory.as_ref() else {
            return Vec::new();
        };
        let known_folder = hit.is_some() || query.kind.and_then(|k| k.folder()).is_some();
        if !known_folder {
            return Vec::new();
        }
        inv.near_misses_for(text, 3)
            .into_iter()
            .map(|m| m.name)
            .collect()
    }

    /// 本机有没有同名的。
    ///
    /// 两层判据，**都不用纯子串**：`anima` 会命中 `anima-turbo`，而那是**另一个模型** ——
    /// 直接判定"本机已有、无需下载"的代价，是缺少一个确实需要的模型。
    ///  ① [`ModelName::same`]（分词后完全一致：`krea2_turbo_int8` 与 `krea2TurboInt8` 是同一个）；
    ///  ② 文件名（去掉目录前缀与后缀）**完全相等** —— 用来认 `/object_info` 那种
    ///     `SDXL\a.safetensors` 的写法与带后缀 / 不带后缀的差别。
    fn local_hit(&self, text: &str, inventory: Option<&Inventory>) -> Option<LocalModel> {
        let inv = inventory?;
        let want = text.trim();
        if want.is_empty() {
            return None;
        }
        let want_base = base_without_ext(want);
        inv.models
            .iter()
            .find(|m| {
                ModelName::same(want, &m.name)
                    || base_without_ext(&m.name).eq_ignore_ascii_case(&want_base)
            })
            .cloned()
    }

    // -----------------------------------------------------------------------
    //  Civitai（复用 CivitaiClient）
    // -----------------------------------------------------------------------

    async fn civitai(&self, text: &str, limit: i64) -> SourceOutcome {
        let Some(search) = self.civitai.as_ref() else {
            return SourceOutcome::Failed {
                source: ModelSource::Civitai,
                state: Reachability::Error,
                detail: "本次运行没有接入 Civitai 搜索客户端（CIVITAI_DISABLED）".to_string(),
            };
        };
        // 固定 `Most Downloaded` + `AllTime`（CivitaiClient 内部定的）：需要的是
        // "这个模型的正主在哪儿"，而不是"最近谁传了个同名的"。
        //
        // `allow_nsfw` **不在这里打开**：`find_model` 的语义是"帮我找某个具体的模型 / 文件"，
        // 成人内容有 `civitai_search` 那条专线（它才带 allowNsfw）。这里打开它，
        // 等于代替其做出未指定的选择。
        match search
            .search(text, false, None, limit.clamp(1, PER_SOURCE_LIMIT))
            .await
        {
            Ok(outcome) => {
                let candidates: Vec<ModelCandidate> = outcome
                    .models
                    .iter()
                    .map(|m| ModelCandidate {
                        name: m.name.clone(),
                        kind: ModelKind::from_civitai(Some(&m.r#type)),
                        base_model: Some(m.base_model.clone()).filter(|b| !b.trim().is_empty()),
                        trained_words: m.trained_words.clone(),
                        size_bytes: m.size_bytes,
                        download_url: m.download_url.clone(),
                        page_url: Some(m.page_url.clone()),
                        sha256: m.sha256.clone(),
                        file_name: m.file_name.clone(),
                        source: ModelSource::Civitai,
                        sources: vec![ModelSource::Civitai],
                        description: Some(m.description.clone()).filter(|d| !d.trim().is_empty()),
                        downloads: Some(m.downloads),
                        note: None,
                    })
                    .collect();
                let detail = describe_count(candidates.len());
                SourceOutcome::Ok {
                    source: ModelSource::Civitai,
                    candidates,
                    detail,
                }
            }
            Err(error) => failed_from_failure(ModelSource::Civitai, &error),
        }
    }

    // -----------------------------------------------------------------------
    //  HuggingFace
    // -----------------------------------------------------------------------

    async fn hugging_face(&self, text: &str) -> SourceOutcome {
        let token = self.hf_token.as_deref();
        let search_url = format!(
            "{}/api/models?search={}&limit={}&sort=downloads&direction=-1",
            self.hf_base,
            url_encode(text),
            self.per_source.clamp(1, PER_SOURCE_LIMIT)
        );
        // ⚠ `GET /api/models` 返回的是一个 **JSON 数组**（不是对象）——
        // 拿"顶层是对象"那条路去取会得到"无法解析的内容"，而那看起来像"HF 挂了"。
        match self
            .get_json_array(&search_url, ModelSource::HuggingFace, token)
            .await
        {
            Ok(repos) => {
                let mut candidates: Vec<ModelCandidate> = Vec::new();
                for repo in repos.iter().filter_map(Value::as_object) {
                    if let Some(candidate) = self.hf_candidate(repo, token).await {
                        candidates.push(candidate);
                    }
                }
                let detail = describe_count(candidates.len());
                SourceOutcome::Ok {
                    source: ModelSource::HuggingFace,
                    candidates,
                    detail,
                }
            }
            Err(error) => SourceOutcome::Failed {
                source: ModelSource::HuggingFace,
                state: error.state,
                detail: error.detail,
            },
        }
    }

    /// 一个 HF 仓库 → 一条候选。
    ///
    /// 要**第二次**请求（`/api/models/{repo}/tree/main`）才拿得到文件名与 LFS oid。
    /// 那次失败是**吞掉**的（降级成"没有文件名 / 没有 sha256"）而不是把整个源判成失败：
    /// 一个仓库的目录读不到，不该让"HF 这个源可达"这个事实变成"HF 挂了"。
    async fn hf_candidate(
        &self,
        repo: &Map<String, Value>,
        token: Option<&str>,
    ) -> Option<ModelCandidate> {
        let repo_id = str_field(repo, "modelId").or_else(|| str_field(repo, "id"))?;
        let tags: Vec<String> = repo
            .get("tags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        // `startsWith(ignoreCase = true)` + `substringAfter("base_model:")`（**后者区分大小写**，
        // 找不到就返回原串 —— Kotlin 那两行的组合行为，照抄）
        let base_model = tags
            .iter()
            .find(|t| {
                t.get(..11)
                    .map(|head| head.eq_ignore_ascii_case("base_model:"))
                    .unwrap_or(false)
            })
            .map(|t| match t.split_once("base_model:") {
                Some((_, rest)) => rest.to_string(),
                None => t.clone(),
            });

        let files = self.hf_tree(&repo_id, token).await.unwrap_or_default();
        // ⚠ `maxByOrNull` 是**并列取第一个**（Rust 的 `max_by_key` 取最后一个）
        let best = files
            .iter()
            .fold(None::<&WeightFile>, |best, file| match best {
                Some(current) if current.size.unwrap_or(0) >= file.size.unwrap_or(0) => {
                    Some(current)
                }
                _ => Some(file),
            });

        // 类型：**文件名 → 仓库名/路径 → pipeline_tag**（后者的信息量最小）。
        // 仓库名也传进去：`Omnico/Krea2_turbo_diff_loras` 这种线索只在仓库名里。
        let full_path = format!(
            "{repo_id}/{}",
            best.map(|f| f.path.as_str()).unwrap_or_default()
        );
        let kind = ModelKind::infer(
            best.map(|f| f.path.as_str()),
            Some(&full_path),
            str_field(repo, "pipeline_tag").as_deref(),
        );

        // diffusers 目录格式：**没有**"一个能放进 diffusion_models\ 的文件"。
        // 这时宁可 `install_into = None` + 一句说明，也不给一个看着能用的目录。
        let diffusers =
            ModelKind::is_diffusers_format(str_field(repo, "library_name").as_deref(), &tags);
        let effective_kind =
            if diffusers && matches!(kind, ModelKind::Diffusion | ModelKind::Checkpoint) {
                ModelKind::Unknown
            } else {
                kind
            };
        let note = if diffusers && effective_kind == ModelKind::Unknown {
            Some(
                "这个仓库是 **diffusers 目录格式**（`model_index.json` + 一堆子目录），\
                 里面**没有**一个能直接放进 ComfyUI 的整包 safetensors —— \
                 去模型页找作者提供的**单文件**版本（通常在同一个仓库的 Files 里，或另一个 repo），\
                 或者让用户自己转格式。**别把这个文件当成能直接用的底模。**"
                    .to_string(),
            )
        } else {
            None
        };

        let Some(best) = best else {
            // 一个权重文件都没有（只有 README / config）：**仍然报出来** ——
            // 需要确认的可能正是"该仓库是否有 safetensors"。但体积 / sha256 只能为空。
            return Some(ModelCandidate {
                name: repo_id.clone(),
                kind: effective_kind,
                base_model,
                trained_words: Vec::new(),
                size_bytes: None,
                download_url: Some(file_link(&repo_id, None)),
                page_url: Some(format!("{HF_BASE_URL}/{repo_id}")),
                sha256: None,
                file_name: None,
                source: ModelSource::HuggingFace,
                sources: vec![ModelSource::HuggingFace],
                description: None,
                downloads: int_field(repo, "downloads"),
                note,
            });
        };
        Some(ModelCandidate {
            name: repo_id.clone(),
            kind: effective_kind,
            base_model,
            trained_words: Vec::new(),
            size_bytes: best.size,
            download_url: Some(file_link(&repo_id, Some(&best.path))),
            page_url: Some(format!("{HF_BASE_URL}/{repo_id}")),
            // HF 的 sha256 **就是 LFS oid**（`lfs.oid` 正是那份大文件的 sha256）。
            // 非 LFS 文件（走 git 对象的小文件）没有 `lfs` 字段 → 留空并如实说明。
            sha256: best.sha256.clone(),
            file_name: Some(best.path.clone()),
            source: ModelSource::HuggingFace,
            sources: vec![ModelSource::HuggingFace],
            description: None,
            downloads: int_field(repo, "downloads"),
            note,
        })
    }

    /// `/api/models/{repo}/tree/main`：取**最大**的权重文件（即需要下载的文件）。
    async fn hf_tree(&self, repo_id: &str, token: Option<&str>) -> Option<Vec<WeightFile>> {
        let url = format!("{}/api/models/{repo_id}/tree/main", self.hf_base);
        // 同样是 **JSON 数组**
        let arr = self
            .get_json_array(&url, ModelSource::HuggingFace, token)
            .await
            .ok()?;
        Some(
            arr.iter()
                .filter_map(Value::as_object)
                .filter_map(|o| {
                    let r#type = str_field(o, "type");
                    if r#type.as_deref().map(|t| t != "file").unwrap_or(false) {
                        return None;
                    }
                    let path = str_field(o, "path")?;
                    if !looks_like_weight(&path) {
                        return None;
                    }
                    let oid = o
                        .get("lfs")
                        .and_then(Value::as_object)
                        .and_then(|lfs| str_field(lfs, "oid"));
                    Some(WeightFile {
                        path,
                        size: long_field(o, "size"),
                        // oid 只有 64 位十六进制才是 sha256
                        // （上游结构变了也不会误当成 hash 报出去）
                        sha256: oid.filter(|value| is_sha256(Some(value))),
                    })
                })
                .collect(),
        )
    }

    // -----------------------------------------------------------------------
    //  ModelScope
    // -----------------------------------------------------------------------

    async fn model_scope(&self, text: &str) -> SourceOutcome {
        // ⚠ 线上核对过（2026-09-22 本机实测）：`GET /api/v1/models?...` 是 **404**，
        // 检索接口是 `PUT /api/v1/dolphin/models` + JSON 体。这不是猜的。
        let body = json!({
            "PageSize": self.per_source.clamp(1, PER_SOURCE_LIMIT),
            "PageNumber": 1,
            "Name": text,
            "SortBy": "Default",
        });
        let url = format!("{}/api/v1/dolphin/models", self.ms_base);
        match self
            .request(&url, ModelSource::ModelScope, None, Some(&body), "PUT")
            .await
        {
            Ok(json) => {
                let mut candidates: Vec<ModelCandidate> = Vec::new();
                for model in ms_models(&json) {
                    if let Some(candidate) = self.model_scope_candidate(model).await {
                        candidates.push(candidate);
                    }
                }
                let detail = describe_count(candidates.len());
                SourceOutcome::Ok {
                    source: ModelSource::ModelScope,
                    candidates,
                    detail,
                }
            }
            Err(error) => SourceOutcome::Failed {
                source: ModelSource::ModelScope,
                state: error.state,
                detail: error.detail,
            },
        }
    }

    async fn model_scope_candidate(&self, model: &Map<String, Value>) -> Option<ModelCandidate> {
        let path = str_field(model, "Path")?;
        let name = str_field(model, "Name").unwrap_or_else(|| path.clone());
        // ⚠ 仓库全名是 **`Path` + `/` + `Name`** —— 实测（2026-09-22，带代理打了两次）：
        // `Path="krea"` + `Name="Krea-2-Turbo"` → `krea/Krea-2-Turbo` 的 `repo/files` 返回 200，
        // 而只拿 `Path` 是 **404** —— 那个 404 会让每个候选都丢掉"文件名 / 直链 / sha256"。
        // `Path` 里已经带斜杠时就是全名，不要再拼一次。
        let repo = if path.contains('/') {
            path.clone()
        } else {
            format!("{path}/{name}")
        };
        let tasks: Vec<String> = model
            .get("Tasks")
            .and_then(Value::as_array)
            .map(|tasks| {
                tasks
                    .iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let base_model = model
            .get("BaseModel")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            .map(str::to_string);

        // 体积：`ModelInfos` 里每个框架一份 `{model_size: …}`。取**最大**那个 ——
        // 需要下载的是权重本体，不是 tokenizer 的那几 MB。
        let meta_size = model
            .get("ModelInfos")
            .and_then(Value::as_object)
            .and_then(|infos| {
                infos
                    .values()
                    .filter_map(Value::as_object)
                    .filter_map(|info| long_field(info, "model_size"))
                    .max()
            });

        // sha256 + 文件名：目录接口 `repo/files` 里每个 blob 带 `Sha256`。取最大的那个权重文件。
        let file = self.ms_largest_file(&repo).await;
        let file_path = file.as_ref().map(|f| f.path.clone());
        Some(ModelCandidate {
            name,
            // 三条线索按可信度排：文件名 → 路径 + 仓库名 → Tasks
            kind: ModelKind::infer(
                file_path.as_deref(),
                Some(&format!("{repo}/{}", file_path.clone().unwrap_or_default())),
                tasks.first().map(String::as_str),
            ),
            base_model,
            trained_words: Vec::new(),
            size_bytes: file.as_ref().and_then(|f| f.size).or(meta_size),
            download_url: file_path
                .as_deref()
                .map(|p| format!("{MODELSCOPE_BASE_URL}/models/{repo}/resolve/master/{p}")),
            page_url: Some(format!("{MODELSCOPE_BASE_URL}/models/{repo}")),
            sha256: file.as_ref().and_then(|f| f.sha256.clone()),
            file_name: file_path,
            source: ModelSource::ModelScope,
            sources: vec![ModelSource::ModelScope],
            description: None,
            downloads: int_field(model, "Downloads"),
            note: None,
        })
    }

    async fn ms_largest_file(&self, path: &str) -> Option<WeightFile> {
        let url = format!(
            "{}/api/v1/models/{path}/repo/files?Revision=master",
            self.ms_base
        );
        let json = self
            .request(&url, ModelSource::ModelScope, None, None, "GET")
            .await
            .ok()?;
        let files = json
            .get("Data")
            .and_then(Value::as_object)
            .and_then(|data| data.get("Files"))
            .and_then(Value::as_array)?;
        // ⚠ `maxByOrNull` 并列取**第一个**（Rust 的 `max_by_key` 取最后一个）
        files
            .iter()
            .filter_map(Value::as_object)
            .filter_map(|o| {
                if str_field(o, "Type").as_deref() != Some("blob") {
                    return None;
                }
                let path = str_field(o, "Path")?;
                if !looks_like_weight(&path) {
                    return None;
                }
                Some(WeightFile {
                    path,
                    size: long_field(o, "Size"),
                    sha256: str_field(o, "Sha256").filter(|value| is_sha256(Some(value))),
                })
            })
            .fold(None::<WeightFile>, |best, file| match best {
                Some(current) if current.size.unwrap_or(0) >= file.size.unwrap_or(0) => {
                    Some(current)
                }
                _ => Some(file),
            })
    }

    // -----------------------------------------------------------------------
    //  出网
    // -----------------------------------------------------------------------

    /// 出一趟网拿一个**顶层是对象**的 JSON（魔搭那两条是；HF 那两条不是）。
    async fn request(
        &self,
        url: &str,
        source: ModelSource,
        token: Option<&str>,
        body: Option<&Value>,
        method: &str,
    ) -> Result<Map<String, Value>, SourceOutcomeError> {
        let text = self.request_text(url, source, token, body, method).await?;
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(obj)) => Ok(obj),
            _ => Err(SourceOutcomeError::new(
                Reachability::Error,
                "返回了无法解析的内容（顶层不是对象）",
            )),
        }
    }

    /// 出一趟网拿一个**顶层是数组**的 JSON。
    ///
    /// 为什么单独一个：`GET /api/models?search=…`（HF 检索）与 `/tree/main`（HF 目录）
    /// 返回的都是数组。第一版用"顶层是对象"那条路去取，结果每次都被判成
    /// "返回了无法解析的内容"——**看起来像 HF 挂了，其实是本地解析错**。
    async fn get_json_array(
        &self,
        url: &str,
        source: ModelSource,
        token: Option<&str>,
    ) -> Result<Vec<Value>, SourceOutcomeError> {
        let text = self.request_text(url, source, token, None, "GET").await?;
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Array(items)) => Ok(items),
            _ => Err(SourceOutcomeError::new(
                Reachability::Error,
                "返回了无法解析的内容（顶层不是数组）",
            )),
        }
    }

    /// 真正发请求的那一层：只负责"发出去 + 状态码映射"，**不解释正文的形状**。
    async fn request_text(
        &self,
        url: &str,
        source: ModelSource,
        token: Option<&str>,
        body: Option<&Value>,
        method: &str,
    ) -> Result<String, SourceOutcomeError> {
        let parsed = reqwest::Url::parse(url).map_err(|_| {
            SourceOutcomeError::new(Reachability::Error, "搜索地址拼装失败（这是本机的 bug）")
        })?;
        // 带 body 却用 GET 是本机的 bug，不是上游的：魔搭的检索接口是 **PUT**，
        // 而"带 body 就发 GET"会线上吃到 `404 page not found`（实测：同一路径 PUT 200 / GET 404）
        // —— 那个 404 会被误读成"上游没这个接口 / URL 写错了"。
        if body.is_some() && method == "GET" {
            return Err(SourceOutcomeError::new(
                Reachability::Error,
                format!("本机 bug：带请求体却用了 GET（{url}）"),
            ));
        }
        let host = parsed.host_str().unwrap_or(source.wire()).to_string();
        let route = crate::infra::outbound::route(&host).describe();

        let client = self.http_client(&host);
        let mut request = client
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| {
                    SourceOutcomeError::new(
                        Reachability::Error,
                        format!("本机 bug：不认识的请求方法（{method}）"),
                    )
                })?,
                parsed,
            )
            .header("Accept", "application/json")
            .timeout(self.timeout);
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(body) = body {
            request = request
                .header("Content-Type", "application/json")
                .body(body.to_string());
        }

        let response = request.send().await.map_err(|e| {
            // 三种连接失败都带**同一句动作指引**（代理怎么设、别反复重试）：
            // 实测本机 `civitai.com` / `huggingface.co` 直连是 **ConnectException（立刻被拒）**，
            // 不是连接超时 —— 只在超时那一档里给指引的话，最常见的那个现场反而拿不到"该怎么办"。
            let guide = format!(
                "本机直连 {host} 不通时，把出网代理设成 127.0.0.1:7890（或设环境变量 {}）——\
                 **不要反复重试**。",
                crate::infra::outbound::ENV_PROXY
            );
            let detail = match outbound_error_kind(&e) {
                OutboundError::ConnectTimeout => format!(
                    "连不上 {host}：{CONNECT_TIMEOUT_SECONDS} 秒内没连上（本次出网路径「{route}」）。{guide}"
                ),
                OutboundError::ReadTimeout => format!(
                    "连上了 {host} 但 {} 秒没回完（本次出网路径「{route}」）。{guide}",
                    self.timeout.as_secs()
                ),
                OutboundError::Other => format!(
                    "连不上 {host}（{}；本次出网路径「{route}」）。{guide}",
                    java_error_name(&e)
                ),
            };
            SourceOutcomeError::new(Reachability::NeedsProxy, detail)
        })?;

        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            // 401 = 没登录 / 令牌无效；403 = gated（HF 要先在网页上同意条款）；
            // 429 = 限流；其余照原样报。
            let (state, hint) = match status {
                401 => (
                    Reachability::NeedsLogin,
                    "需要登录 / 令牌（401）：让用户去「设置 → AI 工具 / 凭据」填一个，别反复试"
                        .to_string(),
                ),
                403 => (
                    Reachability::Gated,
                    "被拒绝（403）：HF 这类 gated 仓库要用户**先在模型页面上同意条款**\
                     ——这一步只能他自己去点，工具替代不了"
                        .to_string(),
                ),
                404 => (Reachability::Error, "这一问的目标不存在（404）".to_string()),
                429 => (
                    Reachability::Error,
                    "被限流了（429）：等一下再试，别连着搜".to_string(),
                ),
                500..=599 => (Reachability::Error, format!("对方暂时不可用（{status}）")),
                _ => (Reachability::Error, format!("返回 {status}")),
            };
            // 5xx 表示"对端已响应"：同时说明出网路径，避免被理解为未走代理。
            let hint = if status >= 500 {
                format!("{hint}（本次出网路径「{route}」）")
            } else {
                hint
            };
            return Err(SourceOutcomeError::new(state, hint));
        }
        Ok(text)
    }

    /// 与 [`crate::tools::web_tools`] / [`crate::tools::civitai`] 同一条代理纪律，超时 20 秒 / 连接 5 秒。
    fn http_client(&self, host: &str) -> reqwest::Client {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECONDS))
            .timeout(self.timeout)
            // 与 CivitaiSearch / KeenableSearch 同规矩：目标是我们写死的域名，不跟随重定向
            .redirect(reqwest::redirect::Policy::none());
        match crate::infra::outbound::route(host) {
            crate::infra::outbound::Route::Proxy(addr) => {
                if let Ok(proxy) = reqwest::Proxy::all(format!("http://{addr}")) {
                    builder = builder.proxy(proxy);
                }
            }
            crate::infra::outbound::Route::Direct => {
                builder = builder.no_proxy();
            }
        }
        // 建不出来时退回一个"没有代理"的默认客户端 —— 与 Kotlin 不同（那边会抛），
        // 但这里只在 TLS 后端初始化失败时才可能发生，届时错误会如实报在那次请求上。
        builder.build().unwrap_or_else(|_| reqwest::Client::new())
    }
}

/// 一个源的候选条数说明（三条成功路径共用同一句话）。
fn describe_count(count: usize) -> String {
    if count == 0 {
        "通了，但这一问没有结果".to_string()
    } else {
        format!("通了，{count} 条")
    }
}

/// 本地短路时，每个源都要在结果里占一行，写清"为什么没查"。
fn skipped(source: ModelSource, local: bool) -> SourceOutcome {
    SourceOutcome::Failed {
        source,
        state: Reachability::Skipped,
        detail: if local {
            "没查：本机已有同名模型（用户拍板的顺序是先查本机、再查外网）".to_string()
        } else {
            "没查：本次只查了本机".to_string()
        },
    }
}

/// [`CivitaiClient`] 抛的 [`ToolFailure`] → 可达性。
///
/// 映射关系直接来自它那套已经上线的错误码（`CIVITAI_TIMEOUT` / `CIVITAI_AUTH` …），
/// 所以"直连超时"与"拒绝了令牌"在一行里就已经分开了，不需要第二条判据。
fn failed_from_failure(source: ModelSource, error: &ToolFailure) -> SourceOutcome {
    let state = match error.code.as_str() {
        // 连接超时 / 连不上：本机多半直连不通（civitai 就是这么个情况）
        "CIVITAI_TIMEOUT" | "CIVITAI_FAILED" => Reachability::NeedsProxy,
        "CIVITAI_AUTH" => Reachability::NeedsLogin,
        _ => Reachability::Error,
    };
    SourceOutcome::Failed {
        source,
        state,
        detail: error.message.clone(),
    }
}

/// 去掉目录前缀与扩展名的文件名（`SDXL\a.safetensors` → `a`）。
fn base_without_ext(name: &str) -> String {
    let normalized = name.replace('\\', "/");
    let bare = normalized.rsplit('/').next().unwrap_or(&normalized);
    match bare.rsplit_once('.') {
        Some((head, _)) => head.to_string(),
        None => bare.to_string(),
    }
}

/// 是否为"需手动下载的"权重文件（按后缀判断，与 [`MODEL_EXTS`] 同源）。
fn looks_like_weight(path: &str) -> bool {
    let ext = path
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_lowercase())
        .unwrap_or_default();
    MODEL_EXTS.contains(&ext.as_str())
}

/// `JsonObject.str`：任意基本类型取字面量，空白算没给。
fn str_field(obj: &Map<String, Value>, key: &str) -> Option<String> {
    let raw = match obj.get(key)? {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    if raw.trim().is_empty() {
        None
    } else {
        Some(raw)
    }
}

/// `JsonPrimitive.intOrNull` 的对应物。
fn int_field(obj: &Map<String, Value>, key: &str) -> Option<i64> {
    match obj.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

/// `size` / `model_size` 这类字段：线上有的给整数、有的给浮点，两种都认；认不出就是 `None`。
fn long_field(obj: &Map<String, Value>, key: &str) -> Option<i64> {
    match obj.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s
            .parse::<i64>()
            .ok()
            .or_else(|| s.parse::<f64>().ok().map(|f| f as i64)),
        _ => None,
    }
}

/// 一个权重文件（HF 的 tree / ModelScope 的 repo/files 共用）。
#[derive(Debug, Clone)]
struct WeightFile {
    path: String,
    size: Option<i64>,
    sha256: Option<String>,
}

/// 直链给**正式站**，不是本次用的 base（base 可以被环境变量指到本机假服务）。
fn file_link(repo_id: &str, path: Option<&str>) -> String {
    match path {
        None => format!("{HF_BASE_URL}/{repo_id}"),
        Some(path) => format!("{HF_BASE_URL}/{repo_id}/resolve/main/{path}"),
    }
}

/// 从 `{"Data":{"Model":{"Models":[…]}}}` 里挖出模型数组（结构在线上核对过）。
fn ms_models(json: &Map<String, Value>) -> Vec<&Map<String, Value>> {
    let data = json.get("Data").and_then(Value::as_object);
    let model = data.and_then(|d| d.get("Model")).and_then(Value::as_object);
    let arr = model
        .and_then(|m| m.get("Models"))
        .or_else(|| data.and_then(|d| d.get("Models")))
        .or_else(|| json.get("Models"))
        .and_then(Value::as_array);
    arr.map(|items| items.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
//  合并去重
// ---------------------------------------------------------------------------

/// 跨源同名的合成一条。
///
/// 键是 `分词集合 + 归一化文件名`：`krea2_turbo_int8.safetensors` 与
/// `krea2-turbo-int8.safetensors`（两个源上的同一份东西）必须合成一条 ——
/// 否则会显示为"两个不同的模型"，而实际为同一个。
///
/// 合并时**优先保留信息更全的那条**（有 sha256 / 体积 / 触发词的赢），
/// 并把两个来源都记进 `sources`。
///
/// ⚠ 键里那一半"归一化文件名"**只做小写**（`krea2_turbo_int8.safetensors` 与
/// `krea2-turbo-int8.safetensors` **不会**因此合并）—— 这是 Kotlin 的代码行为，
/// 上面那段 KDoc 比代码更宽。**照抄代码**：多归一一步会把"两个源上确实不同的文件"
/// 误合成一条，那比"两条同名"更坏。
fn merge(all: Vec<ModelCandidate>) -> Vec<ModelCandidate> {
    let mut order: Vec<String> = Vec::new();
    let mut by_key: HashMap<String, ModelCandidate> = HashMap::new();
    for candidate in all {
        let mut tokens = ModelName::tokens(&candidate.name);
        tokens.sort();
        let file = candidate
            .file_name
            .clone()
            .unwrap_or_default()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_lowercase();
        let key = format!("{}|{}", tokens.join("-"), file);
        match by_key.remove(&key) {
            None => {
                order.push(key.clone());
                by_key.insert(key, candidate);
            }
            Some(previous) => {
                by_key.insert(key, richer(previous, candidate));
            }
        }
    }
    order
        .into_iter()
        .filter_map(|key| by_key.remove(&key))
        .collect()
}

fn richer(a: ModelCandidate, b: ModelCandidate) -> ModelCandidate {
    let (primary, other) = if score(&b) > score(&a) {
        (b, a)
    } else {
        (a, b)
    };
    let mut trained_words = primary.trained_words.clone();
    for word in &other.trained_words {
        if !trained_words.contains(word) {
            trained_words.push(word.clone());
        }
    }
    let mut sources = primary.sources.clone();
    for source in &other.sources {
        if !sources.contains(source) {
            sources.push(*source);
        }
    }
    sources.sort_by_key(|s| {
        ModelSource::ALL
            .iter()
            .position(|x| x == s)
            .unwrap_or(usize::MAX)
    });
    ModelCandidate {
        trained_words,
        base_model: primary
            .base_model
            .clone()
            .or_else(|| other.base_model.clone()),
        size_bytes: primary.size_bytes.or(other.size_bytes),
        download_url: primary
            .download_url
            .clone()
            .or_else(|| other.download_url.clone()),
        page_url: primary.page_url.clone().or_else(|| other.page_url.clone()),
        sha256: primary.sha256.clone().or_else(|| other.sha256.clone()),
        file_name: primary
            .file_name
            .clone()
            .or_else(|| other.file_name.clone()),
        description: primary
            .description
            .clone()
            .or_else(|| other.description.clone()),
        downloads: primary.downloads.or(other.downloads),
        note: primary.note.clone().or_else(|| other.note.clone()),
        sources,
        ..primary
    }
}

fn score(candidate: &ModelCandidate) -> i64 {
    i64::from(candidate.sha256.is_some()) * 4
        + i64::from(candidate.size_bytes.is_some()) * 2
        + i64::from(!candidate.trained_words.is_empty())
        + i64::from(candidate.download_url.is_some())
}

// ---------------------------------------------------------------------------
//  工具入口
// ---------------------------------------------------------------------------

/// `find_model`：定向查询一个模型从哪儿拿（**只给链接，绝不代下**）。
pub async fn tool_find_model(
    state: &AppState,
    arguments: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let Some(raw) = str_arg(arguments, "query") else {
        return Err(ToolFailure::new("INVALID_ARGUMENT", "缺少参数 query"));
    };
    let query = raw.trim().to_string();
    let kind = parse_model_kind(str_arg(arguments, "type").as_deref())?;
    let limit = crate::tools::tools::int_arg(arguments, "limit")
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);

    // 本机清单：**与 comfy_env_report / comfy_fit_workflow 用的是同一份**
    // （`/object_info` ∪ 盘扫）。另算一份就会出现"对账说本机有、find_model 说没有"
    // 这种自相矛盾（AGENTS §10：不要维护两份判据）。
    let inventory = current_inventory(state).await;

    let finder = HttpModelFinder::from_state(state)?;
    let result = finder
        .search(ModelQuery {
            text: query,
            kind,
            limit,
            online: true,
            inventory: Some(inventory),
        })
        .await?;
    let content = render_find_model(&result, kind);
    Ok(ToolOutcome::new(content, to_find_model_json(&result)))
}

/// 本机模型清单（与 `comfy_env_report` / `comfy_fit_workflow` **同一份取法**）。
async fn current_inventory(state: &AppState) -> Inventory {
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
    let model_dirs: Vec<PathBuf> = desktop
        .as_ref()
        .map(|d| d.models_dirs.clone())
        .unwrap_or_default();
    crate::comfy::comfy_models::inventory(object_info.as_ref(), Some(&project_root), &model_dirs)
}

/// HF 令牌：受管凭据库优先、`HF_TOKEN` 环境变量兜底（与 Civitai 的两路同一约定）。
fn resolve_hf_token(state: &AppState) -> Option<String> {
    crate::mobile::auth::resolve_credential(state, HF_CREDENTIAL_REF)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("HF_TOKEN")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
}

/// 把模型给的 `type` 归一成 [`ModelKind`]；不认识就说清可选值（别让它反复试）。
fn parse_model_kind(raw: Option<&str>) -> Result<Option<ModelKind>, ToolFailure> {
    const KNOWN: [&str; 11] = [
        "Checkpoint",
        "LORA",
        "Locon",
        "DoRA",
        "VAE",
        "Controlnet",
        "Upscaler",
        "TextualInversion",
        "diffusion",
        "text_encoder",
        "clip_vision",
    ];
    let Some(value) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let kind = match value.to_lowercase().as_str() {
        "checkpoint" | "ckpt" | "底模" | "大模型" => ModelKind::Checkpoint,
        "lora" | "locon" | "dora" | "lycoris" => ModelKind::Lora,
        "vae" => ModelKind::Vae,
        "controlnet" | "control_net" => ModelKind::Controlnet,
        "upscaler" | "esrgan" => ModelKind::Upscaler,
        "textualinversion" | "embedding" | "embeddings" => ModelKind::Embedding,
        "diffusion" | "diffusion_models" | "unet" | "dit" => ModelKind::Diffusion,
        "text_encoder" | "text_encoders" | "clip" => ModelKind::TextEncoder,
        "clip_vision" => ModelKind::ClipVision,
        _ => {
            return Err(ToolFailure::new(
                "INVALID_ARGUMENT",
                format!(
                    "type 只能是 {}（大小写不敏感）；收到的是「{value}」。不传这个参数也可以。",
                    KNOWN.join(" / ")
                ),
            ))
        }
    };
    Ok(Some(kind))
}

/// 结果文本：**结论先行**，然后是每个源的状态、候选、本机情况。
fn render_find_model(result: &ModelFinderResult, kind: Option<ModelKind>) -> String {
    let mut out = crate::tools::tools::untrusted_banner("模型检索", Some(&result.query));
    out.push_str("\n\n");

    // ① 本机优先那条路的结论（这是最重要的一句：它决定"还要不要下"）
    if result.local_hit() {
        out.push_str(
            "**本机已经有这个文件了** —— 先把这条告诉用户，别急着让他去下（也不用下载）。\n",
        );
        for local in &result.local_matches {
            out.push_str(&format!("· {}/{}", local.folder, local.name));
            if let Some(gb) = local.size_gb() {
                out.push_str(&format!("（{gb:.2}GB）"));
            }
            if !local.visible_to_instance {
                out.push_str("  ← **当前跑的 ComfyUI 实例看不到它**（盘上有、实例没挂全）");
            }
            if let Some(path) = local.path.as_deref() {
                out.push_str(&format!("\n  路径：{path}"));
            }
            out.push('\n');
        }
        if result.local_matches.iter().any(|m| !m.visible_to_instance) {
            out.push_str(
                "→ 正确动作：让用户**重启 ComfyUI / 修好模型根**（`comfy_env_report` 能看出差在哪），\
                 **不要去下载**（文件本来就在硬盘上），也别改 `comfy_submit` 的 overrides。\n",
            );
        }
    } else {
        out.push_str(&format!(
            "本机（清单来源：{}）**没有同名文件** —— ",
            local_source_label(result.local_source)
        ));
        if result.online {
            out.push_str("所以下面这些是外网查到的。\n");
        } else {
            out.push_str("本次也没有去外网查。\n");
        }
        if !result.local_near_misses.is_empty() {
            out.push_str("本机有几个**名字沾边但不是同一个东西**的：");
            out.push_str(&result.local_near_misses.join("、"));
            out.push_str("（别拿它们顶；报给用户时也可以提一句。）\n");
        }
    }

    // ② 每个源都占一行（不可达的也在）
    out.push_str("\n【各源可达性】\n");
    for source in &result.sources {
        out.push_str(&format!(
            "· {}：{}",
            source.source().label(),
            source.state().label()
        ));
        if source.state() == Reachability::Ok {
            out.push_str(&format!("（{} 条）", source.candidates().len()));
        }
        if !source.detail().is_empty() {
            out.push_str(&format!(" —— {}", source.detail()));
        }
        out.push('\n');
    }

    // ③ 候选
    if !result.candidates.is_empty() {
        out.push_str(&format!("\n【候选 {} 条】\n", result.candidates.len()));
        for (index, candidate) in result.candidates.iter().enumerate() {
            out.push_str(&render_candidate(index + 1, candidate, kind));
        }
    } else if !result.local_hit() {
        out.push_str(
            "\n（一条候选都没有。如实告诉用户没查到 + 试过什么词，**不要编模型名或链接**。）\n",
        );
        let dead: Vec<&SourceOutcome> = result
            .sources
            .iter()
            .filter(|s| s.state() != Reachability::Ok && s.state() != Reachability::Skipped)
            .collect();
        if !dead.is_empty() {
            out.push_str("注意：这次有源查不成（");
            out.push_str(
                &dead
                    .iter()
                    .map(|s| format!("{}={}", s.source().label(), s.state().wire()))
                    .collect::<Vec<_>>()
                    .join("、"),
            );
            out.push_str("）—— \"没查到\"里有一部分是\"没查成\"，两件事要分开说。\n");
        }
    }

    // ④ 绝不代下
    out.push_str(
        "\n**绝不代下**（用户拍板）：你只把上面这些贴给他 —— 模型名 / 体积 / 直链 / sha256 / \
         该放进哪个目录，然后问他要不要自己下。**不要替他下载**（`download_model` 是 deny 的），\
         也不要说\"我帮你下好了\"。下完之后可以用 sha256 核对盘上那份是不是同一个模型。\n",
    );

    for note in &result.notes {
        out.push_str(&format!("\n· {note}"));
    }
    out
}

/// 一条候选：名字 / 类型 / 该放哪 / 底模 / 触发词 / 体积 / sha256 / 直链 / 来源。
fn render_candidate(index: usize, candidate: &ModelCandidate, asked: Option<ModelKind>) -> String {
    let mut out = format!(
        "\n[{index}] {}（{}）",
        candidate.name,
        candidate.kind.label()
    );
    if candidate.sources.len() > 1 {
        out.push_str(&format!(
            "  ← 在 {} 上都有",
            candidate
                .sources
                .iter()
                .map(|s| s.label())
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    out.push('\n');
    if let Some(base_model) = candidate.base_model.as_deref() {
        out.push_str(&format!("  底模：{base_model}\n"));
    }
    if let Some(size) = candidate.size_label() {
        out.push_str(&format!("  体积：{size}\n"));
    }
    if !candidate.trained_words.is_empty() {
        out.push_str(&format!(
            "  触发词：{}\n",
            candidate.trained_words.join(", ")
        ));
    }
    if let Some(sha) = candidate.sha256.as_deref() {
        out.push_str(&format!("  sha256：{sha}\n"));
    }
    if let Some(file_name) = candidate.file_name.as_deref() {
        out.push_str(&format!("  文件名：{file_name}\n"));
    }

    // 「该放哪个目录」永远要有一行 —— 拿不到就明说"说不准"，别让模型自己编一个
    match candidate.install_into() {
        Some(folder) => {
            out.push_str(&format!(
                "  该放进：{folder}\\\u{ff08}相对 {INSTALL_ROOT_HINT}）\n"
            ));
        }
        None => {
            if candidate.note.is_some() {
                // 有明确原因（例如 diffusers 目录格式）时先把原因写出来，再说"放哪说不准"
                out.push_str("  该放进：**这个文件不一定能直接放进 ComfyUI**，见下面那条说明。\n");
            } else {
                out.push_str("  该放进：**说不准**（这个源没说清它是什么类型）—— ");
                match asked.and_then(|k| k.folder()) {
                    Some(folder) => out.push_str(&format!(
                        "你说这次要的是 {}，那大概率是 {folder}\\，但**让用户自己确认**一下再放。\n",
                        asked.map(|k| k.label()).unwrap_or_default()
                    )),
                    None => out.push_str(
                        "让用户看工作流里那个加载器要的是哪一类，再决定放哪个子目录。\n",
                    ),
                }
                if let Some(file_name) = candidate.file_name.as_deref() {
                    out.push_str(&format!("  （文件名里有线索：{file_name}）\n"));
                }
            }
        }
    }

    if let Some(url) = candidate.download_url.as_deref() {
        out.push_str(&format!("  直链：{url}\n"));
    }
    if let Some(url) = candidate.page_url.as_deref() {
        out.push_str(&format!("  模型页：{url}\n"));
    }
    if let Some(description) = candidate.description.as_deref() {
        out.push_str(&format!("  简介：{}\n", take_utf16(description, 300)));
    }
    if let Some(downloads) = candidate.downloads {
        out.push_str(&format!("  下载量：{downloads}\n"));
    }
    // 这一条不得含糊：拿不到类型 / 格式不匹配时，原因需原样返回
    if let Some(note) = candidate.note.as_deref() {
        out.push_str(&format!("  ⚠ {note}\n"));
    }
    out.push_str(&format!("  来源：{}", candidate.source.label()));
    if candidate.sha256.is_none() {
        out.push_str("（这一条上游没给 sha256，下完没法校验）");
    }
    out.push('\n');
    out
}

fn local_source_label(source: Source) -> &'static str {
    match source {
        Source::ObjectInfo => "ComfyUI 的 /object_info（能加载什么就是什么）",
        Source::Scan => "目录扫描（盘上有，但未必被 ComfyUI 认到）",
        Source::None => "没读到（ComfyUI 没在跑，也没扫到 models 目录）",
    }
}

/// 结构化结果：界面与模型都能直接用，不必去解析上面那段给模型看的文本。
fn to_find_model_json(result: &ModelFinderResult) -> Value {
    let local_matches: Vec<Value> = result
        .local_matches
        .iter()
        .map(|m| {
            let mut out = Map::new();
            out.insert("name".into(), json!(m.name));
            out.insert("folder".into(), json!(m.folder));
            out.insert("visibleToInstance".into(), json!(m.visible_to_instance));
            if let Some(path) = m.path.as_deref() {
                out.insert("path".into(), json!(path));
            }
            if let Some(gb) = m.size_gb() {
                out.insert("sizeGB".into(), json!((gb * 100.0).round() / 100.0));
            }
            Value::Object(out)
        })
        .collect();
    let sources: Vec<Value> = result
        .sources
        .iter()
        .map(|s| {
            json!({
                "source": s.source().wire(),
                "state": s.state().wire(),
                "detail": s.detail(),
                "count": s.candidates().len(),
            })
        })
        .collect();
    let candidates: Vec<Value> = result
        .candidates
        .iter()
        .map(|c| {
            let mut out = Map::new();
            out.insert("name".into(), json!(c.name));
            out.insert("kind".into(), json!(c.kind.wire()));
            if let Some(folder) = c.install_into() {
                out.insert("installInto".into(), json!(folder));
            }
            if let Some(base_model) = c.base_model.as_deref() {
                out.insert("baseModel".into(), json!(base_model));
            }
            if !c.trained_words.is_empty() {
                out.insert("trainedWords".into(), json!(c.trained_words));
            }
            if let Some(bytes) = c.size_bytes {
                out.insert("sizeBytes".into(), json!(bytes));
                out.insert("sizeLabel".into(), json!(c.size_label()));
            }
            out.insert(
                "url".into(),
                json!(c
                    .download_url
                    .clone()
                    .unwrap_or_else(|| c.page_url.clone().unwrap_or_default())),
            );
            if let Some(url) = c.download_url.as_deref() {
                out.insert("downloadUrl".into(), json!(url));
            }
            if let Some(url) = c.page_url.as_deref() {
                out.insert("pageUrl".into(), json!(url));
            }
            if let Some(sha) = c.sha256.as_deref() {
                out.insert("sha256".into(), json!(sha));
            }
            if let Some(file_name) = c.file_name.as_deref() {
                out.insert("fileName".into(), json!(file_name));
            }
            // 「为什么放说不准」也要进结构化结果（界面与模型都能直接读）
            if let Some(note) = c.note.as_deref() {
                out.insert("note".into(), json!(note));
            }
            out.insert(
                "sources".into(),
                json!(c.sources.iter().map(|s| s.wire()).collect::<Vec<_>>()),
            );
            if let Some(downloads) = c.downloads {
                out.insert("downloads".into(), json!(downloads));
            }
            Value::Object(out)
        })
        .collect();

    let mut out = Map::new();
    out.insert("query".into(), json!(result.query));
    out.insert("online".into(), json!(result.online));
    out.insert(
        "localInventorySource".into(),
        json!(result.local_source.wire()),
    );
    out.insert("localHit".into(), json!(result.local_hit()));
    out.insert("localMatches".into(), Value::Array(local_matches));
    if !result.local_near_misses.is_empty() {
        out.insert("localNearMisses".into(), json!(result.local_near_misses));
    }
    out.insert("sources".into(), Value::Array(sources));
    out.insert("count".into(), json!(result.candidates.len()));
    out.insert("candidates".into(), Value::Array(candidates));
    // 「绝不代下」写进结构化结果里：界面 / 下游读这份 JSON 时也看得到这条纪律
    out.insert("downloadsModel".into(), json!(false));
    if !result.notes.is_empty() {
        out.insert("notes".into(), json!(result.notes));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests;
