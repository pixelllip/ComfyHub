//! API 节点图 → 提示词字段（Kotlin `GraphParse` 的逐行对照，P4 第 43 步第一半）。
//!
//! # 它是干什么的
//!
//! 捕获一次 ComfyUI 运行之后，我们要从"当时真正跑的那份节点图"里反推出人看得懂的那些字段：
//! 正向 / 负向提示词、checkpoint、LoRA 链、采样器 / 调度器 / steps / cfg / seed、
//! 宽高与批量、产物类型（图 / 视频 / 音频），以及"这条工作流叫什么"的标题线索。
//! 这些字段直接落进 `prompts` 表 —— 判错的表现是**画廊里那条记录的参数是错的**，
//! 而没有任何报错。
//!
//! # 为什么是"启发式"，以及为什么顺序是承重的
//!
//! 节点图是**任意**的（自定义节点、组节点、转发节点……），没有一份"标准图"可依赖，
//! 所以 Kotlin 那边写的是"按优先级一路兜底"：先看采样器自己的输入，再看链上的兄弟节点，
//! 最后全图扫描。那些 `firstOrNull { ... }` **取的是文档顺序里的第一个**（Kotlin 的
//! `JsonObject` 是插入序），所以这里必须同样按文档顺序遍历 —— 这也是本仓库
//! 给 `serde_json` 打开 `preserve_order` 的**唯一理由**（见 `Cargo.toml` 那条注释）：
//! 用默认的 BTreeMap（键排序）时，多候选图上"第一个命中的节点"会与 Kotlin 不同，
//! 于是**同一次运行在两个后端解析出不同的 seed / 提示词**，而且不报错。
//!
//! # 一处**有意**修正
//!
//! `longestTextInput` / `negativeTextInput` 是"取最长的那条"。Kotlin 的 `maxByOrNull`
//! 在**并列时取第一个**，而 Rust 的 `max_by_key` 在并列时取**最后一个** —— 直接照搬会让
//! 并列的图（很常见：一个空负向、一个负向关键词表长度相同）两端选到不同的节点。
//! 这里显式实现"取第一个最大"，并有用例钉住。

use serde_json::{Map, Value};

/// 一条 LoRA 引用（Kotlin `LoraRef`）。
#[derive(Debug, Clone, PartialEq)]
pub struct LoraRef {
    pub name: String,
    pub weight: f64,
}

/// 解析结果；字段与 `prompts` 表一一对应（Kotlin `GraphParse.Parsed`）。
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub kind: String,
    pub positive: String,
    pub negative: Option<String>,
    pub checkpoint: Option<String>,
    pub loras: Vec<LoraRef>,
    pub sampler: Option<String>,
    pub scheduler: Option<String>,
    pub steps: Option<i64>,
    pub cfg: Option<f64>,
    pub seed: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub batch: Option<i64>,
    /// 其它散落参数，键名形如 `KSampler.denoise`；**顺序即语义**（Kotlin 是 LinkedHashMap）
    pub extra: Vec<(String, String)>,
    /// 建议的标题前缀（取自保存节点的 `filename_prefix`）
    pub title_hint: Option<String>,
}

impl Default for Parsed {
    fn default() -> Self {
        Self {
            kind: "IMAGE".to_string(),
            positive: String::new(),
            negative: None,
            checkpoint: None,
            loras: Vec::new(),
            sampler: None,
            scheduler: None,
            steps: None,
            cfg: None,
            seed: None,
            width: None,
            height: None,
            batch: None,
            extra: Vec::new(),
            title_hint: None,
        }
    }
}

/// 视频类节点的名字片段（小写比较）。
const VIDEO_CLASS_HINTS: [&str; 12] = [
    "videocombine",
    "savevideo",
    "savewebm",
    "createvideo",
    "saveanimatedwebp",
    "vhs_",
    "wanvideo",
    "saveanimatedpng",
    "svd",
    "imgtovideo",
    "minimaxh3",
    // 占位：Kotlin 是 11 条，这里多留一条不影响（下面用 `iter().any`）
    "",
];
/// 音频类节点的名字片段。
const AUDIO_CLASS_HINTS: [&str; 5] =
    ["saveaudio", "previewaudio", "audioupload", "saveaudiomp3", "voiceclone"];

/// 节点上哪些标量输入算"提示词正文"。
const TEXT_INPUT_KEYS: [&str; 10] = [
    "text",
    "prompt",
    "positive",
    "positive_prompt",
    "system_prompt",
    "text_g",
    "text_l",
    "caption",
    "prompt_text",
    "instruction",
];

/// 负向提示词的键名线索。
const NEGATIVE_HINT_KEYS: [&str; 3] = ["negative", "negative_prompt", "neg"];

/// 继续下钻时认这些"条件 / 文本"转发键。
const CHAIN_KEYS: [&str; 6] = ["conditioning", "cond", "positive", "negative", "clip", "prompt"];

/// 已经单独入库的字段，不再重复塞进 `extra_params`。
const SKIP_INPUT_KEYS: [&str; 32] = [
    "text",
    "prompt",
    "positive",
    "negative",
    "seed",
    "noise_seed",
    "steps",
    "cfg",
    "sampler_name",
    "scheduler",
    "width",
    "height",
    "batch_size",
    "ckpt_name",
    "lora_name",
    "strength_model",
    "model",
    "clip",
    "vae",
    "latent_image",
    "samples",
    "images",
    "filename_prefix",
    "model_name",
    "unet_name",
    "length",
    "batch",
    "audio",
    "video",
    // 下表这三条 Kotlin 原文里就有（`SKIP_INPUT_KEYS` 32 项），别删
    "scheduler_name",
    "sampler",
    "noise",
];

/// 解析一份 API 节点图（`null` / 空图 → 全默认，**不报错**）。
pub fn parse(graph: Option<&Map<String, Value>>) -> Parsed {
    let Some(graph) = graph else { return Parsed::default() };
    if graph.is_empty() {
        return Parsed::default();
    }
    // 只保留"是对象"的条目（脏数据里可能混着字符串），并保持**文档顺序**
    let nodes: Vec<(&str, &Map<String, Value>)> = graph
        .iter()
        .filter_map(|(id, value)| value.as_object().map(|obj| (id.as_str(), obj)))
        .collect();
    if nodes.is_empty() {
        return Parsed::default();
    }

    let sampler = find_sampler(&nodes);
    let guider = sampler
        .and_then(|node| input_ref(node, "guider"))
        .and_then(|id| lookup(&nodes, &id))
        .or_else(|| {
            sampler
                .and_then(|node| input_ref(node, "model"))
                .and_then(|id| lookup(&nodes, &id))
                .filter(|node| cls(node).contains("Guider"))
        });

    // --- 采样参数：先看采样器自己，再看链上的兄弟节点，最后全图兜底 ---
    let sigmas = sampler.and_then(|node| input_ref(node, "sigmas")).and_then(|id| lookup(&nodes, &id));
    let sampler_select =
        sampler.and_then(|node| input_ref(node, "sampler")).and_then(|id| lookup(&nodes, &id));
    let noise = sampler.and_then(|node| input_ref(node, "noise")).and_then(|id| lookup(&nodes, &id));

    let steps_node = [sampler, sigmas, guider]
        .into_iter()
        .flatten()
        .find(|node| input_int(node, "steps").is_some())
        .or_else(|| {
            nodes
                .iter()
                .map(|(_, node)| *node)
                .find(|node| cls(node).to_lowercase().contains("scheduler") && input_int(node, "steps").is_some())
        })
        .or_else(|| nodes.iter().map(|(_, node)| *node).find(|node| input_int(node, "steps").is_some()));

    let sampler_name = sampler
        .and_then(|node| input_text(node, "sampler_name"))
        .or_else(|| sampler_select.and_then(|node| input_text(node, "sampler_name")))
        .or_else(|| {
            nodes
                .iter()
                .map(|(_, node)| *node)
                .find(|node| cls(node).to_lowercase().contains("samplerselect"))
                .and_then(|node| input_text(node, "sampler_name"))
        })
        .or_else(|| {
            nodes
                .iter()
                .map(|(_, node)| *node)
                .find(|node| input_text(node, "sampler_name").is_some())
                .and_then(|node| input_text(node, "sampler_name"))
        });

    let scheduler_name = sampler
        .and_then(|node| input_text(node, "scheduler"))
        .or_else(|| sigmas.and_then(|node| input_text(node, "scheduler")))
        .or_else(|| {
            nodes
                .iter()
                .map(|(_, node)| *node)
                .find(|node| input_text(node, "scheduler").is_some())
                .and_then(|node| input_text(node, "scheduler"))
        });

    let cfg = sampler
        .and_then(|node| input_double(node, "cfg"))
        .or_else(|| guider.and_then(|node| input_double(node, "cfg")))
        .or_else(|| {
            nodes
                .iter()
                .map(|(_, node)| *node)
                .find(|node| cls(node).contains("Guider") && input_double(node, "cfg").is_some())
                .and_then(|node| input_double(node, "cfg"))
        })
        .or_else(|| {
            nodes
                .iter()
                .map(|(_, node)| *node)
                .find(|node| input_double(node, "cfg").is_some())
                .and_then(|node| input_double(node, "cfg"))
        });

    let seed = sampler
        .and_then(|node| input_long(node, "seed"))
        .or_else(|| sampler.and_then(|node| input_long(node, "noise_seed")))
        .or_else(|| noise.and_then(|node| input_long(node, "noise_seed")))
        .or_else(|| noise.and_then(|node| input_long(node, "seed")))
        .or_else(|| {
            nodes
                .iter()
                .map(|(_, node)| *node)
                .find(|node| input_long(node, "noise_seed").is_some())
                .and_then(|node| input_long(node, "noise_seed"))
        })
        .or_else(|| {
            nodes
                .iter()
                .map(|(_, node)| *node)
                .find(|node| input_long(node, "seed").is_some())
                .and_then(|node| input_long(node, "seed"))
        });

    // --- 提示词：优先沿采样器 / 引导器的条件链找，找不到再退化 ---
    let positive = first_non_blank([
        sampler.and_then(|node| text_from_node(node, &nodes, 0)),
        sampler.and_then(|node| text_from_ref(input(node, "positive"), &nodes, 0)),
        guider.and_then(|node| text_from_ref(input(node, "conditioning"), &nodes, 0)),
        guider.and_then(|node| text_from_ref(input(node, "positive"), &nodes, 0)),
        sampler.and_then(|node| text_from_ref(input(node, "positive_cond"), &nodes, 0)),
    ])
    .or_else(|| longest_text_input(&nodes).map(|(_, text)| text));

    let negative = first_non_blank([
        sampler.and_then(|node| text_from_ref(input(node, "negative"), &nodes, 0)),
        guider.and_then(|node| text_from_ref(input(node, "negative"), &nodes, 0)),
        negative_text_input(&nodes).map(|(_, text)| text),
    ]);

    let (loras, checkpoint) = walk_model(sampler, guider, &nodes);
    let dims = resolve_dimensions(sampler.and_then(|node| input(node, "latent_image")), &nodes);

    let kind = detect_kind(&nodes);
    let positive_text = positive.clone().unwrap_or_default();
    Parsed {
        kind,
        positive: positive_text.clone(),
        // 与正向相同、或空白的负向一律丢掉（Kotlin `takeIf { it.isNotBlank() && it != positive }`）
        negative: negative
            .filter(|text| !text.trim().is_empty() && *text != positive_text),
        checkpoint: checkpoint.or_else(|| find_checkpoint(&nodes)),
        loras,
        sampler: sampler_name,
        scheduler: scheduler_name,
        steps: steps_node.and_then(|node| input_int(node, "steps")).map(|v| v as i64),
        cfg,
        seed,
        width: dims.map(|(w, _, _)| w as i64),
        height: dims.map(|(_, h, _)| h as i64),
        batch: dims.and_then(|(_, _, b)| b).map(|v| v as i64),
        extra: collect_extra(&nodes),
        title_hint: find_filename_prefix(&nodes),
    }
}

// ---------------------------------------------------------------------------
//  采样器定位
// ---------------------------------------------------------------------------

/// 找采样器（Kotlin `findSampler`：五种判据**顺序不能换**）。
fn find_sampler<'a>(nodes: &[(&'a str, &'a Map<String, Value>)]) -> Option<&'a Map<String, Value>> {
    let all: Vec<&Map<String, Value>> = nodes.iter().map(|(_, node)| *node).collect();
    // 经典 KSampler：steps + cfg 都在自己身上
    if let Some(node) = all.iter().find(|n| input_int(n, "steps").is_some() && input_double(n, "cfg").is_some()) {
        return Some(node);
    }
    // 自定义采样链的根节点：有 sigmas 且（有 noise 或 guider）
    if let Some(node) = all
        .iter()
        .find(|n| input(n, "sigmas").is_some() && (input(n, "noise").is_some() || input(n, "guider").is_some()))
    {
        return Some(node);
    }
    if let Some(node) = all.iter().find(|n| cls(n).to_lowercase().contains("samplercustom")) {
        return Some(node);
    }
    // 注意 KSamplerSelect 也含 "KSampler"，但它只有 sampler_name，别优先选它
    if let Some(node) = all
        .iter()
        .find(|n| cls(n).to_lowercase().contains("ksampler") && input(n, "sampler_name").is_none())
    {
        return Some(node);
    }
    if let Some(node) = all.iter().find(|n| cls(n).to_lowercase().contains("ksampler")) {
        return Some(node);
    }
    all.iter().find(|n| cls(n).to_lowercase().contains("sampler")).copied()
}

/// 顺着 `model` 引用往回走，收集 LoRA 链和模型名（Kotlin `walkModel`）。
fn walk_model(
    sampler: Option<&Map<String, Value>>,
    guider: Option<&Map<String, Value>>,
    nodes: &[(&str, &Map<String, Value>)],
) -> (Vec<LoraRef>, Option<String>) {
    // 同名 LoRA 只留第一次（Kotlin 是 LinkedHashMap + putIfAbsent）—— 顺序即语义
    let mut loras: Vec<LoraRef> = Vec::new();
    let mut checkpoint: Option<String> = None;

    let starts: Vec<String> = [
        sampler.and_then(|node| input_ref(node, "model")),
        guider.and_then(|node| input_ref(node, "model")),
        sampler.and_then(|node| input_ref(node, "guider")),
    ]
    .into_iter()
    .flatten()
    .collect();

    for start in starts {
        let mut reference = Some(start);
        let mut guard = 0;
        while let Some(current) = reference {
            guard += 1;
            if guard > 40 {
                break;
            }
            let Some(node) = lookup(nodes, &current) else { break };
            let class = cls(node);

            if class.to_lowercase().contains("lora") {
                let raw = input(node, "lora_name")
                    .or_else(|| input(node, "lora"))
                    .or_else(|| input(node, "lora_path"));
                let names: Vec<String> = match raw {
                    Some(Value::String(text)) => vec![text.clone()],
                    Some(Value::Array(items)) => items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect(),
                    _ => Vec::new(),
                };
                let weight = input_double(node, "strength_model")
                    .or_else(|| input_double(node, "strength"))
                    .or_else(|| input_double(node, "weight"))
                    .unwrap_or(1.0);
                for name in names.into_iter().filter(|name| !name.trim().is_empty()) {
                    // 只留文件名（两种分隔符都要认：ComfyUI 的 lora 名可能是 `a\b\lora.safetensors`）
                    let short = name.rsplit(['/', '\\']).next().unwrap_or(&name).to_string();
                    if !loras.iter().any(|existing| existing.name == short) {
                        loras.push(LoraRef { name: short, weight });
                    }
                }
            }

            if checkpoint.is_none() {
                for key in ["ckpt_name", "model_name", "unet_name", "gguf_name", "diffusion_model"] {
                    if let Some(text) = input_text(node, key).filter(|text| !text.trim().is_empty()) {
                        checkpoint = Some(text);
                        break;
                    }
                }
            }

            reference = input_ref(node, "model")
                .or_else(|| input_ref(node, "MODEL"))
                .or_else(|| {
                    input_ref(node, "guider")
                        .and_then(|id| lookup(nodes, &id))
                        .and_then(|next| input_ref(next, "model"))
                });
        }
    }
    (loras, checkpoint)
}

/// 兜底找 checkpoint（Kotlin `findCheckpoint`）。
fn find_checkpoint(nodes: &[(&str, &Map<String, Value>)]) -> Option<String> {
    for (_, node) in nodes {
        if cls(node).to_lowercase().contains("checkpointloader") {
            if let Some(text) = input_text(node, "ckpt_name").filter(|t| !t.trim().is_empty()) {
                return Some(text);
            }
        }
    }
    for key in ["ckpt_name", "model_name", "unet_name", "gguf_name", "diffusion_model"] {
        for (_, node) in nodes {
            let class = cls(node);
            if class.to_lowercase().contains("loader") || class.to_lowercase().contains("unet") {
                if let Some(text) = input_text(node, key).filter(|t| !t.trim().is_empty()) {
                    return Some(text);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
//  文本
// ---------------------------------------------------------------------------

/// 节点自身的文本输入；没有就顺着条件引用往下找（Kotlin `textFromNode`）。
fn text_from_node(
    node: &Map<String, Value>,
    nodes: &[(&str, &Map<String, Value>)],
    depth: usize,
) -> Option<String> {
    if depth > 6 {
        return None;
    }
    for key in TEXT_INPUT_KEYS {
        if let Some(text) = input_text(node, key).filter(|t| !t.trim().is_empty()) {
            return Some(text);
        }
    }
    for key in CHAIN_KEYS {
        if let Some(text) = text_from_ref(input(node, key), nodes, depth + 1) {
            return Some(text);
        }
    }
    None
}

/// 把 `["节点id", 槽位]` 解析成文本，能穿透转发节点与原始值节点（Kotlin `textFromRef`）。
fn text_from_ref(
    reference: Option<&Value>,
    nodes: &[(&str, &Map<String, Value>)],
    depth: usize,
) -> Option<String> {
    let reference = reference?;
    if depth > 6 {
        return None;
    }
    // 直接就是字符串（有些节点把文本内联在连线位置上）
    if let Some(text) = reference.as_str() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(text.to_string());
        }
    }
    let id = reference_ref(Some(reference))?;
    let node = lookup(nodes, &id)?;

    for key in TEXT_INPUT_KEYS {
        if let Some(text) = input_text(node, key).filter(|t| !t.trim().is_empty()) {
            return Some(text);
        }
    }
    // 上游是 String / Primitive 之类的原始值节点
    let class = cls(node).to_lowercase();
    if class.contains("primitive") || class.contains("string") || class.contains("text") {
        for key in ["value", "string", "text", "prompt"] {
            if let Some(text) = input_text(node, key).filter(|t| !t.trim().is_empty()) {
                return Some(text);
            }
        }
    }
    for key in CHAIN_KEYS {
        if let Some(text) = text_from_ref(input(node, key), nodes, depth + 1) {
            return Some(text);
        }
    }
    None
}

/// 全图兜底：最长的那个文本输入通常就是正向提示词。
///
/// ⚠ **并列取第一个**（Kotlin `maxByOrNull` 的语义）：Rust 的 `max_by_key` 并列时取**最后一个**，
/// 直接用它会让"一个空负向 + 一个等长负向表"这类常见图选到不同节点。
fn longest_text_input(
    nodes: &[(&str, &Map<String, Value>)],
) -> Option<(String, String)> {
    let mut best: Option<(String, String)> = None;
    for (_, node) in nodes {
        for key in TEXT_INPUT_KEYS {
            if let Some(text) = input_text(node, key).filter(|t| !t.trim().is_empty()) {
                let better = match &best {
                    Some((_, current)) => text.len() > current.len(),
                    None => true,
                };
                if better {
                    best = Some((key.to_string(), text));
                }
                break; // 一个节点只取第一个命中的键（Kotlin `firstNotNullOfOrNull`）
            }
        }
    }
    best
}

/// 负向提示词兜底：键名或类名里带 negative 的文本输入（并列同样取第一个）。
fn negative_text_input(
    nodes: &[(&str, &Map<String, Value>)],
) -> Option<(String, String)> {
    let mut best: Option<(String, String)> = None;
    for (_, node) in nodes {
        let by_key = NEGATIVE_HINT_KEYS.iter().find_map(|key| {
            input_text(node, key)
                .filter(|t| !t.trim().is_empty())
                .map(|text| (key.to_string(), text))
        });
        let found = by_key.or_else(|| {
            if cls(node).to_lowercase().contains("negative") {
                TEXT_INPUT_KEYS.iter().find_map(|key| {
                    input_text(node, key)
                        .filter(|t| !t.trim().is_empty())
                        .map(|text| (key.to_string(), text))
                })
            } else {
                None
            }
        });
        if let Some((key, text)) = found {
            let better = match &best {
                Some((_, current)) => text.len() > current.len(),
                None => true,
            };
            if better {
                best = Some((key, text));
            }
        }
    }
    best
}

// ---------------------------------------------------------------------------
//  尺寸 / 类型 / 其它参数
// ---------------------------------------------------------------------------

/// 顺着 `latent_image` 找宽高与批量；解析不出来就 `None`（宽高会在 extra 里留线索）。
fn resolve_dimensions(
    reference: Option<&Value>,
    nodes: &[(&str, &Map<String, Value>)],
) -> Option<(i32, i32, Option<i32>)> {
    let mut candidates: Vec<&Map<String, Value>> = Vec::new();
    if let Some(node) = reference_ref(reference).and_then(|id| lookup(nodes, &id)) {
        candidates.push(node);
    }
    // 常见 latent 节点兜底
    if let Some(node) = nodes.iter().map(|(_, n)| *n).find(|node| {
        let class = cls(node).to_lowercase();
        (class.contains("emptylatent")
            || class.contains("emptysd3")
            || class.contains("emptyhunyuan")
            || class.contains("emptymochi")
            || class.contains("emptyimage"))
            && input_int(node, "width").is_some()
    }) {
        candidates.push(node);
    }

    for node in candidates {
        let width = input_int(node, "width");
        let height = input_int(node, "height");
        let batch = input_int(node, "batch_size").or_else(|| input_int(node, "batch"));
        if let (Some(w), Some(h)) = (width, height) {
            return Some((w, h, batch));
        }
    }
    None
}

/// 产物类型（Kotlin `detectKind`）：视频 / 音频 / 图，混合算 `MIXED`。
fn detect_kind(nodes: &[(&str, &Map<String, Value>)]) -> String {
    let mut video = false;
    let mut audio = false;
    let mut image = false;
    for (_, node) in nodes {
        let class = cls(node).to_lowercase();
        if VIDEO_CLASS_HINTS.iter().any(|hint| !hint.is_empty() && class.contains(hint)) {
            video = true;
        } else if AUDIO_CLASS_HINTS.iter().any(|hint| class.contains(hint)) {
            audio = true;
        } else {
            image = true;
        }
    }
    if video && audio {
        "MIXED".to_string()
    } else if video {
        "VIDEO".to_string()
    } else if audio && !image {
        "AUDIO".to_string()
    } else {
        "IMAGE".to_string()
    }
}

/// 建议的标题前缀（Kotlin `findFilenamePrefix`）。
fn find_filename_prefix(nodes: &[(&str, &Map<String, Value>)]) -> Option<String> {
    let prefixes: Vec<String> = nodes
        .iter()
        .filter_map(|(_, node)| {
            let class = cls(node);
            let is_saver = class.to_lowercase().contains("save")
                || class.to_lowercase().contains("videocombine");
            if !is_saver {
                return None;
            }
            input_text(node, "filename_prefix")
        })
        .filter(|text| !text.trim().is_empty())
        .collect();

    let raw = prefixes.first()?;
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw).to_string();
    let trimmed = base.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("ComfyUI") {
        None
    } else {
        Some(base)
    }
}

/// 其它散落参数（Kotlin `collectExtra`）：**最多 60 条**，键名 `类名.输入名`。
fn collect_extra(nodes: &[(&str, &Map<String, Value>)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    'outer: for (id, node) in nodes {
        let class = cls(node).split('(').next().unwrap_or_default().trim().to_string();
        for (key, value) in input_pairs(node) {
            if SKIP_INPUT_KEYS.contains(&key.as_str()) {
                continue;
            }
            let Some(text) = primitive_text(&value) else { continue };
            if text.trim().is_empty() || text.chars().count() > 400 {
                continue;
            }
            out.push((format!("{class}.{key}"), text));
            if out.len() >= 60 {
                break 'outer;
            }
        }
        if out.len() < 60 && !class.is_empty() && !out.iter().any(|(k, _)| k == &format!("{class}.__node_id")) {
            out.push((format!("{class}.__node_id"), id.to_string()));
        }
    }
    out
}

// ---------------------------------------------------------------------------
//  JsonElement 小工具（对 null / 类型不符都安全）
// ---------------------------------------------------------------------------

/// `class_type`（Kotlin `JsonObject.cls()`）。
fn cls(node: &Map<String, Value>) -> String {
    node.get("class_type").and_then(Value::as_str).unwrap_or_default().to_string()
}

/// `inputs` 对象（Kotlin `JsonObject.inputs()`）。
fn inputs(node: &Map<String, Value>) -> Option<&Map<String, Value>> {
    node.get("inputs").and_then(Value::as_object)
}

/// 某个输入（Kotlin `JsonObject.input(key)`）。
fn input<'a>(node: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    inputs(node)?.get(key)
}

/// `inputs` 的键值对（顺序 = 文档顺序）。
fn input_pairs(node: &Map<String, Value>) -> Vec<(String, Value)> {
    inputs(node)
        .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

/// 原始值 → 文本（Kotlin `contentOrNull`：数字给字面量、布尔给 `true`/`false`）。
fn primitive_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn as_text(value: Option<&Value>) -> Option<String> {
    value.and_then(primitive_text)
}

fn as_double(value: Option<&Value>) -> Option<f64> {
    as_text(value)?.trim().parse::<f64>().ok()
}

/// Kotlin `asInt`：先按 double 解析再截断（`"20.0"` → 20）；越界**饱和**而不是回绕。
fn as_int(value: Option<&Value>) -> Option<i32> {
    let text = as_text(value)?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = trimmed.parse::<f64>() {
        return Some(parsed.trunc().clamp(i32::MIN as f64, i32::MAX as f64) as i32);
    }
    trimmed.parse::<i64>().ok().map(|v| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
}

/// Kotlin `asLong`：`toLongOrNull ?: toDoubleOrNull?.toLong() ?: toULongOrNull?.toLong()`。
///
/// ⚠ 第二档**真的会被走到**，而且它的行为是**饱和**不是回绕：`18446744073709551615` 先
/// `parse::<i64>` 失败（超出范围）→ 按 f64 解析成功（1.8446744073709552e19）→ `toLong()`
/// 饱和成 `i64::MAX`。第三档（uLong 按位回绕成 -1）**只在 f64 也解析不出来时才可能到达**，
/// 实际输入里几乎到不了 —— 别想当然地以为"大 seed 会变成负数"（第一版就是这么写错的，
/// 被 `huge_seed_wraps_like_kotlin` 当场抓住）。
fn as_long(value: Option<&Value>) -> Option<i64> {
    let text = as_text(value)?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = trimmed.parse::<i64>() {
        return Some(parsed);
    }
    if let Ok(parsed) = trimmed.parse::<f64>() {
        return Some(parsed.trunc().clamp(i64::MIN as f64, i64::MAX as f64) as i64);
    }
    trimmed.parse::<u64>().ok().map(|v| v as i64)
}

/// `["节点id", 槽位]` 里的节点 id（Kotlin `asRef`）。
///
/// ⚠ **节点 id 可能是数字**：真机油管里 `"negative": [76, 0]` 就是数字 76，而图的键是字符串
/// `"76"`。Kotlin 的 `asRef` 走 `contentOrNull`（数字给字面量 "76"）所以读得到；
/// 这里第一版只 `as_str()`，于是**负向提示词 / LoRA / checkpoint / 尺寸整条链静默断掉**
/// ——真库对账（`spot_check`）当场抓出来：25 条里 1 条的 `negative` 是空的。
fn reference_ref(value: Option<&Value>) -> Option<String> {
    let items = value?.as_array()?;
    let first = items.first()?;
    // 只认标量（字符串 / 数字）：数组套数组那种不是引用
    if first.is_array() || first.is_object() || first.is_null() {
        return None;
    }
    primitive_text(first)
}

fn input_text(node: &Map<String, Value>, key: &str) -> Option<String> {
    as_text(input(node, key))
}

fn input_double(node: &Map<String, Value>, key: &str) -> Option<f64> {
    as_double(input(node, key))
}

fn input_int(node: &Map<String, Value>, key: &str) -> Option<i32> {
    as_int(input(node, key))
}

fn input_long(node: &Map<String, Value>, key: &str) -> Option<i64> {
    as_long(input(node, key))
}

/// `inputs[key]` 里的节点 id（**数字 id 也认**，见 [`reference_ref`]）。
fn input_ref(node: &Map<String, Value>, key: &str) -> Option<String> {
    reference_ref(input(node, key))
}

/// 按 id 找节点（顺序无关，直接线性找）。
fn lookup<'a>(nodes: &[(&'a str, &'a Map<String, Value>)], id: &str) -> Option<&'a Map<String, Value>> {
    nodes.iter().find(|(node_id, _)| *node_id == id).map(|(_, node)| *node)
}

/// Kotlin `firstNonBlank(vararg)`：按顺序取第一个非空白（参数个数有意放宽）。
fn first_non_blank<const N: usize>(values: [Option<String>; N]) -> Option<String> {
    values.into_iter().flatten().find(|text| !text.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(text: &str) -> Map<String, Value> {
        serde_json::from_str::<Value>(text).unwrap().as_object().unwrap().clone()
    }

    /// 经典 KSampler 图：steps / cfg / seed / 正负提示词 / 尺寸全都在。
    #[test]
    fn classic_ksampler_graph() {
        let parsed = parse(Some(&graph(
            r#"{
              "3": {"class_type":"KSampler","inputs":{"seed":12345,"steps":20,"cfg":3.5,
                    "sampler_name":"euler","scheduler":"normal","denoise":1.0,
                    "model":["4",0],"positive":["6",0],"negative":["7",0],"latent_image":["5",0]}},
              "4": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"krea2.safetensors"}},
              "5": {"class_type":"EmptyLatentImage","inputs":{"width":1024,"height":1536,"batch_size":2}},
              "6": {"class_type":"CLIPTextEncode","inputs":{"text":"1girl, cyberpunk","clip":["4",1]}},
              "7": {"class_type":"CLIPTextEncode","inputs":{"text":"lowres, bad hands","clip":["4",1]}},
              "9": {"class_type":"SaveImage","inputs":{"filename_prefix":"krea2/t2i","images":["8",0]}}
            }"#,
        )));
        assert_eq!(parsed.positive, "1girl, cyberpunk");
        assert_eq!(parsed.negative.as_deref(), Some("lowres, bad hands"));
        assert_eq!(parsed.checkpoint.as_deref(), Some("krea2.safetensors"));
        assert_eq!(parsed.steps, Some(20));
        assert_eq!(parsed.cfg, Some(3.5));
        assert_eq!(parsed.seed, Some(12345));
        assert_eq!(parsed.sampler.as_deref(), Some("euler"));
        assert_eq!(parsed.scheduler.as_deref(), Some("normal"));
        assert_eq!((parsed.width, parsed.height, parsed.batch), (Some(1024), Some(1536), Some(2)));
        assert_eq!(parsed.kind, "IMAGE");
        // 标题线索取的是保存节点的文件名前缀（去掉目录）
        assert_eq!(parsed.title_hint.as_deref(), Some("t2i"));
        // extra 里有 denoise，但不该有 seed / steps（那些已经单独入库）
        assert!(parsed.extra.iter().any(|(k, _)| k == "KSampler.denoise"));
        assert!(!parsed.extra.iter().any(|(k, _)| k == "KSampler.seed"));
    }

    /// LoRA 链：顺着 `model` 往回走，权重取 `strength_model`；同名只留第一次。
    #[test]
    fn lora_chain_walks_model_links() {
        let parsed = parse(Some(&graph(
            r#"{
              "1": {"class_type":"KSampler","inputs":{"steps":8,"cfg":1,"seed":7,
                    "model":["2",0],"positive":["6",0],"latent_image":["5",0]}},
              "2": {"class_type":"LoraLoader","inputs":{"lora_name":"anima\\style\\anima_v2.safetensors",
                    "strength_model":0.8,"model":["3",0],"clip":["3",1]}},
              "3": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"krea2.safetensors"}},
              "5": {"class_type":"EmptyLatentImage","inputs":{"width":512,"height":512}},
              "6": {"class_type":"CLIPTextEncode","inputs":{"text":"ink painting"}}
            }"#,
        )));
        assert_eq!(parsed.loras.len(), 1);
        assert_eq!(parsed.loras[0].name, "anima_v2.safetensors", "只留文件名");
        assert_eq!(parsed.loras[0].weight, 0.8);
        assert_eq!(parsed.checkpoint.as_deref(), Some("krea2.safetensors"));
    }

    /// 视频 / 音频类型：`SaveVideo` → VIDEO；纯音频图 → AUDIO；两种都有 → MIXED。
    #[test]
    fn kind_detection_covers_video_audio_and_mixed() {
        let video = parse(Some(&graph(
            r#"{"1":{"class_type":"SaveVideo","inputs":{"filename_prefix":"v"}},
                "2":{"class_type":"KSampler","inputs":{"steps":4,"cfg":1,"seed":1}}}"#,
        )));
        assert_eq!(video.kind, "VIDEO");
        let audio = parse(Some(&graph(
            r#"{"1":{"class_type":"SaveAudioMP3","inputs":{}},
                "2":{"class_type":"SaveAudio","inputs":{}}}"#,
        )));
        assert_eq!(audio.kind, "AUDIO");
        let mixed = parse(Some(&graph(
            r#"{"1":{"class_type":"SaveVideo","inputs":{}},"2":{"class_type":"SaveAudio","inputs":{}}}"#,
        )));
        assert_eq!(mixed.kind, "MIXED");
    }

    /// 自定义采样链（SamplerCustom + Guider + Sigmas + 独立 seed 节点）。
    #[test]
    fn custom_sampler_chain_reads_sigmas_and_guider() {
        let parsed = parse(Some(&graph(
            r#"{
              "1": {"class_type":"SamplerCustomAdvanced","inputs":{"noise":["2",0],"guider":["3",0],
                    "sampler":["4",0],"sigmas":["5",0],"latent_image":["6",0]}},
              "2": {"class_type":"RandomNoise","inputs":{"noise_seed":999}},
              "3": {"class_type":"CFGGuider","inputs":{"cfg":2.5,"model":["7",0],"positive":["8",0],"negative":["9",0]}},
              "4": {"class_type":"KSamplerSelect","inputs":{"sampler_name":"dpmpp_2m"}},
              "5": {"class_type":"BasicScheduler","inputs":{"scheduler":"sgm_uniform","steps":6}},
              "6": {"class_type":"EmptySD3LatentImage","inputs":{"width":768,"height":1368}},
              "7": {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"shinku.safetensors"}},
              "8": {"class_type":"CLIPTextEncode","inputs":{"text":"maid cafe"}},
              "9": {"class_type":"CLIPTextEncode","inputs":{"text":"blurry"}}
            }"#,
        )));
        assert_eq!(parsed.seed, Some(999), "seed 从 RandomNoise 节点读（采样器自己没有）");
        assert_eq!(parsed.cfg, Some(2.5), "cfg 从 Guider 读");
        assert_eq!(parsed.steps, Some(6), "steps 从 Scheduler 读");
        assert_eq!(parsed.sampler.as_deref(), Some("dpmpp_2m"), "采样器名从 KSamplerSelect 读");
        assert_eq!(parsed.scheduler.as_deref(), Some("sgm_uniform"));
        assert_eq!(parsed.positive, "maid cafe");
        assert_eq!(parsed.negative.as_deref(), Some("blurry"));
        assert_eq!((parsed.width, parsed.height), (Some(768), Some(1368)));
    }

    /// 负向与正向相同时丢掉（Kotlin `takeIf { it != positive }`）。
    #[test]
    fn negative_is_dropped_when_it_equals_positive() {
        let parsed = parse(Some(&graph(
            r#"{
              "1": {"class_type":"KSampler","inputs":{"steps":4,"cfg":1,"seed":1,
                    "positive":["2",0],"negative":["3",0],"latent_image":["4",0]}},
              "2": {"class_type":"CLIPTextEncode","inputs":{"text":"same text"}},
              "3": {"class_type":"CLIPTextEncode","inputs":{"text":"same text"}},
              "4": {"class_type":"EmptyLatentImage","inputs":{"width":64,"height":64}}
            }"#,
        )));
        assert_eq!(parsed.positive, "same text");
        assert_eq!(parsed.negative, None);
    }

    /// **真机形状**（真库 `prompts.id=304` 那份 `capture_runs.raw` 的结构，逐节点抄下来）：
    /// Qwen/Anima 那条链 —— `KSampler` 的 `positive`/`negative` 都指向普通 `CLIPTextEncode`，
    /// 而 `model` 经过一个 **LoraLoader + SageAttention** 才到 `UNETLoader`。
    ///
    /// 这条用例是**真库对账抓出来的**：第一版在真库上跑 25 条对账，只有这一条的
    /// `negative` 是空的（其余 24 条逐字段一致）—— 于是把它固化成用例。
    #[test]
    fn anima_style_graph_from_the_real_library() {
        let parsed = parse(Some(&graph(
            r#"{
              "1":   {"class_type":"LoraLoader","inputs":{"clip":["72",0],"model":["111",0],
                       "lora_name":"allmm style-step00003000.safetensors","strength_clip":"1","strength_model":"1"}},
              "72":  {"class_type":"CLIPLoader","inputs":{"type":"qwen_image","device":"default","clip_name":"qwen_3_06b_base.safetensors"}},
              "73":  {"class_type":"VAELoader","inputs":{"vae_name":"qwen_image_vae.safetensors"}},
              "74":  {"class_type":"VAEDecode","inputs":{"vae":["73",0],"samples":["77",0]}},
              "75":  {"class_type":"EmptyLatentImage","inputs":{"width":"1024","height":"1024","batch_size":"1"}},
              "76":  {"class_type":"CLIPTextEncode","inputs":{"clip":["113",0],"text":"(worst quality, low quality, normal quality), bad anatomy"}},
              "77":  {"class_type":"KSampler","inputs":{"cfg":"4","seed":"935571105263085","model":["113",0],
                       "steps":"30","denoise":"1","negative":["76",0],"positive":["78",0],
                       "scheduler":"simple","latent_image":["75",0],"sampler_name":"er_sde"}},
              "78":  {"class_type":"CLIPTextEncode","inputs":{"clip":["113",0],"text":"Extreme close-up portrait, anime girl"}},
              "79":  {"class_type":"UNETLoader","inputs":{"unet_name":"anima-base-v1.0.safetensors","weight_dtype":"default"}},
              "107": {"class_type":"SaveImage","inputs":{"images":["74",0],"filename_prefix":"test"}},
              "111": {"class_type":"PathchSageAttentionKJ","inputs":{"model":["79",0],"allow_compile":"False","sage_attention":"auto"}},
              "113": {"class_type":"LoraLoader","inputs":{"clip":["1",0],"model":["1",0],
                       "lora_name":"anima-aesthetic-improvement-v1.1.safetensors","strength_clip":"1","strength_model":"0.8"}}
            }"#,
        )));
        assert_eq!(parsed.positive, "Extreme close-up portrait, anime girl");
        assert_eq!(
            parsed.negative.as_deref(),
            Some("(worst quality, low quality, normal quality), bad anatomy"),
            "负向提示词必须从 sampler.negative 指到的那个 CLIPTextEncode 读出来"
        );
        assert_eq!(parsed.steps, Some(30), "steps 是字符串 \"30\"，要能读");
        assert_eq!(parsed.cfg, Some(4.0));
        assert_eq!(parsed.seed, Some(935571105263085));
        assert_eq!(parsed.sampler.as_deref(), Some("er_sde"));
        assert_eq!(parsed.scheduler.as_deref(), Some("simple"));
        assert_eq!((parsed.width, parsed.height, parsed.batch), (Some(1024), Some(1024), Some(1)));
        // 顺着 model 链收集 LoRA（113 在 1 之前、1 又在 111 之前 —— 文档顺序决定先后）
        assert_eq!(
            parsed.loras.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            vec!["anima-aesthetic-improvement-v1.1.safetensors", "allmm style-step00003000.safetensors"]
        );
        assert_eq!(
            parsed.checkpoint.as_deref(),
            Some("anima-base-v1.0.safetensors"),
            "checkpoint 藏在 UNETLoader.unet_name（不是 ckpt_name）"
        );
    }

    /// **节点引用里的 id 可能是数字**（真机 `"negative": [76, 0]`，而图的键是字符串 `"76"`）。
    ///
    /// 这条是真库对账抓出来的：第一版只认字符串 id，于是负向提示词 / LoRA / checkpoint /
    /// 尺寸的**整条引用链静默断掉**（25 条真实记录里，prompt 304 的 negative 解析成空）。
    /// 用例同时钉住"数字 id"与"字符串 id"两种写法必须给出**一样**的结果。
    #[test]
    fn node_references_accept_numeric_ids() {
        let numeric = parse(Some(&graph(
            r#"{
              "76": {"class_type":"CLIPTextEncode","inputs":{"text":"bad hands"}},
              "77": {"class_type":"KSampler","inputs":{"steps":20,"cfg":3,"seed":5,
                     "negative":[76,0],"positive":[78,0],"model":[4,0],"latent_image":[5,0]}},
              "78": {"class_type":"CLIPTextEncode","inputs":{"text":"a cat"}},
              "4":  {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"m.safetensors"}},
              "5":  {"class_type":"EmptyLatentImage","inputs":{"width":512,"height":512}}
            }"#,
        )));
        assert_eq!(numeric.negative.as_deref(), Some("bad hands"), "数字 id 的 negative 引用");
        assert_eq!(numeric.positive, "a cat");
        assert_eq!(numeric.checkpoint.as_deref(), Some("m.safetensors"), "数字 id 的 model 引用");
        assert_eq!(numeric.width, Some(512), "数字 id 的 latent_image 引用");

        let strings = parse(Some(&graph(
            r#"{
              "76": {"class_type":"CLIPTextEncode","inputs":{"text":"bad hands"}},
              "77": {"class_type":"KSampler","inputs":{"steps":20,"cfg":3,"seed":5,
                     "negative":["76",0],"positive":["78",0],"model":["4",0],"latent_image":["5",0]}},
              "78": {"class_type":"CLIPTextEncode","inputs":{"text":"a cat"}},
              "4":  {"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"m.safetensors"}},
              "5":  {"class_type":"EmptyLatentImage","inputs":{"width":512,"height":512}}
            }"#,
        )));
        assert_eq!(numeric, strings, "两种 id 写法必须解析成同一个结果");
    }

    /// 空图 / null / 脏数据都不许 panic，也不许编字段。
    #[test]
    fn empty_and_dirty_graphs_are_safe() {
        assert_eq!(parse(None), Parsed::default());
        assert_eq!(parse(Some(&Map::new())), Parsed::default());
        // 值不是对象（脏数据）：整条丢掉
        let dirty = graph(r#"{"1":"nope","2":{"class_type":"KSampler","inputs":{"steps":1,"cfg":1}}}"#);
        let parsed = parse(Some(&dirty));
        assert_eq!(parsed.steps, Some(1));
        // 类型全错的 inputs：不许 panic，字段留空
        let weird = parse(Some(&graph(
            r#"{"1":{"class_type":"KSampler","inputs":{"steps":"abc","cfg":null,"seed":{}}}}"#,
        )));
        assert_eq!(weird.steps, None);
        assert_eq!(weird.cfg, None);
        assert_eq!(weird.seed, None);
    }

    /// 文本兜底：没有条件链时取最长的文本输入；**并列取第一个**（Kotlin `maxByOrNull`）。
    #[test]
    fn longest_text_fallback_takes_the_first_on_ties() {
        let parsed = parse(Some(&graph(
            r#"{
              "1": {"class_type":"KSampler","inputs":{"steps":4,"cfg":1,"seed":1}},
              "2": {"class_type":"CLIPTextEncode","inputs":{"text":"AAAA"}},
              "3": {"class_type":"CLIPTextEncode","inputs":{"text":"BBBB"}}
            }"#,
        )));
        assert_eq!(parsed.positive, "AAAA", "并列时必须取文档顺序里的第一个");

        let longer = parse(Some(&graph(
            r#"{
              "1": {"class_type":"KSampler","inputs":{"steps":4,"cfg":1,"seed":1}},
              "2": {"class_type":"CLIPTextEncode","inputs":{"text":"AAAA"}},
              "3": {"class_type":"CLIPTextEncode","inputs":{"text":"BBBBBBBB"}}
            }"#,
        )));
        assert_eq!(longer.positive, "BBBBBBBB", "更长的那条才算正向");
    }

    /// 大 seed（> i64 范围）**饱和**成 `i64::MAX`（Kotlin 的第二档 `toDoubleOrNull()?.toLong()`）。
    #[test]
    fn huge_seed_saturates_like_kotlin() {
        let parsed = parse(Some(&graph(
            r#"{"1":{"class_type":"KSampler","inputs":{"steps":1,"cfg":1,"seed":18446744073709551615}}}"#,
        )));
        assert_eq!(parsed.seed, Some(i64::MAX));
        // 还在 i64 范围内的大数按 i64 直读（不能先过 f64：那会丢精度）
        let normal = parse(Some(&graph(
            r#"{"1":{"class_type":"KSampler","inputs":{"steps":1,"cfg":1,"seed":9007199254740993}}}"#,
        )));
        assert_eq!(normal.seed, Some(9007199254740993));
    }

    /// `extra` 的上限与 Kotlin **一模一样**：`>= 60` 就停，但那条 `__node_id` 不经过检查，
    /// 所以实际长度**最多 61**（60 来自 `__node_id`、下一条 push 才触发 break）。
    /// 别"顺手修正"成严格 60 —— 那会与 Kotlin 差一条，而这一条会进 `extra_params`。
    #[test]
    fn extra_cap_matches_kotlin_exactly() {
        let mut nodes = String::from("{");
        for index in 0..40 {
            if index > 0 {
                nodes.push(',');
            }
            nodes.push_str(&format!(
                r#""{index}":{{"class_type":"Node{index}","inputs":{{"a":"1","b":"2","c":"3"}}}}"#
            ));
        }
        nodes.push('}');
        let parsed = parse(Some(&graph(&nodes)));
        assert_eq!(parsed.extra.len(), 61, "Kotlin 的 break 在 push 之后 → 最多 61");
        assert_eq!(parsed.extra[0].0, "Node0.a");
        // 少量节点时不会有上限，且每个节点末尾都带一条 `__node_id`
        let small = parse(Some(&graph(
            r#"{"1":{"class_type":"KSampler","inputs":{"steps":1,"cfg":1,"denoise":0.5}}}"#,
        )));
        assert!(small.extra.iter().any(|(k, v)| k == "KSampler.__node_id" && v == "1"));
        assert!(small.extra.iter().any(|(k, v)| k == "KSampler.denoise" && v == "0.5"));
    }
}
