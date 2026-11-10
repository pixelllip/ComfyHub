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

use std::collections::{HashSet, VecDeque};

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
const AUDIO_CLASS_HINTS: [&str; 5] = [
    "saveaudio",
    "previewaudio",
    "audioupload",
    "saveaudiomp3",
    "voiceclone",
];

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

/// 顺着**连线**继续找正文时认的键。
///
/// ⚠ **不能含 `system_prompt` / `instruction`**：那是 `TextGenerate` 的扩写指令
/// （真机 prompt 549 / 597 的编码器 `text` 直接连到 `TextGenerate`），把它当正向
/// 比留空更差 —— 画廊中会显示一整段"你是一个提示词工程师……"而不是目标画面。
const LINKED_TEXT_KEYS: [&str; 8] = [
    "text",
    "prompt",
    "positive",
    "positive_prompt",
    "text_g",
    "text_l",
    "caption",
    "prompt_text",
];

/// 继续下钻时认这些"条件 / 文本"转发键。
const CHAIN_KEYS: [&str; 6] = [
    "conditioning",
    "cond",
    "positive",
    "negative",
    "clip",
    "prompt",
];

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
    let Some(graph) = graph else {
        return Parsed::default();
    };
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
    let sigmas = sampler
        .and_then(|node| input_ref(node, "sigmas"))
        .and_then(|id| lookup(&nodes, &id));
    let sampler_select = sampler
        .and_then(|node| input_ref(node, "sampler"))
        .and_then(|id| lookup(&nodes, &id));
    let noise = sampler
        .and_then(|node| input_ref(node, "noise"))
        .and_then(|id| lookup(&nodes, &id));

    let steps_node = [sampler, sigmas, guider]
        .into_iter()
        .flatten()
        .find(|node| input_int(node, "steps").is_some())
        .or_else(|| {
            nodes.iter().map(|(_, node)| *node).find(|node| {
                cls(node).to_lowercase().contains("scheduler") && input_int(node, "steps").is_some()
            })
        })
        .or_else(|| {
            nodes
                .iter()
                .map(|(_, node)| *node)
                .find(|node| input_int(node, "steps").is_some())
        });

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
        negative: negative.filter(|text| !text.trim().is_empty() && *text != positive_text),
        checkpoint: checkpoint.or_else(|| find_checkpoint(&nodes)),
        loras,
        sampler: sampler_name,
        scheduler: scheduler_name,
        steps: steps_node
            .and_then(|node| input_int(node, "steps"))
            .map(|v| v as i64),
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
//  运行时最终提示词（2026-10-25）
// ---------------------------------------------------------------------------

/// 从 `/history` 的 `outputs` 里取**这次运行真正喂给正向条件的文本**。
///
/// # 为什么节点图不够
///
/// Krea2 r2i 那类工作流的正向提示词是**运行时生成**的：
/// `TextGenerate`（本地看图 / 扩写模型）→ `ComfySwitchNode` → `PreviewAny` → `CLIPTextEncode.text`。
/// 节点图里只有 `TextGenerate.system_prompt` 和两个**空**的输入槽，最终那一整段话
/// 只出现在 `/history` 的 `outputs` 里（`PreviewAny` / `ShowText` 的 `text` 数组）。
/// 只看图的话，画廊详情中的提示词会一直为空（`test_00901_.png` 即如此）。
///
/// # 判据（只认"它确实喂给了正向条件"）
///
/// 1. 从采样器的 `positive`（以及 guider 的 `conditioning`）顺着条件链找到文本编码器；
/// 2. 编码器的 `text` 是**连线**时才查运行时输出（字面量由 [`parse`] 直接拿到，
///    不能被旁边某个预览节点的文本顶掉）；
/// 3. 只认这条连线指向的节点（含 PreviewAny / ShowText / 开关这类纯转发节点）在
///    `outputs[id].text` 里的值；
/// 4. 找不到就 `None` —— **绝不拿别的文本输出顶替**（那可能是备注或负向），
///    调用方保持 [`parse`] 的原结果。
pub fn final_positive(
    graph: Option<&Map<String, Value>>,
    outputs: Option<&Map<String, Value>>,
) -> Option<String> {
    let graph = graph?;
    let outputs = outputs?;
    if graph.is_empty() {
        return None;
    }
    let nodes: Vec<(&str, &Map<String, Value>)> = graph
        .iter()
        .filter_map(|(id, value)| value.as_object().map(|obj| (id.as_str(), obj)))
        .collect();
    if nodes.is_empty() {
        return None;
    }

    let sampler = find_sampler(&nodes);
    // guider 的取法与 [`parse`] 完全一致（Krea 2 的 `BasicGuider` 只有 `conditioning`）
    let guider = sampler
        .and_then(|node| input_ref(node, "guider"))
        .and_then(|id| lookup(&nodes, &id))
        .or_else(|| {
            sampler
                .and_then(|node| input_ref(node, "model"))
                .and_then(|id| lookup(&nodes, &id))
                .filter(|node| cls(node).contains("Guider"))
        });

    let mut starts: Vec<String> = Vec::new();
    if let Some(node) = sampler {
        starts.extend(input_ref(node, "positive"));
        starts.extend(input_ref(node, "positive_cond"));
    }
    if let Some(node) = guider {
        starts.extend(input_ref(node, "conditioning"));
        starts.extend(input_ref(node, "positive"));
    }

    let mut queue: VecDeque<(String, usize)> = starts.into_iter().map(|id| (id, 0)).collect();
    let mut seen: HashSet<String> = HashSet::new();
    while let Some((id, depth)) = queue.pop_front() {
        if depth > 8 || id.trim().is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        let Some(node) = lookup(&nodes, &id) else {
            continue;
        };
        if cls(node).to_ascii_lowercase().contains("textencode") {
            let Some(text) = input(node, "text") else {
                continue;
            };
            // 字面量（parse 已经拿得到）不在这里处理；只有连线才需要看运行时输出。
            let Some(upstream) = reference_ref(Some(text)) else {
                continue;
            };
            if let Some(found) = runtime_text_in_chain(&upstream, &nodes, outputs, 0) {
                let trimmed = found.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
            continue;
        }
        let Some(map) = inputs(node) else { continue };
        for (key, value) in map {
            if !positive_chain_key(key) {
                continue;
            }
            if let Some(up) = reference_ref(Some(value)) {
                queue.push_back((up, depth + 1));
            }
        }
    }
    None
}

/// 条件链上还要继续跟着走的输入（与 `workflow_submit::apply_positive_prompt` 同一档）。
fn positive_chain_key(key: &str) -> bool {
    key == "positive"
        || key == "conditioning"
        || key.starts_with("conditioning_")
        || key == "on_true"
        || key == "on_false"
}

/// 文本转发节点上可以继续往下找的输入：`PreviewAny.source`、开关的 `on_true/on_false`、
/// `StringConcatenate.string_a/b`、`PrimitiveString*.value` 都在这一档。
fn runtime_text_chain_key(key: &str) -> bool {
    matches!(
        key,
        "source"
            | "text"
            | "value"
            | "string"
            | "string_a"
            | "string_b"
            | "prompt"
            | "on_true"
            | "on_false"
    )
}

/// 某个节点这次运行输出的文本（`PreviewAny` / `ShowText` 一类）。
///
/// ComfyUI 的形状是 `outputs[id].text = ["…", …]`（字符串数组）；个别实现给字符串 —— 两种都认。
fn runtime_text_of(outputs: &Map<String, Value>, id: &str) -> Option<String> {
    let node = outputs.get(id)?.as_object()?;
    match node.get("text")? {
        Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            (!joined.is_empty()).then_some(joined)
        }
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        _ => None,
    }
}

/// 从编码器 `text` 指向的节点往回找它的运行时文本；中间允许穿过纯转发节点。
///
/// 深度上限与 [`text_from_ref`] 同一档（6）：真实链很少超过三层，
/// 而"图里绕圈子"的脏数据不能把捕获轮询卡住。
fn runtime_text_in_chain(
    id: &str,
    nodes: &[(&str, &Map<String, Value>)],
    outputs: &Map<String, Value>,
    depth: usize,
) -> Option<String> {
    if depth > 6 {
        return None;
    }
    if let Some(text) = runtime_text_of(outputs, id) {
        return Some(text);
    }
    let node = lookup(nodes, id)?;
    let map = inputs(node)?;
    for (key, value) in map {
        if !runtime_text_chain_key(key) {
            continue;
        }
        let Some(up) = reference_ref(Some(value)) else {
            continue;
        };
        if let Some(text) = runtime_text_in_chain(&up, nodes, outputs, depth + 1) {
            return Some(text);
        }
    }
    None
}

// ---------------------------------------------------------------------------
//  采样器定位
// ---------------------------------------------------------------------------

/// 找采样器（Kotlin `findSampler`：五种判据**顺序不能换**）。
fn find_sampler<'a>(nodes: &[(&'a str, &'a Map<String, Value>)]) -> Option<&'a Map<String, Value>> {
    let all: Vec<&Map<String, Value>> = nodes.iter().map(|(_, node)| *node).collect();
    // 经典 KSampler：steps + cfg 都在自己身上
    if let Some(node) = all
        .iter()
        .find(|n| input_int(n, "steps").is_some() && input_double(n, "cfg").is_some())
    {
        return Some(node);
    }
    // 自定义采样链的根节点：有 sigmas 且（有 noise 或 guider）
    if let Some(node) = all.iter().find(|n| {
        input(n, "sigmas").is_some()
            && (input(n, "noise").is_some() || input(n, "guider").is_some())
    }) {
        return Some(node);
    }
    if let Some(node) = all
        .iter()
        .find(|n| cls(n).to_lowercase().contains("samplercustom"))
    {
        return Some(node);
    }
    // 注意 KSamplerSelect 也含 "KSampler"，但它只有 sampler_name，别优先选它
    if let Some(node) = all
        .iter()
        .find(|n| cls(n).to_lowercase().contains("ksampler") && input(n, "sampler_name").is_none())
    {
        return Some(node);
    }
    if let Some(node) = all
        .iter()
        .find(|n| cls(n).to_lowercase().contains("ksampler"))
    {
        return Some(node);
    }
    all.iter()
        .find(|n| cls(n).to_lowercase().contains("sampler"))
        .copied()
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
            let Some(node) = lookup(nodes, &current) else {
                break;
            };

            if looks_like_lora_node(node) {
                // 从当前节点往**上游**收：本节点的槽位 + 它引用的 LoRA 栈（WAS 那类把名字
                // 放在 `lora_stack` 指向的另一个节点里）。`visited` 防环，深度上限与文本链一致。
                let mut visited: HashSet<String> = HashSet::new();
                visited.insert(current.clone());
                collect_loras(node, nodes, &mut loras, &mut visited, 0);
            }

            if checkpoint.is_none() {
                for key in [
                    "ckpt_name",
                    "model_name",
                    "unet_name",
                    "gguf_name",
                    "diffusion_model",
                ] {
                    if let Some(text) = input_text(node, key).filter(|text| !text.trim().is_empty())
                    {
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

/// 这个节点像不像"加载 LoRA"的节点。
///
/// 两条判据任一命中即可（**不要**维护第三方类名白名单 —— 安装的节点包无法预知）：
///  ① 类名里带 `lora`：官方 `LoraLoader` / `LoraLoaderModelOnly`、rgthree 的
///     `Lora Loader Stack` / `Power Lora Loader`、Krea2 的 `Krea2ControlLoRALoader`……
///  ② 有以 `lora` 开头的**文件名槽位**：有些自定义节点类名看不出用途，但槽位名是
///     `lora_name` / `lora_01` 这类。
fn looks_like_lora_node(node: &Map<String, Value>) -> bool {
    if cls(node).to_lowercase().contains("lora") {
        return true;
    }
    inputs(node)
        .map(|map| {
            map.iter().any(|(key, value)| {
                // 文件名槽位，或 `lora_stack` 这种指向上游栈节点的连线
                lora_slot_key(key) || (key.to_lowercase().starts_with("lora") && is_link(value))
            })
        })
        .unwrap_or(false)
}

/// 输入键是不是"LoRA 文件名槽位"。
///
/// 认的是**槽位命名**，不是某几个固定键：
/// * 官方：`lora_name` / `lora` / `lora_path`
/// * rgthree `Lora Loader Stack`：`lora_01` … `lora_04`（权重在旁边 `strength_01`）
/// * Comfyroll / WAS 的栈：`lora_name_1`（权重是 `model_weight_1`）
///
/// 同一批节点还会带权重 / 触发词 / 栈引用这类**同前缀但不是文件名**的键，一律排除。
fn lora_slot_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    if !lower.starts_with("lora") {
        return false;
    }
    if ["strength", "weight", "wt", "scale", "trigger", "stack"]
        .iter()
        .any(|hint| lower.contains(hint))
    {
        return false;
    }
    // 开关类键（`lora_on`）不是文件名。只按**整键 / 后缀**判，别用 `contains`：
    // 那会误伤 `lora_version` 这类正常槽位。
    !(lower == "lora_on"
        || lower == "lora_enable"
        || lower == "lora_enabled"
        || lower.ends_with("_on")
        || lower.ends_with("_enable")
        || lower.ends_with("_enabled"))
}

/// 槽位对应的权重。
///
/// 先按**编号后缀**找（`lora_01` → `strength_01`、`lora_name_1` → `model_weight_1`），
/// 找不到再退回老的三档（`strength_model` / `strength` / `weight`）。
/// 一个值都没有时按 1.0 —— 老行为，别改成 0（0 权重等于"没启用"，语义完全不同）。
fn lora_weight(node: &Map<String, Value>, key: &str) -> f64 {
    let lower = key.to_lowercase();
    let index = lower
        .rsplit_once('_')
        .filter(|(_, tail)| !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()))
        .map(|(_, tail)| tail.to_string());
    let mut candidates: Vec<String> = Vec::new();
    if let Some(index) = index {
        // `_01` 与 `01` 两种拼法都试（不同节点包的写法不一样）
        for prefix in [
            "strength",
            "weight",
            "model_weight",
            "strength_model",
            "strength_clip",
            "lora_strength",
            "lora_weight",
        ] {
            candidates.push(format!("{prefix}_{index}"));
            candidates.push(format!("{prefix}{index}"));
        }
    }
    for prefix in [
        "strength_model",
        "strength",
        "weight",
        "model_weight",
        "lora_strength",
    ] {
        candidates.push(prefix.to_string());
    }
    candidates
        .into_iter()
        .find_map(|candidate| input_double(node, &candidate))
        .unwrap_or(1.0)
}

/// 从一个 LoRA 槽位节点里收 `(名字, 权重)`。**只看槽位本身，不递归**。
///
/// 三种值形状都要认：
///  * 字符串：`"lora_name": "a.safetensors"`
///  * 字符串数组：少数节点把多个名字写在一个键里
///  * 对象：rgthree 的 `Power Lora Loader` 写成
///    `"lora_1": {"on": true, "lora": "Krea2\\x.safetensors", "strength": 1}`
///
/// ⚠ `["节点id", 槽位]` 是**连线**，不是文件名 —— 必须挡掉，否则会把节点 id 当成 LoRA 名。
fn collect_lora_entries(node: &Map<String, Value>) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    let Some(inputs) = inputs(node) else {
        return out;
    };
    for (key, value) in inputs {
        if !lora_slot_key(key) {
            continue;
        }
        match value {
            Value::String(name) => out.push((name.clone(), lora_weight(node, key))),
            Value::Array(items) => {
                if is_link(value) {
                    continue;
                }
                for item in items {
                    match item {
                        Value::String(name) => out.push((name.clone(), lora_weight(node, key))),
                        Value::Object(object) => {
                            if let Some(entry) = nested_lora_entry(object) {
                                out.push(entry);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Value::Object(object) => {
                if let Some(entry) = nested_lora_entry(object) {
                    out.push(entry);
                }
            }
            _ => {}
        }
    }
    out
}

/// rgthree `Power Lora Loader` 那种"一个槽位一个对象"的写法。
///
/// `on: false` 的槽位**跳过**：它在 ComfyUI 里没有生效，记进提示词等于撒谎。
fn nested_lora_entry(object: &Map<String, Value>) -> Option<(String, f64)> {
    let name = object
        .get("lora")
        .and_then(Value::as_str)
        .or_else(|| object.get("name").and_then(Value::as_str))?;
    if name.trim().is_empty() {
        return None;
    }
    if let Some(flag) = object.get("on") {
        let enabled = match flag {
            Value::Bool(value) => *value,
            Value::String(text) => {
                !text.trim().is_empty()
                    && !text.eq_ignore_ascii_case("false")
                    && !text.eq_ignore_ascii_case("off")
            }
            Value::Number(number) => number.as_f64().map(|value| value != 0.0).unwrap_or(true),
            _ => true,
        };
        if !enabled {
            return None;
        }
    }
    let weight = as_double(object.get("strength"))
        .or_else(|| as_double(object.get("weight")))
        .unwrap_or(1.0);
    Some((name.to_string(), weight))
}

/// 收 LoRA：本节点 + 它用 `lora*` 连线引用的上游（栈节点）。
///
/// WAS 那类把名字放在 `lora_stack` 指向的另一个节点里，加载器自己只有一条连线；
/// 只走 `model` 链会漏掉整座栈，所以要顺着 LoRA 键往下看一眼。
fn collect_loras(
    node: &Map<String, Value>,
    nodes: &[(&str, &Map<String, Value>)],
    loras: &mut Vec<LoraRef>,
    visited: &mut HashSet<String>,
    depth: usize,
) {
    if depth > 6 {
        return;
    }
    for (name, weight) in collect_lora_entries(node) {
        push_lora(loras, &name, weight);
    }
    let Some(inputs) = inputs(node) else { return };
    for (key, value) in inputs {
        if !key.to_lowercase().starts_with("lora") || !is_link(value) {
            continue;
        }
        let Some(id) = reference_ref(Some(value)) else {
            continue;
        };
        if !visited.insert(id.clone()) {
            continue;
        }
        if let Some(next) = lookup(nodes, &id) {
            collect_loras(next, nodes, loras, visited, depth + 1);
        }
    }
}

/// 入库一条 LoRA：只留文件名、同名只留第一次（顺序即语义）。
fn push_lora(loras: &mut Vec<LoraRef>, raw: &str, weight: f64) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return;
    }
    // 两种分隔符都要认：ComfyUI 的 lora 名可能是 `a\b\lora.safetensors`
    let short = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed).trim();
    if short.is_empty() {
        return;
    }
    // rgthree 的 `Lora Loader Stack` 用字符串 `"None"` 占位未启用的槽位（真机捕获里
    // `"lora_04": "None"` 很常见）—— 那是空槽，不是名叫 None 的 LoRA。
    if matches!(
        short.to_lowercase().as_str(),
        "none" | "null" | "nil" | "undefined"
    ) {
        return;
    }
    if !loras.iter().any(|existing| existing.name == short) {
        loras.push(LoraRef {
            name: short.to_string(),
            weight,
        });
    }
}

/// `["节点id", 槽位]` 形状的连线（第二项是数字才算；名字数组没有数字第二项）。
fn is_link(value: &Value) -> bool {
    let Some(items) = value.as_array() else {
        return false;
    };
    items.len() >= 2 && items[1].is_number() && (items[0].is_string() || items[0].is_number())
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
    for key in [
        "ckpt_name",
        "model_name",
        "unet_name",
        "gguf_name",
        "diffusion_model",
    ] {
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
    // 正文经常不在编码器自己身上：`CLIPTextEncode.text` 被改成**连线**，真正的长文本
    // 在上游的 `PrimitiveStringMultiline.value` 里（真机 `test_01357_.png`）。上面只取
    // 字面量，取不到时必须顺着这些文本键继续往下走，否则整段正向会被静默当成空。
    for key in LINKED_TEXT_KEYS {
        if let Some(text) = text_from_ref(input(node, key), nodes, depth + 1) {
            if !text.trim().is_empty() {
                return Some(text);
            }
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
fn longest_text_input(nodes: &[(&str, &Map<String, Value>)]) -> Option<(String, String)> {
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
fn negative_text_input(nodes: &[(&str, &Map<String, Value>)]) -> Option<(String, String)> {
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
        if VIDEO_CLASS_HINTS
            .iter()
            .any(|hint| !hint.is_empty() && class.contains(hint))
        {
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
        let class = cls(node)
            .split('(')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        for (key, value) in input_pairs(node) {
            if SKIP_INPUT_KEYS.contains(&key.as_str()) {
                continue;
            }
            let Some(text) = primitive_text(&value) else {
                continue;
            };
            if text.trim().is_empty() || text.chars().count() > 400 {
                continue;
            }
            out.push((format!("{class}.{key}"), text));
            if out.len() >= 60 {
                break 'outer;
            }
        }
        if out.len() < 60
            && !class.is_empty()
            && !out.iter().any(|(k, _)| k == &format!("{class}.__node_id"))
        {
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
    node.get("class_type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
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
    trimmed
        .parse::<i64>()
        .ok()
        .map(|v| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
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
fn lookup<'a>(
    nodes: &[(&'a str, &'a Map<String, Value>)],
    id: &str,
) -> Option<&'a Map<String, Value>> {
    nodes
        .iter()
        .find(|(node_id, _)| *node_id == id)
        .map(|(_, node)| *node)
}

/// Kotlin `firstNonBlank(vararg)`：按顺序取第一个非空白（参数个数有意放宽）。
fn first_non_blank<const N: usize>(values: [Option<String>; N]) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .find(|text| !text.trim().is_empty())
}

#[cfg(test)]
mod tests;
