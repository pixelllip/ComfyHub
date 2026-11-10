//! 模型能力目录（`ModelCapabilityCatalog` 的逐条对照，P4 第 39 步）。
//!
//! # 三层，优先级从高到低
//!
//! | 层 | 文件 | 谁写 | 定位 |
//! | --- | --- | --- | --- |
//! | custom | `<storage>\ai\capabilities\custom.json` | 手动 / 界面导出 | 手工修正，最高优先 |
//! | remote | `<storage>\ai\capabilities\remote.json` | 在线数据集（models.dev） | 主力更新源 |
//! | builtin | [`crate::provider::capability_rules`]（**生成**的 119 条） | 随版本发布 | 家族级兜底 |
//!
//! 匹配方式：custom / remote 按**精确 id**（大小写不敏感；网关 id 带 `org/` 前缀而数据集
//! 不带、或反过来时做**去前缀**匹配，但要求结果唯一 —— 有歧义宁可不命中）；
//! builtin 沿用**包含匹配**。同一维度三层都有时取上层；某层没给这一维度就落到下一层。
//!
//! # 两条容易做错的地方
//!
//! 1. **档位 / 方言必须与"支持推理"一致**：最终 `reasoning=false` 时一律清空
//!    （不管哪层给过）—— 留着就会出现"声明了档位却没勾推理"，而保存时
//!    `validate_model` 会拒绝该行（表现为"导入的模型无法保存"）。
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
    /// 命中的规则，用于解释"为什么为其勾选了图片"
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
/// 图片需手动勾选（勾错会被 Provider 拒绝，但不会被静默丢弃）。
/// 工具**默认启用**：许多网关不声明工具能力，但实际支持。
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

/// 内置规则（**按生成表原样建**：119 条、调用频率很低；缓存带来的失效风险不值这点开销）。
pub fn rules() -> Vec<(&'static str, Capability)> {
    crate::provider::capability_rules::RAW_RULES
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
/// 不缓存的原因：`custom.json` 由手动编写或界面导出，缓存后会出现
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
    crate::infra::iso_time::parse(parsed.fetched_at.as_deref()?)
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
fn lookup_builtin(
    rules: &[(&'static str, Capability)],
    id: &str,
) -> Option<(&'static str, Capability)> {
    rules
        .iter()
        .find(|(pattern, _)| id.contains(pattern))
        .map(|(pattern, cap)| (*pattern, cap.clone()))
}

/// 覆盖层匹配：**精确 id** → **去前缀**（两个方向），去前缀要求**唯一命中**。
///
/// 「两个条目都叫 `…/dup` 时宁可不命中，绝不猜」—— 猜错的代价是给模型勾错模态，
/// 而那是**会发出错误请求**的（会被网关拒绝，但无法判断原因）。
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
        let by_bare: Vec<&OverlayEntry> = overlay
            .entries
            .iter()
            .filter(|(key, _)| key.as_str() == bare)
            .map(|(_, v)| v)
            .collect();
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
        parts.push(format!(
            "在线目录 {}",
            remote_source.unwrap_or("remote.json")
        ));
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
            .or_else(|| {
                builtin
                    .as_ref()
                    .map(|(_, cap)| cap.thinking_efforts.clone())
            })
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
            .or_else(|| {
                builtin
                    .as_ref()
                    .and_then(|(_, cap)| cap.thinking_format.clone())
            });
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
mod tests;
