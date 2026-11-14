//! 一次捕获运行的**实际参数**摘要：把 `capture_runs.raw` 里那份 API 图读成
//! "提示词 / 采样 / 尺寸 / 模型 / 其它开关"几组可以直接拿去 `comfy_submit(overrides=…)` 的键值。
//!
//! # 为什么要有这一层（2026-11-14 用户报的原话）
//!
//! 「AI 非常固执地只认用软件内最近提交的工作流的参数，完全不理 ComfyUI 里最新跑通的工作流参数。」
//!
//! 现场是这样的：用户在 ComfyUI 界面里自己跑（`capture_runs.submitted_by='ComfyUI'`）时，
//! 那份工作流**记着自己的参数** —— LoRA 强度、`ComfyUI-Krea2T-Enhancer` 的开关、尺寸都是，
//! 跟应用上一次 `comfy_submit` 用 `overrides` 改出来的不是一套。而 AI 手里能拿到的只有：
//!
//! * `comfy_recent_successes` —— 标题 / 模型文件名 / 时间；
//! * `comfy_get_run` —— 状态 / 产物数 / 提示词 id；
//! * `comfy_view_outputs` —— 提示词开头。
//!
//! **三件都没有参数**，于是它只能拿"自己上次提交的那份 overrides"当现状。
//!
//! # 判据
//!
//! 参数归类**不另写一套**：直接用 [`crate::workflow::workflow_recipe::analyze`] 的旋钮
//! （提示词 / 采样 / 种子 / 尺寸 / 加载器 / 其它）与模型槽位。这样与配方、蓝图看到的是同一份事实，
//! 不会出现"配方说 0.8、这里说 0.45"。
//!
//! **这条摘要只读真实发生的那一跑**（`raw` 里存的是当时真正提交给 ComfyUI `/prompt` 的图），
//! 不是"应该怎么跑"的建议 —— 参数该改成什么仍由 AI 结合用户的话决定。

use serde_json::{json, Map, Value};

use crate::workflow::workflow_recipe::{analyze, KnobKind};

/// 提示词开头留多少字（够 AI 认出"是不是同一版"，全文用 `search_prompts` / `comfy_view_outputs`）。
pub const PROMPT_HEAD_CHARS: usize = 120;
/// 单个参数值最多留多少字（模型文件名一般 20~60 字，开关只有 true/false）。
pub const VALUE_CHARS: usize = 80;
/// 每组最多列几条：再多就只进 JSON（正文只做"一眼看出差在哪"）。
const MAX_PROMPTS: usize = 3;
const MAX_SAMPLING: usize = 12;
const MAX_IMAGES: usize = 4;
const MAX_OTHERS: usize = 18;
const MAX_MODELS: usize = 12;

/// 一条提示词旋钮（节点 id.输入名 + 字数 + 开头）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptField {
    pub key: String,
    /// 原始字数（不是截断后的）
    pub chars: usize,
    pub head: String,
}

/// 一次运行的实际参数（空组就是"这张图里没有这一类"）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunParams {
    /// 正面 / 负面提示词（按字数从多到少；第一条通常就是正面）
    pub prompts: Vec<PromptField>,
    /// 采样 / 种子 / 尺寸：`33.steps=8` 这种
    pub sampling: Vec<(String, String)>,
    /// 输入图 / 遮罩文件名：`12.image=x.png`
    pub images: Vec<(String, String)>,
    /// 加载器开关 / LoRA 强度 / 其它字面量
    pub others: Vec<(String, String)>,
    /// 模型槽位：覆盖键 → 文件名
    pub models: Vec<(String, String)>,
    /// 任一组被组内上限截断（JSON 里也是截断后的）。
    pub truncated: bool,
}

/// 从 API 节点图归纳出参数摘要。
pub fn summarize(graph: &Map<String, Value>) -> RunParams {
    from_recipe(&analyze(graph, None))
}

/// 已经 `analyze` 过的调用方走这一条（`comfy_recent_successes` 本来就要那份 Recipe，
/// 不必为了参数再跑一遍分类）。
pub fn from_recipe(recipe: &crate::workflow::workflow_recipe::Recipe) -> RunParams {
    let mut params = RunParams::default();

    for knob in &recipe.knobs {
        let key = knob.override_key();
        match knob.kind {
            KnobKind::Prompt => params.prompts.push(PromptField {
                key,
                chars: knob.value.chars().count(),
                head: clip(&knob.value, PROMPT_HEAD_CHARS),
            }),
            KnobKind::Sampling | KnobKind::Seed | KnobKind::Size => {
                params.sampling.push((key, clip(&knob.value, VALUE_CHARS)))
            }
            KnobKind::Image => params.images.push((key, clip(&knob.value, VALUE_CHARS))),
            KnobKind::Loader | KnobKind::Other => {
                params.others.push((key, clip(&knob.value, VALUE_CHARS)))
            }
        }
    }
    // 最长的那条排前面：正面提示词通常比负面长得多，AI 要认的是它。
    // 同字数保持图里的顺序（`sort_by_key` 是稳定排序）。
    params.prompts.sort_by_key(|p| std::cmp::Reverse(p.chars));

    for slot in &recipe.slots {
        params
            .models
            .push((slot.override_key(), slot.file_name.clone()));
    }

    params.truncated = params.prompts.len() > MAX_PROMPTS
        || params.sampling.len() > MAX_SAMPLING
        || params.images.len() > MAX_IMAGES
        || params.others.len() > MAX_OTHERS
        || params.models.len() > MAX_MODELS;
    params.prompts.truncate(MAX_PROMPTS);
    params.sampling.truncate(MAX_SAMPLING);
    params.images.truncate(MAX_IMAGES);
    params.others.truncate(MAX_OTHERS);
    params.models.truncate(MAX_MODELS);
    params
}

/// 从 `capture_runs.raw`（裸 API 图或整条 `/history` 条目）归纳参数。
///
/// 两条存法都由 [`crate::workflow::auto_select::graph_of_raw`] 认；解析不出来时返回 `None`
/// —— 调用方必须如实说"这条没有完整 API 图，读不出参数"，不许拿别的运行顶上。
pub fn from_raw(raw: &str) -> Option<RunParams> {
    crate::workflow::auto_select::graph_of_raw(raw).map(|graph| summarize(&graph))
}

impl RunParams {
    /// 什么都没有（空图 / 认不出）：调用方据此说"读不出参数"。
    pub fn is_empty(&self) -> bool {
        self.prompts.is_empty()
            && self.sampling.is_empty()
            && self.images.is_empty()
            && self.others.is_empty()
            && self.models.is_empty()
    }

    /// 给模型看的正文（缩进由调用方定：`comfy_get_run` 顶格、列表项缩两格）。
    ///
    /// 每一行都以 "键=值" 的形式给出，键就是 `comfy_submit(overrides=…)` 能用的那种。
    pub fn render(&self, indent: &str) -> String {
        let mut out = String::new();
        if self.is_empty() {
            out.push_str(indent);
            out.push_str("（这张图里读不出参数：空图，或者 raw 解析失败）\n");
            return out;
        }
        if !self.prompts.is_empty() {
            out.push_str(indent);
            out.push_str("- 提示词：");
            let list: Vec<String> = self
                .prompts
                .iter()
                .map(|p| format!("{}（{} 字）「{}」", p.key, p.chars, p.head))
                .collect();
            out.push_str(&list.join(" · "));
            if self.prompts.len() > 1 {
                out.push_str("（按字数从多到少；第一条通常是正面）");
            }
            out.push('\n');
        }
        for (label, pairs) in [
            ("模型", &self.models),
            ("采样/尺寸", &self.sampling),
            ("输入图", &self.images),
            ("其它开关", &self.others),
        ] {
            if pairs.is_empty() {
                continue;
            }
            out.push_str(indent);
            out.push_str("- ");
            out.push_str(label);
            out.push('：');
            out.push_str(&render_pairs(pairs));
            out.push('\n');
        }
        if self.truncated {
            out.push_str(indent);
            out.push_str("（这里只列了一部分；要全量就看返回 JSON 的 params）\n");
        }
        out
    }

    /// JSON 形状：每组都是 `键 → 值`，键可以直接抄进 `overrides`。
    pub fn to_json(&self) -> Value {
        let map = |pairs: &[(String, String)]| -> Value {
            let mut out = Map::new();
            for (key, value) in pairs {
                out.insert(key.clone(), json!(value));
            }
            Value::Object(out)
        };
        json!({
            "prompts": self
                .prompts
                .iter()
                .map(|p| json!({"key": p.key, "chars": p.chars, "head": p.head}))
                .collect::<Vec<_>>(),
            "models": map(&self.models),
            "sampling": map(&self.sampling),
            "images": map(&self.images),
            "others": map(&self.others),
            "truncated": self.truncated,
        })
    }
}

fn render_pairs(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 从 [`RunParams::to_json`] 那份 JSON 重新渲染成正文。
///
/// 给"条目已经是一个 JSON 对象"的调用方用（`comfy_recent_successes` 把参数塞进每条 item
/// 一起返回，正文与 JSON 必须说同一件事 —— 各写一份格式化迟早对不上）。
pub fn render_from_json(params: &Value, indent: &str) -> String {
    let Some(obj) = params.as_object() else {
        return String::new();
    };
    let mut out = String::new();
    if let Some(list) = obj.get("prompts").and_then(Value::as_array) {
        let rendered: Vec<String> = list
            .iter()
            .filter_map(|p| {
                let key = p.get("key")?.as_str()?;
                let chars = p.get("chars").and_then(Value::as_i64).unwrap_or(0);
                let head = p.get("head").and_then(Value::as_str).unwrap_or("");
                Some(format!("{key}（{chars} 字）「{head}」"))
            })
            .collect();
        if !rendered.is_empty() {
            out.push_str(indent);
            out.push_str("- 提示词：");
            out.push_str(&rendered.join(" · "));
            out.push('\n');
        }
    }
    for (label, key) in [
        ("模型", "models"),
        ("采样/尺寸", "sampling"),
        ("输入图", "images"),
        ("其它开关", "others"),
    ] {
        let Some(map) = obj
            .get(key)
            .and_then(Value::as_object)
            .filter(|m| !m.is_empty())
        else {
            continue;
        };
        let pairs: Vec<String> = map
            .iter()
            .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
            .collect();
        out.push_str(indent);
        out.push_str("- ");
        out.push_str(label);
        out.push('：');
        out.push_str(&pairs.join(" · "));
        out.push('\n');
    }
    out
}

/// 按字符（不是字节）截断，末尾补省略号。
fn clip(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 现场图（2026-11-14 03:41 用户自己在 ComfyUI 里跑的那一跑，节选）：
    /// 参数与"应用上次提交"的 0.45 / enhancer 关 不是一套 —— 这正是 AI 当时答错的地方。
    fn user_graph() -> Map<String, Value> {
        let value = json!({
            "29": {"class_type": "VAELoader", "inputs": {"vae_name": "qwen_image_vae.safetensors"}},
            "30": {"class_type": "CLIPLoader", "inputs": {"clip_name": "qwen3VL.safetensors", "type": "krea2"}},
            "31": {"class_type": "EmptyLatentImage", "inputs": {"width": 1024, "height": 1536, "batch_size": 1}},
            "33": {"class_type": "Krea2TurboReferenceSigmaSchedulerFromLatent", "inputs": {"steps": 8, "denoise": 1.0}},
            "35": {"class_type": "ComfyUI-Krea2T-Enhancer", "inputs": {"enabled": true, "strength": 1.25}},
            "37": {"class_type": "KSamplerSelect", "inputs": {"sampler_name": "er_sde"}},
            "42": {"class_type": "RandomNoise", "inputs": {"noise_seed": 441816587469389_i64}},
            "44": {"class_type": "UNETLoader", "inputs": {"unet_name": "krea2Turbo.safetensors"}},
            "55": {"class_type": "PrimitiveStringMultiline", "inputs": {"value": "@tanihara natsuki, An open lakeshore in the last of the afternoon, the camera set low and far back with a wide lens so the empty water fills the middle of the frame, the horizon one straight line across the lower third and the sky holding the two thirds above it, a girl stands partway down a low wooden jetty"}},
            "70": {"class_type": "Lora Loader Stack (rgthree)", "inputs": {
                "lora_01": "KNP_000003000.safetensors", "strength_01": 1.25,
                "lora_04": "tanihara_natsuki.safetensors", "strength_04": 0.8,
                "model": ["44", 0], "clip": ["30", 0]
            }},
            "39": {"class_type": "SaveImage", "inputs": {"filename_prefix": "test", "images": ["38", 0]}}
        });
        value.as_object().unwrap().clone()
    }

    fn value_of(pairs: &[(String, String)], key: &str) -> Option<String> {
        pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    #[test]
    fn 摘要读的是那一跑自己的参数而不是建议值() {
        let params = summarize(&user_graph());
        assert_eq!(value_of(&params.sampling, "33.steps").as_deref(), Some("8"));
        assert_eq!(
            value_of(&params.sampling, "37.sampler_name").as_deref(),
            Some("er_sde")
        );
        assert_eq!(
            value_of(&params.sampling, "42.noise_seed").as_deref(),
            Some("441816587469389")
        );
        // 尺寸来自 EmptyLatentImage（宽高不在 slot 里，是 Size 旋钮）
        assert_eq!(
            value_of(&params.sampling, "31.width").as_deref(),
            Some("1024")
        );

        assert_eq!(
            value_of(&params.others, "35.enabled").as_deref(),
            Some("true")
        );
        assert_eq!(
            value_of(&params.others, "35.strength").as_deref(),
            Some("1.25")
        );
        assert_eq!(
            value_of(&params.others, "70.strength_01").as_deref(),
            Some("1.25")
        );
        assert_eq!(
            value_of(&params.others, "70.strength_04").as_deref(),
            Some("0.8")
        );

        assert_eq!(
            value_of(&params.models, "44.unet_name").as_deref(),
            Some("krea2Turbo.safetensors")
        );
        assert_eq!(
            value_of(&params.models, "29.vae_name").as_deref(),
            Some("qwen_image_vae.safetensors")
        );

        assert_eq!(params.prompts.len(), 1);
        assert_eq!(params.prompts[0].key, "55.value");
        assert!(params.prompts[0].chars > PROMPT_HEAD_CHARS);
        assert!(params.prompts[0].head.starts_with("@tanihara natsuki"));

        let text = params.render("");
        assert!(text.contains("35.enabled=true"), "{text}");
        assert!(text.contains("70.strength_04=0.8"), "{text}");
        assert!(text.contains("55.value"), "{text}");
    }

    #[test]
    fn 从_history_形状的_raw_也能读参数() {
        let entry = json!({
            "prompt": [1, "run", user_graph(), {}, ["39"]],
            "outputs": {"39": {"images": [{"filename": "x.png"}]}}
        });
        let params = from_raw(&entry.to_string()).unwrap();
        assert_eq!(
            params
                .others
                .iter()
                .find(|(k, _)| k == "35.enabled")
                .map(|(_, v)| v.as_str()),
            Some("true")
        );
    }

    #[test]
    fn 空图如实给空摘要() {
        let params = summarize(&Map::new());
        assert!(params.is_empty());
        assert!(params.render("").contains("读不出参数"));
    }

    #[test]
    fn 长提示词只留开头() {
        let long = "x".repeat(PROMPT_HEAD_CHARS + 50);
        let graph = json!({
            "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "a.safetensors"}},
            "2": {"class_type": "CLIPTextEncode", "inputs": {"text": long, "clip": ["1", 1]}},
            "3": {"class_type": "SaveImage", "inputs": {"images": ["2", 0]}}
        });
        let params = summarize(graph.as_object().unwrap());
        assert_eq!(params.prompts[0].chars, PROMPT_HEAD_CHARS + 50);
        assert_eq!(
            params.prompts[0].head.chars().count(),
            PROMPT_HEAD_CHARS + 1,
            "截断后要带一个省略号"
        );
    }
}
