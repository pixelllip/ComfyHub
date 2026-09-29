//! 模型能力目录（`ModelCapabilityCatalog` 的逐条对照，P4 第 39 步）。
//!
//! # 三层，优先级从高到低
//!
//! | 层 | 文件 | 谁写 | 定位 |
//! | --- | --- | --- | --- |
//! | custom | `<storage>\ai\capabilities\custom.json` | 用户 / 界面导出 | 手工修正，最高优先 |
//! | remote | `<storage>\ai\capabilities\remote.json` | 在线数据集（models.dev） | 主力更新源 |
//! | builtin | [`crate::capability_rules`]（**生成**的 118 条） | 随版本发布 | 家族级兜底 |
//!
//! 匹配方式：custom / remote 按**精确 id**（大小写不敏感；网关 id 带 `org/` 前缀而数据集
//! 不带、或反过来时做**去前缀**匹配，但要求结果唯一 —— 有歧义宁可不命中）；
//! builtin 沿用**包含匹配**。同一维度三层都有时取上层；某层没给这一维度就落到下一层。
//!
//! # 两条容易做错的地方
//!
//! 1. **档位 / 方言必须与"支持推理"一致**：最终 `reasoning=false` 时一律清空
//!    （不管哪层给过）—— 留着就会出现"声明了档位却没勾推理"，而保存时
//!    `validate_model` 会拒掉这一行（用户看到的是"导入的模型存不进去"）。
//! 2. **坏文件当"该层不存在"，不许抛**：目录坏了不能让对话也起不来（Kotlin 同）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 目录版本（`ModelCapabilityCatalog.VERSION`，**改了要同时改两端**）。
pub const VERSION: &str = "2026-09b";

/// 覆盖层文件放 `<storage>\ai\capabilities\` 下（与 skills / memory 同一个 ai 子目录约定）。
pub const DIR_NAME: &str = "capabilities";
pub const CUSTOM_FILE: &str = "custom.json";
pub const REMOTE_FILE: &str = "remote.json";

/// 查表结论（`ModelCapabilityCatalog.Capability`）。
///
/// 档位与方言是 **owned**（`String`）而不是 `&'static str`：覆盖层那两层来自磁盘文件，
/// 用 `&'static` 就得 `Box::leak`（每次 lookup 漏一点内存），而这条链路每次导入模型都会走。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub modalities: Vec<String>,
    pub tools: bool,
    pub reasoning: bool,
    /// 命中的规则，便于向用户解释"为什么给它勾了图片"
    pub matched_by: String,
    /// 预填的思考档位（AIH-056）：等级 → 线上表达；空 = 声明了推理但**不预填**档位
    pub thinking_efforts: Vec<(String, String)>,
    /// 预填的思考方言（网关字段差异）；`None` = 按协议默认
    pub thinking_format: Option<String>,
}

/// 生成表里的一行（`capability_rules.rs`，**自动生成、别手改**）。
pub struct RawRule {
    pub pattern: &'static str,
    pub modalities: &'static [&'static str],
    pub tools: bool,
    pub reasoning: bool,
    pub efforts: Option<&'static [(&'static str, &'static str)]>,
    pub format: Option<&'static str>,
}

/// 保守默认（`ModelCapabilityCatalog.UNKNOWN`）：目录里没有、接口也没说 → **仅文本**。
///
/// 图片要用户自己勾（勾错了会被 Provider 拒绝，但不会被我们静默丢弃）。
/// 工具**默认给上**（用户要求）：很多网关不声明工具能力、但实际都支持。
pub fn unknown() -> Capability {
    Capability {
        modalities: vec!["text".to_string()],
        tools: true,
        reasoning: false,
        matched_by: "unknown".to_string(),
        thinking_efforts: Vec::new(),
        thinking_format: None,
    }
}

/// 内置规则（**按生成表原样建**：118 条、调用频率很低；缓存带来的失效风险不值这点开销）。
pub fn rules() -> Vec<(&'static str, Capability)> {
    crate::capability_rules::RAW_RULES
        .iter()
        .map(|raw| {
            (
                raw.pattern,
                Capability {
                    modalities: raw.modalities.iter().map(|m| m.to_string()).collect(),
                    tools: raw.tools,
                    reasoning: raw.reasoning,
                    matched_by: raw.pattern.to_string(),
                    thinking_efforts: raw
                        .efforts
                        .unwrap_or(&[])
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                    thinking_format: raw.format.map(str::to_string),
                },
            )
        })
        .collect()
}

/// 覆盖层里的一条模型声明（每个维度都可省 —— 省了就落到下一层，**绝不发明默认值**）。
///
/// ⚠ `skip_serializing_if` 是**刻意的**：Kotlin 的 `AppJson` 是 `explicitNulls = false`，
/// 写出来的 `remote.json` 里**没有 `null` 的键**。Rust 这边若写成 `"tools": null`，
/// 两版后端来回跑一次就会把同一份文件改得面目全非（虽然两边都能读，但 diff 会骗人）。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modalities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_efforts: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_format: Option<String>,
}

/// 覆盖层文件（`custom.json` / `remote.json`，同一个形状）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayFile {
    /// 数据集出处（remote 固定 "models.dev"；custom 默认 "custom"），只作展示
    #[serde(default = "default_custom_source")]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// remote：拉取时间（ISO-8601）；custom 可空
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
    /// key = 模型 id（加载时统一小写）
    #[serde(default)]
    pub entries: BTreeMap<String, OverlayEntry>,
}

fn default_custom_source() -> String {
    "custom".to_string()
}

/// `custom.json` / `remote.json` 的读取结果（文件不在 / 读坏 → `None`，**不抛**）。
#[derive(Debug, Default)]
pub struct Overlays {
    pub custom: Option<OverlayFile>,
    pub remote: Option<OverlayFile>,
}

/// 读两层覆盖层（Kotlin 的 `reload(dir)`，但**每次现读**而不是缓存）。
///
/// 为什么不缓存：`custom.json` 是用户手写/界面导出的，缓存了就会出现
/// "手工改完目录、重启前不生效"（与 Skills / 长期记忆同一条纪律：**磁盘是真源**）。
pub fn load_overlays(dir: &Path) -> Overlays {
    Overlays {
        custom: load_layer(&dir.join(CUSTOM_FILE), "custom"),
        remote: load_layer(&dir.join(REMOTE_FILE), "remote"),
    }
}

/// `<storage>\ai\capabilities`（与 `skills` / `memory` 同一个 ai 子目录约定）。
pub fn dir_of(storage: &Path) -> PathBuf {
    storage.join("ai").join(DIR_NAME)
}

fn load_layer(file: &Path, _label: &str) -> Option<OverlayFile> {
    let text = std::fs::read_to_string(file).ok()?;
    let parsed: OverlayFile = serde_json::from_str(&text).ok()?;
    // 加载时把 key 统一小写（Kotlin 在 `load` 里就做了这件事）
    let entries = parsed
        .entries
        .into_iter()
        .map(|(key, value)| (key.trim().to_lowercase(), value))
        .collect();
    Some(OverlayFile { entries, ..parsed })
}

/// 目录概况的一层（`OverlayMeta`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayMeta {
    pub source: String,
    pub fetched_at: Option<String>,
    pub entry_count: i64,
}

/// 目录概况（`CatalogStatus`，`/api/ai/capabilities/status` 用它）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogStatus {
    pub version: String,
    pub builtin_rules: i64,
    pub custom: Option<OverlayMeta>,
    pub remote: Option<OverlayMeta>,
}

/// `status()`：**只读**，不写任何东西。
pub fn status(dir: &Path) -> CatalogStatus {
    let overlays = load_overlays(dir);
    let meta = |file: Option<OverlayFile>| {
        file.map(|f| OverlayMeta {
            source: f.source,
            fetched_at: f.fetched_at,
            entry_count: f.entries.len() as i64,
        })
    };
    CatalogStatus {
        version: VERSION.to_string(),
        builtin_rules: rules().len() as i64,
        custom: meta(overlays.custom),
        remote: meta(overlays.remote),
    }
}

/// `remote.json` 的拉取时间（`CapabilityRefresh.remoteFetchedAt`）。
///
/// ⚠ 解析不出来 / 文件不在 → `None`；调用方据此判"过期"（`is_stale` 就是这么用的）。
pub fn remote_fetched_at(dir: &Path) -> Option<(i64, u32)> {
    let file = dir.join(REMOTE_FILE);
    let text = std::fs::read_to_string(file).ok()?;
    let parsed: OverlayFile = serde_json::from_str(&text).ok()?;
    crate::iso_time::parse(parsed.fetched_at.as_deref()?)
}

/// `remoteAgeHours`：拉取时间到现在的小时数（`None` = 没拉过 / 时间读不出来）。
pub fn remote_age_hours(dir: &Path, now_secs: i64) -> Option<i64> {
    let (secs, _) = remote_fetched_at(dir)?;
    Some((now_secs - secs) / 3600)
}

/// `isStale`：**没拉过就算过期**（`None → true`），超过 [`REFRESH_INTERVAL_DAYS`] 天也算。
pub const REFRESH_INTERVAL_DAYS: i64 = 7;

pub fn is_stale(dir: &Path, now_secs: i64) -> bool {
    match remote_age_hours(dir, now_secs) {
        None => true,
        Some(age) => age > REFRESH_INTERVAL_DAYS * 24,
    }
}

// ---------------------------------------------------------------------------
//  查表（`lookup`）—— 三层逐维度合并
// ---------------------------------------------------------------------------

/// builtin 的**包含匹配**（先命中先用；宽前缀规则排在后面，免得 `gpt-4` 抢走 `gpt-4o`）。
fn lookup_builtin(rules: &[(&'static str, Capability)], id: &str) -> Option<(&'static str, Capability)> {
    rules
        .iter()
        .find(|(pattern, _)| id.contains(pattern))
        .map(|(pattern, cap)| (*pattern, cap.clone()))
}

/// 覆盖层匹配：**精确 id** → **去前缀**（两个方向），去前缀要求**唯一命中**。
///
/// 「两个条目都叫 `…/dup` 时宁可不命中，绝不猜」—— 猜错的代价是给模型勾错模态，
/// 而那是**会发出错误请求**的（会被网关拒，但用户看不出为什么）。
fn resolve_overlay(overlay: Option<&OverlayFile>, id: &str) -> Option<OverlayEntry> {
    let overlay = overlay?;
    if let Some(hit) = overlay.entries.get(id) {
        return Some(hit.clone());
    }
    // 数据集带前缀（"z-ai/xxx"）、网关 id 不带（"xxx"）
    let by_suffix: Vec<&OverlayEntry> = overlay
        .entries
        .iter()
        .filter(|(key, _)| key.as_str() != id && key.rsplit('/').next() == Some(id))
        .map(|(_, value)| value)
        .collect();
    if by_suffix.len() == 1 {
        return Some(by_suffix[0].clone());
    }
    // 网关 id 带前缀（"org/xxx"）、数据集条目不带（"xxx"）
    let bare = id.rsplit('/').next().unwrap_or(id);
    if bare != id {
        let by_bare: Vec<&OverlayEntry> =
            overlay.entries.iter().filter(|(key, _)| key.as_str() == bare).map(|(_, v)| v).collect();
        if by_bare.len() == 1 {
            return Some(by_bare[0].clone());
        }
    }
    None
}

/// 来源说明（单行，进界面 / `capabilityNote`）。
fn matched_by(
    custom_hit: Option<&OverlayEntry>,
    remote_hit: Option<&OverlayEntry>,
    remote_source: Option<&str>,
    builtin_hit: Option<&(&'static str, Capability)>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if custom_hit.is_some() {
        parts.push("手工目录 custom.json".to_string());
    }
    if remote_hit.is_some() {
        parts.push(format!("在线目录 {}", remote_source.unwrap_or("remote.json")));
    }
    if let Some((pattern, _)) = builtin_hit {
        parts.push(format!("内置规则「{pattern}」"));
    }
    if parts.is_empty() {
        "unknown".to_string()
    } else {
        parts.join(" + ")
    }
}

/// 查表；`None` 表示"未知"，调用方必须按 [`unknown`] 的保守默认处理。
///
/// 逐维度：custom > remote > builtin；某层没给这一维度就落到下一层。
pub fn lookup(overlays: &Overlays, model_id: &str) -> Option<Capability> {
    let id = model_id.trim().to_lowercase();
    // ⚠ 先把表**绑到变量**再借：直接 `lookup_builtin(&rules(), …)` 借的是临时值
    let table = rules();
    let builtin = lookup_builtin(&table, &id);
    let remote_hit = resolve_overlay(overlays.remote.as_ref(), &id);
    let custom_hit = resolve_overlay(overlays.custom.as_ref(), &id);
    if builtin.is_none() && remote_hit.is_none() && custom_hit.is_none() {
        return None;
    }

    let modalities = custom_hit
        .as_ref()
        .and_then(|h| h.modalities.clone())
        .or_else(|| remote_hit.as_ref().and_then(|h| h.modalities.clone()))
        .or_else(|| builtin.as_ref().map(|(_, cap)| cap.modalities.clone()))
        .unwrap_or_else(|| vec!["text".to_string()]);
    let tools = custom_hit
        .as_ref()
        .and_then(|h| h.tools)
        .or_else(|| remote_hit.as_ref().and_then(|h| h.tools))
        .or_else(|| builtin.as_ref().map(|(_, cap)| cap.tools))
        .unwrap_or(true);
    let reasoning = custom_hit
        .as_ref()
        .and_then(|h| h.reasoning)
        .or_else(|| remote_hit.as_ref().and_then(|h| h.reasoning))
        .or_else(|| builtin.as_ref().map(|(_, cap)| cap.reasoning))
        .unwrap_or(false);
    // 档位 / 方言必须与"支持推理"一致：最终 reasoning=false 时一律清空，不管哪层给过
    let (thinking_efforts, thinking_format) = if !reasoning {
        (Vec::new(), None)
    } else {
        let efforts_from_overlay = |hit: &Option<OverlayEntry>| -> Option<Vec<(String, String)>> {
            hit.as_ref()
                .and_then(|h| h.thinking_efforts.as_ref())
                .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        };
        let efforts = efforts_from_overlay(&custom_hit)
            .or_else(|| efforts_from_overlay(&remote_hit))
            .or_else(|| builtin.as_ref().map(|(_, cap)| cap.thinking_efforts.clone()))
            .unwrap_or_default();
        let format = custom_hit
            .as_ref()
            .and_then(|h| h.thinking_format.clone())
            .filter(|f| !f.trim().is_empty())
            .or_else(|| {
                remote_hit
                    .as_ref()
                    .and_then(|h| h.thinking_format.clone())
                    .filter(|f| !f.trim().is_empty())
            })
            .or_else(|| builtin.as_ref().and_then(|(_, cap)| cap.thinking_format.clone()));
        (efforts, format)
    };

    Some(Capability {
        modalities,
        tools,
        reasoning,
        matched_by: matched_by(
            custom_hit.as_ref(),
            remote_hit.as_ref(),
            overlays.remote.as_ref().map(|r| r.source.as_str()),
            builtin.as_ref(),
        ),
        thinking_efforts,
        thinking_format,
    })
}

/// 目录概况的 JSON 形状（端点直接回它）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogStatusDto {
    pub version: String,
    pub builtin_rules: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_fetched_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_entries: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_fetched_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_entries: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_age_hours: Option<i64>,
    pub remote_stale: bool,
    pub refresh_interval_days: i64,
    /// 是否允许实测校准（发真实请求；默认关）
    pub probe_enabled: bool,
    /// 最近一次（手动）刷新的人话结果；状态接口不产生这句话时**键不出现**
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ 每个用例**自己一个目录**：共用 `comfyhub-cap-<pid>` 会让并行用例互相覆盖
    /// `custom.json`（这个坑在本仓库踩过不止一次，见 docs/pitfalls.md 那条"偶发红"）。
    fn dir(tag: &str) -> PathBuf {
        crate::testpaths::scratch(&format!("cap-{tag}"))
    }

    /// ★ 内置规则的**条数与 Kotlin 一致**（118 条）—— 这是"生成器漏规则"的绊线。
    ///
    /// 真机验收靠"两端 `/api/ai/capabilities/status` 的 `builtinRules` 相等"，
    /// 但那条要起两个后端；这条能在 `cargo test` 里就把它钉住。
    #[test]
    fn 内置规则条数与生成源一致() {
        assert_eq!(rules().len(), 118, "规则表变了？重跑 tools/gen_capability_rules.py");
        // 前几条的顺序**就是语义**（宽前缀在后），抽查开头与结尾
        assert_eq!(rules()[0].0, "claude-sonnet-5");
        assert_eq!(rules().last().unwrap().0, "grok");
        assert!(rules().iter().any(|(p, _)| *p == "deepseek-v4.1"));
        // 编码是对的：`claude` 那条不能抢在 `claude-sonnet-5` 前面
        let idx = |name: &str| rules().iter().position(|(p, _)| *p == name).unwrap();
        assert!(idx("claude-sonnet-5") < idx("claude"), "宽前缀必须排在后面");
        assert!(idx("gpt-5.6") < idx("gpt-5"));
        assert!(idx("deepseek-v4.1") < idx("deepseek-v4"));
    }

    /// 包含匹配 + 先命中先用：图片 / 工具 / 推理 / 档位 / 方言都照表来。
    #[test]
    fn 内置匹配按包含且先命中先用() {
        let overlays = Overlays::default();
        let hit = lookup(&overlays, "anthropic/claude-sonnet-5").unwrap();
        assert_eq!(hit.modalities, vec!["text", "image"]);
        assert!(hit.tools && hit.reasoning);
        assert_eq!(hit.thinking_format, None, "claude 系没有特殊方言");
        assert_eq!(hit.matched_by, "内置规则「claude-sonnet-5」");
        assert!(hit.thinking_efforts.iter().any(|(k, v)| *k == "max" && *v == "max"));

        // deepseek-v4.1：带图片 + 思考档位 + deepseek 方言（这一条 reasoning=true，方言留着）
        let deepseek = lookup(&overlays, "deepseek-v4.1").unwrap();
        assert_eq!(deepseek.modalities, vec!["text", "image"]);
        assert!(deepseek.reasoning);
        assert_eq!(deepseek.thinking_format.as_deref(), Some("deepseek"));
        assert!(deepseek.thinking_efforts.iter().any(|(k, _)| k == "max"));

        // deepseek-chat：只吃文本、**不声明推理** → 方言也一并清空（Kotlin 同：
        // `val format = if (!reasoning) null else …`）。
        // 这条看着反直觉（"方言又不是推理能力"），但留着它就会与 `validate_model` 打架。
        let chat = lookup(&overlays, "deepseek-chat").unwrap();
        assert_eq!(chat.modalities, vec!["text"]);
        assert!(!chat.reasoning, "deepseek-chat 不声明推理");
        assert_eq!(chat.thinking_format, None, "不支持推理 → 方言与档位一律清空");
        assert!(chat.thinking_efforts.is_empty());

        // 目录里没有的模型 → `None`（调用方按 unknown() 兜底）
        assert!(lookup(&overlays, "some-unknown-model-3000").is_none());
        let fallback = unknown();
        assert_eq!(fallback.modalities, vec!["text"]);
        assert!(fallback.tools, "工具默认给上（很多网关不声明但实际支持）");
        assert!(!fallback.reasoning);
    }

    /// 覆盖层：**精确匹配 → 去前缀（两个方向）→ 有歧义就不命中**。
    #[test]
    fn 覆盖层去前缀只在唯一命中时才认() {
        let d = dir("overlay");
        std::fs::write(
            d.join(CUSTOM_FILE),
            r#"{"source":"custom","entries":{
                 "z-ai/glm-5": {"tools": false},
                 "a/x": {"tools": false},
                 "b/x": {"tools": false},
                 "plain": {"reasoning": true}
               }}"#,
        )
        .unwrap();
        let overlays = load_overlays(&d);

        // 精确命中（大小写不敏感）
        let hit = lookup(&overlays, "Z-AI/GLM-5").unwrap();
        assert!(!hit.tools);
        assert!(hit.matched_by.contains("手工目录 custom.json"), "{}", hit.matched_by);

        // 网关 id 不带前缀 → 数据集条目的后缀唯一命中
        assert!(!lookup(&overlays, "glm-5").unwrap().tools);

        // 有歧义（a/x 与 b/x 都以 /x 结尾）→ **三层都不命中**，落回调用方的保守默认
        // （猜错的代价是给模型勾错模态，而那是会发出错误请求的）
        assert!(
            lookup(&overlays, "x").is_none(),
            "两个候选时宁可不命中（`lookup` 回 None，调用方按 unknown() 兜底）"
        );

        // 只声明了一个维度：其余维度落回内置
        let plain = lookup(&overlays, "plain").unwrap();
        assert!(plain.reasoning);
        assert_eq!(plain.modalities, vec!["text"], "覆盖层没说模态 → 落回内置的 TEXT_ONLY");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ★ 最终 `reasoning=false` 时，档位与方言**一律清空**（不管哪层给过）。
    ///
    /// 留着就会与 `validate_model` 打架：那一条会拒掉"声明了档位却没勾推理"，
    /// 而用户看到的是"导入进来的模型存不进去"。
    #[test]
    fn 不支持推理时档位与方言一律清空() {
        let d = dir("reasoning");
        // 内置 `kimi-k2.5` 是 TEXT_IMAGE、不支持推理；覆盖层硬塞档位与方言
        std::fs::write(
            d.join(CUSTOM_FILE),
            r#"{"entries":{"kimi-k2.5":{"thinkingEfforts":{"high":"high"},"thinkingFormat":"zai"}}}"#,
        )
        .unwrap();
        let overlays = load_overlays(&d);
        let hit = lookup(&overlays, "kimi-k2.5").unwrap();
        assert!(!hit.reasoning);
        assert!(hit.thinking_efforts.is_empty(), "不支持推理就一个档位都不许留");
        assert_eq!(hit.thinking_format, None, "不支持推理就不给方言");
        // 而同一层的条目如果**明说**支持推理，档位就保留
        std::fs::write(
            d.join(CUSTOM_FILE),
            r#"{"entries":{"kimi-k2.5":{"reasoning":true,"thinkingEfforts":{"high":"high"},"thinkingFormat":"zai"}}}"#,
        )
        .unwrap();
        let hit = lookup(&load_overlays(&d), "kimi-k2.5").unwrap();
        assert!(hit.reasoning);
        assert_eq!(hit.thinking_format.as_deref(), Some("zai"));
        assert_eq!(hit.thinking_efforts, vec![("high".to_string(), "high".to_string())]);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 状态：三层各自的概况 + `builtinRules`；坏文件当"该层不存在"（**不抛**）。
    #[test]
    fn 状态读三层而坏文件不炸() {
        let d = dir("status");
        // 先什么都没有
        let empty = status(&d);
        assert_eq!(empty.version, VERSION);
        assert_eq!(empty.builtin_rules, 118);
        assert!(empty.custom.is_none() && empty.remote.is_none());
        assert!(is_stale(&d, 1_000_000), "没拉过就算过期");

        // 两层都在
        std::fs::write(d.join(CUSTOM_FILE), r#"{"source":"custom","entries":{"a":{},"b":{}}}"#).unwrap();
        std::fs::write(
            d.join(REMOTE_FILE),
            r#"{"source":"models.dev","fetchedAt":"2026-09-25T00:00:00Z","entries":{"a":{}}}"#,
        )
        .unwrap();
        let s = status(&d);
        assert_eq!(s.custom.as_ref().unwrap().entry_count, 2);
        assert_eq!(s.custom.as_ref().unwrap().source, "custom");
        assert_eq!(s.remote.as_ref().unwrap().source, "models.dev");
        assert_eq!(s.remote.as_ref().unwrap().fetched_at.as_deref(), Some("2026-09-25T00:00:00Z"));
        // 拉取时间 → 年龄 / 过期（7 天 = 168 小时）
        // ⚠ 用**解析出来的**那个时刻做基准，别自己手算 epoch（手算错一位就是一条假红的用例）
        let (fetched, _) = crate::iso_time::parse("2026-09-25T00:00:00Z").unwrap();
        assert_eq!(remote_age_hours(&d, fetched + 3600 * 5).unwrap(), 5);
        assert!(!is_stale(&d, fetched + 3600 * 100), "100 小时不算过期");
        assert!(is_stale(&d, fetched + 3600 * 200), "200 小时 > 7 天算过期");
        // 默认 source（文件里没写）
        std::fs::write(d.join(CUSTOM_FILE), r#"{"entries":{}}"#).unwrap();
        assert_eq!(status(&d).custom.unwrap().source, "custom");

        // 坏 JSON → 该层当不存在，**不许抛**
        std::fs::write(d.join(CUSTOM_FILE), "{ 这不是 JSON").unwrap();
        let broken = status(&d);
        assert!(broken.custom.is_none(), "坏文件当没有这一层");
        assert!(broken.remote.is_some(), "另一层不受影响");
        let _ = std::fs::remove_dir_all(&d);
    }
}
