//! 本机 ComfyUI 的**模型清单**与"工作流要的模型 ↔ 本机最接近的那个"的配对
//! （`ComfyModels.kt` + `ModelName` 的逐条移植，P4 下一批）。
//!
//! # 为什么需要它
//!
//! 用户的原话是"可以使用 civitai 等网站查找符合要求的工作流，**优先根据本机已有的模型修改该工作流**"。
//! 三件工具当时都齐了（`civitai_search` 能搜模型、`comfy_load_workflow` 能读工作流、
//! `comfy_submit(overrides=…)` 能改参数），**缺的正是中间那一步**：判断"这份图里点名的
//! `xxx.safetensors`，本机上哪个文件能顶"。
//!
//! # 两条纪律（一个字都不改）
//!
//!  1. **只报不改**：这个模块只回答三个问题 —— 图里要哪些模型 / 本机最接近的是哪个 /
//!     要改哪几个 `overrides` 键。真正动手改图的永远是 `comfy_submit(overrides=…)`。
//!  2. **不猜也不编**：配不上就报 `missing` 并明说"本机没有"，**绝不**为了好看硬塞一个
//!     "最接近的"上去；分数低于 [`MIN_SCORE`] 的一律不给候选。
//!
//! # 清单从哪来（两级来源，都要）
//!
//!  - **`/object_info`**：`CheckpointLoaderSimple.ckpt_name` 的枚举**就是** ComfyUI 自己能加载的
//!    checkpoint 列表 —— 这是唯一权威的"能加载什么"。
//!  - **扫盘**：按目录扫 `models\<folder>`（判据复用 [`crate::comfy::comfy_dirs::comfy_homes`]），
//!    只看文件元数据、不打开文件。
//!
//! ⚠ **必须取并集**（2026-09-22 的事故就是"问到实例就 `return`"造成的）：`launch_comfyui` 起的
//! 实例没挂共享模型根时，`/object_info` 是一份**权威但看不到共享目录**的清单，于是"盘上明明有的
//! anima-base / qwen_3_06b_base"被报成"本机没有"，模型据此让用户去下载一个**本来就在硬盘上**的
//! 文件。现在：能加载的标 `visibleToInstance = true`（权威），盘上多出来的也列进来但标 `false`
//! 并单列进 [`Inventory::disk_only`] —— 报告据此说"**这个实例看不到它们**"，而不是"本机没有"。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Map, Value};

use crate::tools::tools::{ToolFailure, ToolOutcome};
use crate::web::routes::AppState;

/// 认的模型文件后缀（ComfyUI 实际能加载的那些；`.pth` / `.bin` / `.ckpt` 是历史格式）。
pub const MODEL_EXTS: &[&str] = &[
    "safetensors",
    "ckpt",
    "pt",
    "pth",
    "bin",
    "gguf",
    "sft",
    "onnx",
    "yaml",
    "json",
];

/// 一次最多列多少条（用户的 models 目录上千个文件是常态，不能整个塞进工具结果）。
pub const MAX_ENTRIES: usize = 500;

/// 一条候选最多给几个（3 个足够做决定，多了只会淹掉结论）。
pub const MAX_CANDIDATES: usize = 3;

/// 多低的相似度就不给候选了。
///
/// 0.35 ≈ "至少有一半 token 对上了"。定这条线是为了**不编**：ComfyUI 只认磁盘上真实存在的
/// 文件名，给一个看着像的名字等于让用户白等一次提交（还会得到一句看不懂的报错）。
pub const MIN_SCORE: f64 = 0.35;

/// "沾边"的下限（只作为缺模型时的背景，**不出现在候选里**）。
///
/// 0.15 ≈ 只共用一个词。定这条是因为**报错的名字比不报更糟**：用户看到
/// "本机有个 sd15.safetensors" 会以为"那不就是我的模型吗"，而它跟 `krea2TurboInt8` 其实毫无关系。
pub const NEAR_MISS_MIN: f64 = 0.15;

/// 扫描深度上限。
const MAX_SCAN_DEPTH: usize = 3;

/// 扫描结果的缓存时长（装完模型重查能得到新结果，又不必每次工具调用都重扫）。
const SCAN_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// 一个节点输入"等着一个模型文件"。
///
/// `folder` 是 ComfyUI 在 `models\` 下的子目录名 —— 扫盘兜底时按它找；
/// 从 `/object_info` 拿时其实不需要它（枚举已经列全了），留着是为了让报告能说清"该放哪"。
struct ModelInput {
    cls: &'static str,
    input: &'static str,
    folder: &'static str,
}

/// 认得出的"模型加载器"（`ComfyModels.MODEL_INPUTS` 逐条照抄）。
///
/// 只列**确定**的：判据是"这个类的这个输入，值就是一个模型文件名"。
/// 认不出来的第三方加载器**不猜**（宁可漏报，也不要把 `model` 这种转发连线当成文件名）。
const MODEL_INPUTS: &[ModelInput] = &[
    ModelInput {
        cls: "CheckpointLoaderSimple",
        input: "ckpt_name",
        folder: "checkpoints",
    },
    ModelInput {
        cls: "CheckpointLoader",
        input: "ckpt_name",
        folder: "checkpoints",
    },
    ModelInput {
        cls: "unCLIPCheckpointLoader",
        input: "ckpt_name",
        folder: "checkpoints",
    },
    ModelInput {
        cls: "ImageOnlyCheckpointLoader",
        input: "ckpt_name",
        folder: "checkpoints",
    },
    ModelInput {
        cls: "UNETLoader",
        input: "unet_name",
        folder: "diffusion_models",
    },
    ModelInput {
        cls: "UnetLoaderGGUF",
        input: "unet_name",
        folder: "diffusion_models",
    },
    ModelInput {
        cls: "UnetLoaderGGUFAdvanced",
        input: "unet_name",
        folder: "diffusion_models",
    },
    ModelInput {
        cls: "DiffusionModelLoader",
        input: "model_name",
        folder: "diffusion_models",
    },
    ModelInput {
        cls: "LoraLoader",
        input: "lora_name",
        folder: "loras",
    },
    ModelInput {
        cls: "LoraLoaderModelOnly",
        input: "lora_name",
        folder: "loras",
    },
    ModelInput {
        cls: "LoraModelLoader",
        input: "lora_name",
        folder: "loras",
    },
    ModelInput {
        cls: "VAELoader",
        input: "vae_name",
        folder: "vae",
    },
    ModelInput {
        cls: "CLIPLoader",
        input: "clip_name",
        folder: "text_encoders",
    },
    ModelInput {
        cls: "DualCLIPLoader",
        input: "clip_name1",
        folder: "text_encoders",
    },
    ModelInput {
        cls: "DualCLIPLoader",
        input: "clip_name2",
        folder: "text_encoders",
    },
    ModelInput {
        cls: "TripleCLIPLoader",
        input: "clip_name1",
        folder: "text_encoders",
    },
    ModelInput {
        cls: "TripleCLIPLoader",
        input: "clip_name2",
        folder: "text_encoders",
    },
    ModelInput {
        cls: "TripleCLIPLoader",
        input: "clip_name3",
        folder: "text_encoders",
    },
    ModelInput {
        cls: "CLIPVisionLoader",
        input: "clip_name",
        folder: "clip_vision",
    },
    ModelInput {
        cls: "ControlNetLoader",
        input: "control_net_name",
        folder: "controlnet",
    },
    ModelInput {
        cls: "DiffControlNetLoader",
        input: "control_net_name",
        folder: "controlnet",
    },
    ModelInput {
        cls: "StyleModelLoader",
        input: "style_model_name",
        folder: "style_models",
    },
    ModelInput {
        cls: "UpscaleModelLoader",
        input: "model_name",
        folder: "upscale_models",
    },
    ModelInput {
        cls: "GLIGENLoader",
        input: "gligen_name",
        folder: "gligen",
    },
    ModelInput {
        cls: "PhotoMakerLoader",
        input: "photomaker_model_name",
        folder: "photomaker",
    },
    ModelInput {
        cls: "IPAdapterModelLoader",
        input: "ipadapter_file",
        folder: "ipadapter",
    },
    ModelInput {
        cls: "CLIPVisionEncode",
        input: "clip_vision",
        folder: "clip_vision",
    },
    ModelInput {
        cls: "InstantIDModelLoader",
        input: "instantid_file",
        folder: "instantid",
    },
    ModelInput {
        cls: "IPAdapterUnifiedLoader",
        input: "preset",
        folder: "ipadapter",
    },
];

/// 清单的来源 —— 报告里要能看出这一档（"能加载"与"盘上有"不是一回事）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// 来自 ComfyUI 的 `/object_info`：**权威**，列出来的就是它能加载的。
    ObjectInfo,
    /// 拿不到 ComfyUI，按目录扫出来的：盘上有，但未必被它认到。
    Scan,
    /// 两边都没有（ComfyUI 没跑，也没找到 models 目录）。
    None,
}

impl Source {
    pub fn wire(self) -> &'static str {
        match self {
            Source::ObjectInfo => "object_info",
            Source::Scan => "scan",
            Source::None => "none",
        }
    }
}

/// 本机的一个模型文件。
#[derive(Debug, Clone, PartialEq)]
pub struct LocalModel {
    /// ComfyUI 认的名字（可能带子目录前缀，如 `SDXL\a.safetensors`）
    pub name: String,
    /// 它在 `models\` 下属于哪一类（checkpoints / loras / vae …）
    pub folder: String,
    /// 盘上的绝对路径（扫盘来源才有；`/object_info` 来源为 `None`）
    pub path: Option<String>,
    pub bytes: Option<u64>,
    /// **当前跑着的那个 ComfyUI 实例能不能加载它**（= 它出现在 `/object_info` 的枚举里）。
    ///
    /// 只有 `false` 才是需要解释的状态（"盘上有、这个实例看不到"）。
    pub visible_to_instance: bool,
}

impl LocalModel {
    /// 给用户看的体积（GB）；不知道就 `None` —— **不编一个数出来**。
    pub fn size_gb(&self) -> Option<f64> {
        self.bytes.map(|b| b as f64 / 1024.0 / 1024.0 / 1024.0)
    }
}

/// 一份完整的本机清单。
#[derive(Debug, Clone)]
pub struct Inventory {
    pub source: Source,
    pub models: Vec<LocalModel>,
    /// 认出来的 `models\` 目录（扫盘来源才有；报告里要能说清"扫的是哪儿"）
    pub models_dir: Option<PathBuf>,
    pub truncated: bool,
    /// **盘上有、但当前实例枚举不到**的那些。
    pub disk_only: Vec<LocalModel>,
    /// 这份清单是不是 `/object_info` 与盘扫的**并集**（只有并集才谈得上"看不到"）。
    pub merged: bool,
}

/// 一个候选（`same_folder` 为假 = 它不在该输入本来的那一类目录里，用之前要跟用户说一声）。
#[derive(Debug, Clone)]
pub struct Scored {
    pub model: LocalModel,
    pub score: f64,
    pub same_folder: bool,
}

impl Inventory {
    pub fn in_folder(&self, folder: &str) -> Vec<&LocalModel> {
        self.models
            .iter()
            .filter(|m| m.folder.eq_ignore_ascii_case(folder))
            .collect()
    }

    /// 全清单里找候选：**先在本类目录里找，不够再跨目录找**
    /// （用户把 LoRA 放进 checkpoints 是常见事故，报出来比一句"本机没有"有用得多）。
    pub fn candidates_for(&self, folder: &str, wanted: &str) -> Vec<Scored> {
        let scored_of = |local: &LocalModel, same_folder: bool| -> Option<Scored> {
            let s = ModelName::similarity(wanted, &local.name);
            if s < MIN_SCORE {
                return None;
            }
            Some(Scored {
                model: local.clone(),
                score: s,
                same_folder,
            })
        };
        let same: Vec<Scored> = self
            .in_folder(folder)
            .into_iter()
            .filter_map(|m| scored_of(m, true))
            .collect();
        let cross: Vec<Scored> = if same.len() >= MAX_CANDIDATES {
            Vec::new()
        } else {
            self.models
                .iter()
                .filter(|m| !m.folder.eq_ignore_ascii_case(folder))
                .filter_map(|m| scored_of(m, false))
                .collect()
        };
        let mut all: Vec<Scored> = same.into_iter().chain(cross).collect();
        all.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all.truncate(MAX_CANDIDATES);
        all
    }

    /// **接近但不够像**的几条（分数低于 [`MIN_SCORE`]）—— 缺模型时给用户看的一句话背景。
    ///
    /// 两道闸门，缺一个这句提示就会变成噪声：
    ///  ① 分数要过 [`NEAR_MISS_MIN`]；② 至少要有一个**长度 ≥ 3 的公共词**。
    pub fn near_misses_for(&self, wanted: &str, limit: usize) -> Vec<LocalModel> {
        let want = ModelName::token_set(wanted);
        let mut hits: Vec<(LocalModel, f64)> = self
            .models
            .iter()
            .map(|m| (m.clone(), ModelName::similarity(wanted, &m.name)))
            .filter(|(local, s)| {
                if !(*s > NEAR_MISS_MIN && *s < MIN_SCORE) {
                    return false;
                }
                let have = ModelName::token_set(&local.name);
                want.intersection(&have).any(|t| t.chars().count() >= 3)
            })
            .collect();
        hits.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.name.cmp(&b.0.name))
        });
        hits.truncate(limit);
        hits.into_iter().map(|(m, _)| m).collect()
    }
}

// ---------------------------------------------------------------------------
//  清单来源
// ---------------------------------------------------------------------------

struct Cached {
    at: std::time::Instant,
    models_dir: Option<PathBuf>,
    models: Vec<LocalModel>,
    truncated: bool,
}

fn scan_cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 拿本机清单：**`/object_info` 与盘扫的并集**。
///
/// * `object_info`：ComfyUI 的 `/object_info`（拿得到就是"能加载什么"的唯一权威）；
///   为 `None` 时退回扫盘。
/// * `model_dirs`：用户**真实的模型根**（`models` 目录本身），例如 ComfyUI Desktop 的
///   `…\ComfyUI-Shared\models`。由调用方传进来、**不在这里读环境** ——
///   这样单测才不会被开发机的真实目录影响。
pub fn inventory(
    object_info: Option<&Map<String, Value>>,
    project_root: Option<&Path>,
    model_dirs: &[PathBuf],
) -> Inventory {
    let from_instance = from_object_info(object_info);
    let from_disk = scan(project_root, model_dirs);
    let Some(from_instance) = from_instance else {
        return from_disk;
    };
    if from_disk.models.is_empty() {
        return from_instance;
    }
    merge(&from_instance, &from_disk)
}

/// 并集：盘上的为准（有 path / 体积），实例枚举到的标成"能加载"。
///
/// 判重按 `目录 + 文件名` 归一化后比（`/` 与 `\` 都当分隔符、大小写不敏感）——
/// `/object_info` 给的可能是 `SDXL\a.safetensors`，而扫盘给的是 `SDXL/a.safetensors`。
fn merge(instance: &Inventory, disk: &Inventory) -> Inventory {
    let mut by_key: HashMap<String, LocalModel> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for m in &disk.models {
        let k = key_of(m);
        if !by_key.contains_key(&k) {
            order.push(k.clone());
        }
        let mut m = m.clone();
        m.visible_to_instance = false;
        by_key.insert(k, m);
    }
    for m in &instance.models {
        let k = key_of(m);
        let on_disk = by_key.get(&k);
        if !by_key.contains_key(&k) {
            order.push(k.clone());
        }
        by_key.insert(
            k,
            LocalModel {
                visible_to_instance: true,
                path: on_disk.and_then(|d| d.path.clone()),
                bytes: on_disk.and_then(|d| d.bytes),
                ..m.clone()
            },
        );
    }
    let mut all: Vec<LocalModel> = by_key.into_values().collect();
    all.sort_by(|a, b| a.folder.cmp(&b.folder).then_with(|| a.name.cmp(&b.name)));
    let truncated = all.len() > MAX_ENTRIES;
    let disk_only: Vec<LocalModel> = all
        .iter()
        .filter(|m| !m.visible_to_instance)
        .cloned()
        .collect();
    if truncated {
        all.truncate(MAX_ENTRIES);
    }
    Inventory {
        // 权威来源仍然是实例：**能不能加载**由它说了算
        source: instance.source,
        models: all,
        models_dir: disk.models_dir.clone(),
        truncated,
        disk_only,
        merged: true,
    }
}

fn key_of(m: &LocalModel) -> String {
    format!(
        "{}|{}",
        m.folder.to_lowercase(),
        m.name.replace('\\', "/").to_lowercase()
    )
}

/// 从 `/object_info` 的**输入枚举**里抠出模型清单。
///
/// 这是"清单从哪来"说的首选来源：`CheckpointLoaderSimple` 的 `ckpt_name` 声明形如
/// `[["a.safetensors", "sub\\b.safetensors"]]`（第一项是枚举，后面可能跟 `{"tooltip":…}`），
/// 枚举里那一串**就是** ComfyUI 能加载的全部文件。
///
/// 拿不到任何一条时返回 `None`（→ 退回扫盘）：空清单比"没有清单"更危险，
/// 它会让每一份工作流都看起来"什么模型都缺"。
pub fn from_object_info(object_info: Option<&Map<String, Value>>) -> Option<Inventory> {
    let object_info = object_info?;
    let mut out: Vec<(String, LocalModel)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for spec in MODEL_INPUTS {
        let Some(node) = object_info.get(spec.cls).and_then(Value::as_object) else {
            continue;
        };
        let Some(required) = node
            .get("input")
            .and_then(Value::as_object)
            .and_then(|i| i.get("required"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        let Some(decl) = required.get(spec.input).and_then(Value::as_array) else {
            continue;
        };
        let Some(enum_values) = decl.first().and_then(Value::as_array) else {
            continue;
        };
        for raw in enum_values {
            let Some(name) = raw.as_str() else { continue };
            if name.trim().is_empty() {
                continue;
            }
            // 同一个文件名可能挂在不同类下：以**先出现的类**为准，
            // 避免同一份文件在清单里出现两次
            let key = format!("{}|{}", spec.cls, name);
            if !seen.insert(key) {
                continue;
            }
            out.push((
                name.to_string(),
                LocalModel {
                    name: name.to_string(),
                    folder: spec.folder.to_string(),
                    path: None,
                    bytes: None,
                    visible_to_instance: true,
                },
            ));
        }
    }
    if out.is_empty() {
        return None;
    }
    let mut models: Vec<LocalModel> = out.into_iter().map(|(_, m)| m).collect();
    models.sort_by(|a, b| a.folder.cmp(&b.folder).then_with(|| a.name.cmp(&b.name)));
    let truncated = models.len() > MAX_ENTRIES;
    if truncated {
        models.truncate(MAX_ENTRIES);
    }
    Some(Inventory {
        source: Source::ObjectInfo,
        models,
        models_dir: None,
        truncated,
        disk_only: Vec::new(),
        merged: false,
    })
}

/// 扫盘时看的 ComfyUI 根：环境变量 / 用户配置的输出目录父目录 / 项目内 `comfyui`。
fn scan_roots(project_root: Option<&Path>) -> Vec<PathBuf> {
    let Some(root) = project_root else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = Vec::new();
    for home in crate::comfy::comfy_dirs::auto_read_roots(root, None) {
        // auto_read_roots 给的是"ComfyUI 根"与"它周围的目录"，只留真的有 models 的
        if home.join("models").is_dir() && !out.contains(&home) {
            out.push(home.clone());
        }
        // ComfyUI Desktop 把共享模型放在 ComfyUI-Shared 那一侧：它的 models 是兄弟目录
        let is_models_dir = home
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase().contains("models"))
            .unwrap_or(false);
        if is_models_dir {
            if let Some(parent) = home.parent() {
                if parent.is_dir() && !out.contains(&parent.to_path_buf()) {
                    out.push(parent.to_path_buf());
                }
            }
        }
    }
    out
}

/// 模型输入认得出的那些 `models\` 子目录（按首次出现去重，顺序稳定）。
fn model_folders() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for spec in MODEL_INPUTS {
        if !out.contains(&spec.folder) {
            out.push(spec.folder);
        }
    }
    out
}

/// 扫盘：在 ComfyUI 根底下看 `models\<folder>\**`（只读元数据），
/// 外加调用方给的真实模型根 `model_dirs`。
///
/// 有界：每个目录最多 [`MAX_ENTRIES`] 条、不跟随符号链接、深度 ≤ [`MAX_SCAN_DEPTH`]。
/// 扫不到就返回空清单（[`Source::None`]）—— 那是"这台机器没装 ComfyUI / 没放模型"的正常情形，
/// 不是错误；报告里会如实说"本机清单是空的，所以配不上任何模型"。
pub fn scan(project_root: Option<&Path>, model_dirs: &[PathBuf]) -> Inventory {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for root in scan_roots(project_root) {
        let dir = root.join("models");
        if dir.is_dir() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    for d in model_dirs {
        if d.is_dir() && !dirs.contains(d) {
            dirs.push(d.clone());
        }
    }
    if dirs.is_empty() {
        return Inventory {
            source: Source::None,
            models: Vec::new(),
            models_dir: None,
            truncated: false,
            disk_only: Vec::new(),
            merged: false,
        };
    }

    let key = dirs
        .iter()
        .map(|d| d.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(";");
    if let Ok(cache) = scan_cache().lock() {
        if let Some(hit) = cache.get(&key) {
            if hit.at.elapsed() < SCAN_TTL {
                return Inventory {
                    source: Source::Scan,
                    models: hit.models.clone(),
                    models_dir: hit.models_dir.clone(),
                    truncated: hit.truncated,
                    disk_only: Vec::new(),
                    merged: false,
                };
            }
        }
    }

    let mut found: Vec<(String, LocalModel)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut truncated = false;
    let mut models_dir: Option<PathBuf> = None;

    'outer: for dir in &dirs {
        if models_dir.is_none() {
            models_dir = Some(dir.clone());
        }
        for folder in model_folders() {
            let folder_dir = dir.join(folder);
            if !folder_dir.is_dir() {
                continue;
            }
            for file in list_files(&folder_dir) {
                if found.len() >= MAX_ENTRIES {
                    truncated = true;
                    break 'outer;
                }
                let name = relative_name(&folder_dir, &file);
                let map_key = format!("{folder}|{name}");
                if !seen.insert(map_key) {
                    continue;
                }
                found.push((
                    name.clone(),
                    LocalModel {
                        name,
                        folder: folder.to_string(),
                        path: Some(
                            crate::comfy::comfy_dirs::plain_path(file.clone())
                                .display()
                                .to_string(),
                        ),
                        bytes: std::fs::metadata(&file).ok().map(|m| m.len()),
                        visible_to_instance: false,
                    },
                ));
            }
        }
    }

    let mut models: Vec<LocalModel> = found.into_iter().map(|(_, m)| m).collect();
    models.sort_by(|a, b| a.folder.cmp(&b.folder).then_with(|| a.name.cmp(&b.name)));
    if let Ok(mut cache) = scan_cache().lock() {
        cache.insert(
            key,
            Cached {
                at: std::time::Instant::now(),
                models_dir: models_dir.clone(),
                models: models.clone(),
                truncated,
            },
        );
    }
    Inventory {
        source: Source::Scan,
        models,
        models_dir,
        truncated,
        disk_only: Vec::new(),
        merged: false,
    }
}

/// 相对 `folder_dir` 的路径，分隔符统一成 `/`（与 `/object_info` 给的形状一致）。
fn relative_name(dir: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(dir).unwrap_or(file);
    rel.to_string_lossy().replace('\\', "/")
}

/// 一个目录下所有认得出是模型的文件（带深度上限、不跟符号链接），按名字排序。
fn list_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    walk(dir, 0, &mut out);
    out.sort();
    out.truncate(MAX_ENTRIES);
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_SCAN_DEPTH || out.len() >= MAX_ENTRIES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_ENTRIES {
            return;
        }
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk(&path, depth + 1, out);
        } else if meta.is_file() && is_model_file(&path) {
            out.push(path);
        }
    }
}

fn is_model_file(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
        return false;
    };
    let lower = name.to_lowercase();
    // ⚠ **有意与 Kotlin 不同**（2026-09-27 真机抽查实测）：ComfyUI / Civitai 会给每个模型
    // 配一份同名 sidecar `xxx.metadata.json`（762 字节的 JSON，写着 sha256 / base_model /
    // 预览图 URL）。Kotlin 的 `MODEL_EXTS` 含 `json`，于是这 3 个 sidecar 被当成"盘上有、
    // 实例枚举不到"的模型 —— 报告据此喊"这个实例没挂共享模型根 → 让用户重启 ComfyUI"，
    // **是一个纯粹的假警报**（真的 `.safetensors` 就在旁边、实例看得到）。
    // 判据取最窄的那一条：只排除 `.metadata.json` 这个明确的后缀，别的 `json`/`yaml`
    // 照样收（GGUF / 工作流配置那些确实可能被 loader 读）。
    if lower.ends_with(".metadata.json") {
        return false;
    }
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    MODEL_EXTS.contains(&ext.as_str())
}

// ---------------------------------------------------------------------------
//  图里要哪些模型
// ---------------------------------------------------------------------------

/// 图里点名的**一个**模型。
#[derive(Debug, Clone, PartialEq)]
pub struct Requirement {
    pub node_id: String,
    pub class_type: String,
    pub input_name: String,
    pub file_name: String,
    pub folder: String,
    pub title: Option<String>,
    /// 1 = 必填（`/object_info` 说它是 required），0 = 可选
    pub required: bool,
}

impl Requirement {
    /// 覆盖时用的键（`comfy_submit(overrides=…)` 的键就是它）。
    pub fn override_key(&self) -> String {
        format!("{}.{}", self.node_id, self.input_name)
    }
}

/// 扫一份 **API 格式**节点图，列出它点名的所有模型文件。
///
/// 只认 [`MODEL_INPUTS`] 里那些**确定是文件名**的输入，且值必须是**字符串**：
/// 值是数组（连线）就跳过 —— 那是"从别的节点来的"，不是"从磁盘加载的"。
///
/// `model_inputs` 可选：用 `/object_info` 现算的"哪个类哪个输入是模型枚举"，
/// 用来补上 [`MODEL_INPUTS`] 里没写死的自定义节点。
pub fn requirements(
    graph: &Map<String, Value>,
    model_inputs: Option<&HashMap<String, HashSet<String>>>,
) -> Vec<Requirement> {
    let mut out: Vec<Requirement> = Vec::new();
    for (node_id, raw) in graph {
        let Some(node) = raw.as_object() else {
            continue;
        };
        let Some(cls) = node.get("class_type").and_then(Value::as_str) else {
            continue;
        };
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        let title = node
            .get("_meta")
            .and_then(Value::as_object)
            .and_then(|m| m.get("title"))
            .and_then(Value::as_str)
            .map(|s| s.to_string());

        let known: Vec<(&str, &str)> = MODEL_INPUTS
            .iter()
            .filter(|s| s.cls == cls)
            .map(|s| (s.input, s.folder))
            .collect();
        let mut all: Vec<(String, String)> = known
            .iter()
            .map(|(i, f)| (i.to_string(), f.to_string()))
            .collect();
        if let Some(extra) = model_inputs.and_then(|m| m.get(cls)) {
            for input in extra {
                if !all.iter().any(|(i, _)| i == input) {
                    all.push((input.clone(), folder_guess(input).to_string()));
                }
            }
        }

        for (input, folder) in all {
            let Some(value) = inputs.get(&input) else {
                continue;
            };
            let Some(name) = value.as_str() else { continue };
            if name.trim().is_empty() {
                continue;
            }
            out.push(Requirement {
                node_id: node_id.clone(),
                class_type: cls.to_string(),
                input_name: input,
                file_name: name.to_string(),
                folder,
                title: title.clone(),
                required: true,
            });
        }
    }
    // 每个覆盖键只留一条，按节点 id 数字升序（认不出来的排最后）
    let mut seen: HashSet<String> = HashSet::new();
    out.retain(|r| seen.insert(r.override_key()));
    out.sort_by_key(|r| r.node_id.parse::<i64>().unwrap_or(i64::MAX));
    out
}

/// 从 `/object_info` 反推一个模型枚举输入属于哪一类目录（报告里说"该放哪"用）。
fn folder_guess(input: &str) -> &'static str {
    let i = input.to_lowercase();
    if i.contains("ckpt") {
        "checkpoints"
    } else if i.contains("lora") {
        "loras"
    } else if i.contains("vae") {
        "vae"
    } else if i.contains("clip_vision") {
        "clip_vision"
    } else if i.contains("clip") {
        "text_encoders"
    } else if i.contains("control_net") {
        "controlnet"
    } else if i.contains("unet") || i.contains("model_name") {
        "diffusion_models"
    } else if i.contains("style") {
        "style_models"
    } else {
        "checkpoints"
    }
}

/// 从 `/object_info` 算出"哪些类的哪些输入是模型枚举"。
///
/// 判据只有一条：**这个输入的第一个声明是字符串数组**（那就是 ComfyUI 给的枚举），
/// 且里面有模型后缀的项。这样第三方加载器也能被认出来（不用维护一张越来越长的白名单），
/// 而 `INT` / `STRING`（自由文本）之类不会被误认。
pub fn enumerate_model_inputs(
    object_info: Option<&Map<String, Value>>,
) -> HashMap<String, HashSet<String>> {
    let mut out: HashMap<String, HashSet<String>> = HashMap::new();
    let Some(object_info) = object_info else {
        return out;
    };
    for (cls, raw) in object_info {
        let Some(node) = raw.as_object() else {
            continue;
        };
        let Some(input_section) = node.get("input").and_then(Value::as_object) else {
            continue;
        };
        for section_name in ["required", "optional"] {
            let Some(section) = input_section.get(section_name).and_then(Value::as_object) else {
                continue;
            };
            for (input, decl) in section {
                let Some(arr) = decl.as_array() else { continue };
                let Some(enum_values) = arr.first().and_then(Value::as_array) else {
                    continue;
                };
                let looks_like_files = enum_values.iter().any(|e| {
                    e.as_str()
                        .map(|s| {
                            MODEL_EXTS.contains(
                                &s.rsplit_once('.')
                                    .map(|(_, x)| x.to_lowercase())
                                    .unwrap_or_default()
                                    .as_str(),
                            )
                        })
                        .unwrap_or(false)
                });
                if looks_like_files {
                    out.entry(cls.clone()).or_default().insert(input.clone());
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
//  模型文件名的配对打分（`ModelName`）
// ---------------------------------------------------------------------------

/// 模型文件名的**配对打分**（用户建议 ① 的核心判据）。
///
/// 现场长这样：工作流点名 `krea2TurboOfficialComfy_krea2TurboInt8.safetensors`，
/// 而本机盘上那份叫 `krea2_turbo_int8_v2.safetensors` —— 字面上几乎不像，但**词根全对**。
/// 反过来 `sd_xl_base_1.0` 与 `sd_xl_refiner_1.0` 字面上很像，却**是两份不同的模型**。
///
/// 所以判据是**分词后的集合相似度**（Dice 系数），不是字符串编辑距离。
pub struct ModelName;

impl ModelName {
    /// 词根级相似度（0~1）。
    pub fn similarity(wanted: &str, local: &str) -> f64 {
        let a = Self::token_set(wanted);
        let b = Self::token_set(local);
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }
        if a == b {
            return 1.0;
        }
        let inter = a.intersection(&b).count() as f64;
        let dice = 2.0 * inter / (a.len() + b.len()) as f64;
        // 一个完整包含另一个（`krea2_turbo` ⊂ `krea2_turbo_int8_v2`）：给一点加权，
        // 免得"更完整的那个"因为分母变大反而被扣分
        let containment = if a.is_subset(&b) || b.is_subset(&a) {
            0.12
        } else {
            0.0
        };
        (dice + containment).min(1.0)
    }

    /// 严格相等（同名同后缀）：只有这一种情况才允许"不用问用户就替换"。
    pub fn same(wanted: &str, local: &str) -> bool {
        Self::tokens(wanted).into_iter().collect::<HashSet<_>>()
            == Self::tokens(local).into_iter().collect::<HashSet<_>>()
    }

    /// 把文件名变成一个**能拿去搜的查询词**（用户建议 ②，2026-09-22）。
    ///
    /// `krea2TurboOfficialComfy_krea2TurboInt8.safetensors` → `krea2 Turbo Int8`
    /// —— 拿整个文件名去 `civitai_search` 是搜不到的（那是一串打包名），但词根级别的词是能的。
    ///
    /// **这不是"猜模型名"**：它只是把用户图里那个文件名转成一句更好的搜索词。
    pub fn lookup_query(file_name: &str) -> String {
        let stop: HashSet<&str> = [
            "official", "comfy", "comfyui", "final", "fixed", "copy", "new", "old", "v", "ver",
            "version", "model",
        ]
        .into_iter()
        .collect();
        // ⚠ 顺序要紧：**先归一化去重，再截断**。
        //  ① 打包名里同一个词会出现两遍，先去重才能保证 `take(N)` 用的是不重复的词；
        //  ② 单字**数字**（int8 的那个 `8`）必须留着 —— 它是"量化位宽"里最有信息量的一位。
        let kept: Vec<String> = Self::tokens(file_name)
            .into_iter()
            .filter(|t| !stop.contains(t.as_str()))
            .filter(|t| t.chars().count() > 1 || t.chars().all(|c| c.is_ascii_digit()))
            .collect();
        if kept.is_empty() {
            // 全被停用词吃掉时**退回原文件名**（空查询词会被 civitai_search 当场拒掉）
            return file_name.trim().chars().take(80).collect();
        }
        // 再把"字母后紧跟的数字"粘回去（`krea` + `2` → `krea2`）
        let mut merged: Vec<String> = Vec::new();
        for w in kept.into_iter().take(6) {
            let prev_last_is_alpha = merged
                .last()
                .and_then(|p: &String| p.chars().last())
                .map(|c| c.is_alphabetic())
                .unwrap_or(false);
            if prev_last_is_alpha && w.chars().all(|c| c.is_ascii_digit()) {
                let last = merged.len() - 1;
                merged[last] = format!("{}{}", merged[last], w);
            } else {
                merged.push(w);
            }
        }
        merged.join(" ")
    }

    /// 归一化成一个可比较的词序列（去重、保留首次出现顺序）。
    pub fn tokens(raw: &str) -> Vec<String> {
        let no_ext = match raw.rsplit_once('.') {
            Some((head, _)) => head,
            None => raw,
        };
        // ⚠ 大小写边界要在**动过大小写之前**判（`to_lowercase()` 之后
        // `krea2TurboOfficialComfy` 已经看不出词界了）。
        // 相邻判定看的是**原串**的邻居（与 Kotlin 逐字一致）。
        let chars: Vec<char> = no_ext.chars().collect();
        let mut spaced = String::new();
        for (i, c) in chars.iter().enumerate() {
            let prev = if i > 0 { Some(chars[i - 1]) } else { None };
            if " -_.\\/|+()[]{}".contains(*c) {
                spaced.push(' ');
            } else if c.is_uppercase() && prev.map(|p| p.is_lowercase()).unwrap_or(false) {
                // 小写 → 大写：驼峰词界（`krea2Turbo` → `krea2` + `Turbo`）
                spaced.push(' ');
                spaced.push(*c);
            } else if c.is_ascii_digit() && prev.map(|p| p.is_alphabetic()).unwrap_or(false) {
                // 字母 → 数字（`krea2`）
                spaced.push(' ');
                spaced.push(*c);
            } else if c.is_alphabetic() && prev.map(|p| p.is_ascii_digit()).unwrap_or(false) {
                // 数字 → 字母（`flux1dev` → flux/1/dev）
                spaced.push(' ');
                spaced.push(*c);
            } else {
                spaced.push(*c);
            }
        }
        let mut out: Vec<String> = Vec::new();
        for token in spaced.to_lowercase().split(' ') {
            if token.trim().is_empty() {
                continue;
            }
            if !out.iter().any(|t| t == token) {
                out.push(token.to_string());
            }
        }
        out
    }

    /// 词集合（比较用）。
    pub fn token_set(raw: &str) -> HashSet<String> {
        Self::tokens(raw).into_iter().collect()
    }
}

// ---------------------------------------------------------------------------
//  工具：comfy_env_report
// ---------------------------------------------------------------------------

/// `comfy_env_report`：本机 ComfyUI 环境对账（**只读**，`ToolRegistry.kt` 的逐条对照）。
pub async fn tool_env_report(
    state: &AppState,
    arguments: &Value,
) -> Result<ToolOutcome, ToolFailure> {
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
    let inv = inventory(object_info.as_ref(), Some(&project_root), &model_dirs);
    let configured_output = config.as_ref().and_then(|c| c.output_dir.clone());

    let folder = arguments
        .get("folder")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let limit = arguments
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(40)
        .clamp(1, 200) as usize;

    let visible = inv.models.iter().filter(|m| m.visible_to_instance).count();
    let matched: Vec<&LocalModel> = inv
        .models
        .iter()
        .filter(|m| {
            folder
                .as_deref()
                .map(|f| m.folder.eq_ignore_ascii_case(f))
                .unwrap_or(true)
                && query
                    .as_deref()
                    .map(|q| m.name.to_lowercase().contains(&q.to_lowercase()))
                    .unwrap_or(true)
        })
        .collect();

    let mut text = String::new();
    text.push_str("【本机 ComfyUI 环境对账】\n");
    text.push_str(&format!("· 实例能加载：{visible} 个模型文件"));
    if object_info.is_none() {
        text.push_str(
            "（**没问到 /object_info** —— ComfyUI 没在跑，下面\"实例看不到\"那一栏不可信）",
        );
    } else {
        text.push_str("（= `/object_info` 的枚举，这就是**提交时真正能加载的**那些）");
    }
    text.push('\n');
    text.push_str(&format!("· 盘上扫到：{} 个\n", inv.models.len()));
    if !inv.disk_only.is_empty() {
        text.push_str(&format!(
            "· ⚠ **盘上有、但当前实例枚举不到的：{} 个** —— 这通常意味着这个实例没挂上共享模型根（不是\"本机没有\"）。\n",
            inv.disk_only.len()
        ));
        for m in inv.disk_only.iter().take(8) {
            text.push_str(&format!("    - {}/{}", m.folder, m.name));
            if let Some(gb) = m.size_gb() {
                text.push_str(&format!("（{gb:.2}GB）"));
            }
            if let Some(p) = &m.path {
                text.push_str(&format!("  {p}"));
            }
            text.push('\n');
        }
        if inv.disk_only.len() > 8 {
            text.push_str(&format!("    …（还有 {} 个）\n", inv.disk_only.len() - 8));
        }
        text.push_str(
            "  → 正确动作：让用户**重启 ComfyUI**（ComfyHub 起实例时会自动挂上 ComfyUI Desktop 的模型根），**不要去下载**（文件就在硬盘上）。\n",
        );
    }
    text.push_str(&format!(
        "· 自动捕获盯着的目录：output = {}",
        configured_output
            .clone()
            .unwrap_or_else(|| "（没配）".to_string())
    ));
    if let Some(d) = &desktop {
        if let Some(input) = &d.input_dir {
            text.push_str(&format!("，input = {}", input.display()));
        }
    }
    text.push('\n');
    match &desktop {
        Some(env) => {
            let dirs: Vec<String> = env
                .models_dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect();
            text.push_str(&format!(
                "· ComfyUI Desktop 的模型根（来源：{}）：",
                env.source
            ));
            text.push_str(if dirs.is_empty() {
                "（没读到）"
            } else {
                ""
            });
            if !dirs.is_empty() {
                text.push_str(&dirs.join("、"));
            }
            text.push('\n');
            if let Some(cfg) = &env.model_paths_config {
                text.push_str(&format!("  启动时挂的映射文件：{}\n", cfg.display()));
            }
        }
        None => {
            text.push_str(
                "· 没读到 ComfyUI Desktop 的配置 —— 如果用户的模型放在共享目录或别的盘，实例可能看不到它们（见上面那一栏）。\n",
            );
        }
    }
    if folder.is_some() || query.is_some() {
        text.push_str("\n【清单");
        if let Some(f) = &folder {
            text.push_str(&format!(" folder={f}"));
        }
        if let Some(q) = &query {
            text.push_str(&format!(" query={q}"));
        }
        text.push_str(&format!("】命中 {}", matched.len()));
        if matched.len() > limit {
            text.push_str(&format!("（只列前 {limit} 个）"));
        }
        text.push_str("：\n");
        if matched.is_empty() {
            text.push_str("  （一个都没有）\n");
        }
        for m in matched.iter().take(limit) {
            text.push_str(&format!("  {}/{}", m.folder, m.name));
            if !m.visible_to_instance {
                text.push_str("  ← **实例看不到**");
            }
            if let Some(gb) = m.size_gb() {
                text.push_str(&format!("（{gb:.2}GB）"));
            }
            text.push('\n');
        }
    } else {
        text.push_str(
            "\n（要看具体清单就带 folder（比如 checkpoints / loras）或 query 再调一次）\n",
        );
    }

    let mut payload = Map::new();
    payload.insert("instanceLoadable".into(), json!(visible));
    payload.insert("onDisk".into(), json!(inv.models.len()));
    payload.insert("invisibleToInstance".into(), json!(inv.disk_only.len()));
    payload.insert("merged".into(), json!(inv.merged));
    payload.insert("inventorySource".into(), json!(inv.source.wire()));
    if let Some(d) = &inv.models_dir {
        payload.insert("modelsDir".into(), json!(d.display().to_string()));
    }
    if let Some(o) = &configured_output {
        payload.insert("captureOutputDir".into(), json!(o));
    }
    if let Some(d) = &desktop {
        if let Some(i) = &d.input_dir {
            payload.insert("inputDir".into(), json!(i.display().to_string()));
        }
        if let Some(o) = &d.output_dir {
            payload.insert("desktopOutputDir".into(), json!(o.display().to_string()));
        }
        payload.insert("desktopSource".into(), json!(d.source));
        payload.insert(
            "modelsDirs".into(),
            json!(d
                .models_dirs
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()),
        );
        if let Some(cfg) = &d.model_paths_config {
            payload.insert("modelPathsConfig".into(), json!(cfg.display().to_string()));
        }
    }
    if let Some(f) = &folder {
        payload.insert("folder".into(), json!(f));
    }
    if let Some(q) = &query {
        payload.insert("query".into(), json!(q));
    }
    payload.insert("matched".into(), json!(matched.len()));
    payload.insert(
        "models".into(),
        Value::Array(
            matched
                .iter()
                .take(limit)
                .map(|m| {
                    let mut o = Map::new();
                    o.insert("name".into(), json!(m.name));
                    o.insert("folder".into(), json!(m.folder));
                    o.insert("visibleToInstance".into(), json!(m.visible_to_instance));
                    if let Some(p) = &m.path {
                        o.insert("path".into(), json!(p));
                    }
                    if let Some(gb) = m.size_gb() {
                        o.insert("sizeGB".into(), json!((gb * 100.0).round() / 100.0));
                    }
                    Value::Object(o)
                })
                .collect(),
        ),
    );
    Ok(ToolOutcome::new(text, Value::Object(payload)))
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
