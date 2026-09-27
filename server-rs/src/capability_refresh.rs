//! 模型能力目录的**在线刷新**（`POST /api/ai/capabilities/refresh`）。
//!
//! 逐条对照 Kotlin `legacy/server/.../ai/CapabilityRefresh.kt`（那份是行为真源）。
//! 数据从 `https://models.dev/api.json` 来：它聚合了各家官方文档的模态 / `tool_call` /
//! `reasoning` / **思考档位**，免密钥、更新勤。拉下来规范化成 [`crate::capability_catalog::OverlayFile`]
//! 写进 `<storage>\ai\capabilities\remote.json` —— **不用重启、不用发版**。
//!
//! # 四条纪律（前三条与 Kotlin 一字不差，第四条是 Rust 侧多写的）
//!
//! 1. **远端数据只是数据**：只取几个固定字段，绝不执行其中任何内容（与工具结果同一条纪律）；
//! 2. **失败不阻塞**：出网失败 / 解析失败都只如实回结果，**旧文件原样保留** ——
//!    "刷新失败"绝不能把本来能用的目录弄坏；
//! 3. **`COMFYHUB_NO_DOWNLOAD=1` 一刀关**（与 Java / MCP 运行时那条链同一个开关）；
//! 4. **不猜方言**：线上数据集里没有"思考方言"（`thinkingFormat`）这个概念 ——
//!    猜错比不填更糟，所以这里**永远不填**它（方言由内置家族规则给）。

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

use crate::capability_catalog::{OverlayEntry, OverlayFile, REMOTE_FILE};

/// 数据集出处（写进 `remote.json` 的 `source`，只作展示）。
pub const SOURCE: &str = "models.dev";
/// 数据集地址（**固定常量**，不是模型/用户给的 → 不跟随重定向也没有 SSRF 面）。
pub const URL: &str = "https://models.dev/api.json";
/// 出网超时（Kotlin 同值 30 秒）。
pub const HTTP_TIMEOUT_SECONDS: u64 = 30;

/// 一次刷新的结果（人话 + 事实）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshResult {
    pub ok: bool,
    /// 主动跳过（`COMFYHUB_NO_DOWNLOAD=1` / 还没过期）—— **不算坏**，界面上如实显示原因。
    pub skipped: bool,
    pub message: String,
    pub entry_count: usize,
    pub fetched_at: Option<String>,
}

impl RefreshResult {
    fn skipped(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            skipped: true,
            message: message.into(),
            entry_count: 0,
            fetched_at: None,
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            skipped: false,
            message: message.into(),
            entry_count: 0,
            fetched_at: None,
        }
    }
}

/// 现在（unix 秒）—— `is_stale` 要它。
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `COMFYHUB_NO_DOWNLOAD` 是否关掉了在线更新（与 Java / MCP 运行时同一个开关）。
pub fn no_download() -> bool {
    std::env::var("COMFYHUB_NO_DOWNLOAD")
        .map(|v| !v.trim().is_empty() && v.trim() != "0")
        .unwrap_or(false)
}

/// 该不该跳过这次刷新（**纯函数**，用例直接喂三个布尔）。
///
/// 返回 `Some(人话)` = 跳过（并把这句如实回给界面）。
pub fn skip_reason(force: bool, no_download: bool, stale: bool) -> Option<String> {
    if no_download {
        return Some(format!(
            "COMFYHUB_NO_DOWNLOAD=1 已关闭在线更新（目录仍用内置表 + 已有 remote.json）"
        ));
    }
    if !force && !stale {
        return Some(format!(
            "remote.json 还没过期（每 {} 天自动拉一次），要立刻更新请用强制刷新",
            crate::capability_catalog::REFRESH_INTERVAL_DAYS
        ));
    }
    None
}

/// 一次走完：判过期 → 出网 → 解析落盘。**任何失败都不动旧文件。**
pub async fn refresh(
    state: &crate::routes::AppState,
    force: bool,
) -> RefreshResult {
    let dir = crate::capability_catalog::dir_of(
        state.media_dir().parent().unwrap_or(state.media_dir()),
    );
    let stale = crate::capability_catalog::is_stale(&dir, unix_now());
    if let Some(message) = skip_reason(force, no_download(), stale) {
        return RefreshResult::skipped(message);
    }

    let Some(host) = crate::upstream::host_of(URL) else {
        return RefreshResult::failed("数据集地址解析失败（这是内置常量，属于程序 bug）");
    };
    let client = match crate::upstream::client_for(&host) {
        Ok(client) => client,
        Err(error) => {
            return RefreshResult::failed(format!("HTTP 客户端创建失败：{}", error.human()))
        }
    };
    let response = match client
        .get(URL)
        .header("accept", "application/json")
        .header("user-agent", "ComfyHub")
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECONDS))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return RefreshResult::failed(format!("拉取失败：{error}（旧目录原样保留）"));
        }
    };
    let status = response.status();
    if !status.is_success() {
        return RefreshResult::failed(format!(
            "models.dev 返回 HTTP {}（旧目录原样保留）",
            status.as_u16()
        ));
    }
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => return RefreshResult::failed(format!("读取返回体失败：{error}（旧目录原样保留）")),
    };

    let fetched_at = crate::iso_time::from_system_time(std::time::SystemTime::now());
    match parse_and_write(&body, &dir, &fetched_at) {
        Ok(count) => RefreshResult {
            ok: true,
            skipped: false,
            message: format!("已从 {SOURCE} 更新 {count} 条模型声明"),
            entry_count: count,
            fetched_at: Some(fetched_at),
        },
        Err(message) => RefreshResult::failed(format!("{message}（旧目录原样保留）")),
    }
}

// ---------------------------------------------------------------------------
//  解析 + 落盘（**纯函数**：body 是注入的，离线可测）
// ---------------------------------------------------------------------------

/// models.dev 一条模型 → 我们的覆盖层条目（一个有用维度都没有 → `None`，不占地方）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    id: String,
    modalities: Option<Vec<String>>,
    tools: Option<bool>,
    reasoning: Option<bool>,
    efforts: Option<BTreeMap<String, String>>,
    last_updated: Option<String>,
}

impl Entry {
    fn to_overlay(&self) -> OverlayEntry {
        OverlayEntry {
            modalities: self.modalities.clone(),
            tools: self.tools,
            reasoning: self.reasoning,
            thinking_efforts: self.efforts.clone(),
            // ★ 方言**永远不填**（见文件头纪律 4）
            thinking_format: None,
        }
    }

    fn any(&self) -> bool {
        self.modalities.is_some()
            || self.tools.is_some()
            || self.reasoning.is_some()
            || self.efforts.is_some()
    }
}

fn str_of(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// `false` 也是一条**声明**（"明确不支持工具"）：只有字段缺失 / 类型不对才是 `None`。
fn bool_of(obj: &Map<String, Value>, key: &str) -> Option<bool> {
    obj.get(key).and_then(Value::as_bool)
}

/// 线上模态词 → 我们的词表（`pdf` → `document`，与 `AiUpstream.modalityTokens` 同一套映射 ——
/// **别再发明第三套**）。
fn modality_of(token: &str) -> Option<&'static str> {
    match token.trim().to_lowercase().as_str() {
        "text" | "prompt" => Some("text"),
        "image" | "vision" => Some("image"),
        "video" => Some("video"),
        "audio" => Some("audio"),
        "pdf" | "file" | "document" => Some("document"),
        _ => None,
    }
}

/// 线上档位值 → 我们的等级键：`none` → `off`，其余同名；**不认识的值跳过（绝不发明）**。
fn effort_of(raw: &str) -> Option<(String, String)> {
    let v = raw.trim().to_lowercase();
    match v.as_str() {
        "none" => Some(("off".to_string(), "none".to_string())),
        "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => {
            Some((v.clone(), v))
        }
        _ => None,
    }
}

fn normalize(key: &str, model: &Map<String, Value>) -> Option<Entry> {
    let id = str_of(model, "id").unwrap_or_else(|| key.to_string()).trim().to_lowercase();
    if id.is_empty() || id.chars().any(char::is_control) {
        return None;
    }

    let modalities = model
        .get("modalities")
        .and_then(Value::as_object)
        .and_then(|m| m.get("input"))
        .and_then(Value::as_array)
        .map(|input| {
            let mut out: Vec<String> = Vec::new();
            for item in input {
                let Some(token) = item.as_str() else { continue };
                let Some(mapped) = modality_of(token) else { continue };
                if !out.iter().any(|v| v == mapped) {
                    out.push(mapped.to_string());
                }
            }
            out
        })
        .filter(|v| !v.is_empty());

    let tools = bool_of(model, "tool_call");
    let reasoning_declared = bool_of(model, "reasoning");

    let mut efforts: Option<BTreeMap<String, String>> = None;
    if let Some(options) = model.get("reasoning_options").and_then(Value::as_array) {
        let mut map: BTreeMap<String, String> = BTreeMap::new();
        for option in options {
            let Some(option) = option.as_object() else { continue };
            if option.get("type").and_then(Value::as_str) != Some("effort") {
                continue;
            }
            let Some(values) = option.get("values").and_then(Value::as_array) else { continue };
            for raw in values {
                let Some(raw) = raw.as_str() else { continue };
                if let Some((k, v)) = effort_of(raw) {
                    map.insert(k, v);
                }
            }
        }
        if !map.is_empty() {
            // `reasoning` 明确为 false 的条目**不许带档位**（validateThinkingEfforts 的纪律）
            if reasoning_declared != Some(false) {
                efforts = Some(map);
            }
        }
    }

    // 数据集给了档位却没给 reasoning 布尔：能给档位就说明支持推理（**诚实推断**，不是发明）
    let reasoning = reasoning_declared.or_else(|| efforts.as_ref().map(|_| true));

    let entry = Entry {
        id,
        modalities,
        tools,
        reasoning,
        efforts,
        last_updated: str_of(model, "last_updated"),
    };
    if !entry.any() {
        return None;
    }
    Some(entry)
}

/// ISO-8601 字典序 = 时间序（`None` 排最后）。
fn newer_than(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(a), Some(b)) => a >= b,
    }
}

/// 多家 provider 带同一个模型：**逐维度合并**（新的优先、旧的补位）——
/// 新条目没给模态，不该把旧条目的模态信息挤掉。合并后 `reasoning == false` 时不留档位。
fn merge_entries(a: Entry, b: Entry) -> Entry {
    let (newer, older) = if newer_than(a.last_updated.as_deref(), b.last_updated.as_deref()) {
        (a, b)
    } else {
        (b, a)
    };
    let reasoning = newer.reasoning.or(older.reasoning);
    let efforts = newer
        .efforts
        .clone()
        .or_else(|| older.efforts.clone())
        .filter(|_| reasoning != Some(false));
    let last_updated = match (newer.last_updated.clone(), older.last_updated.clone()) {
        (Some(x), Some(y)) => Some(if x >= y { x } else { y }),
        (x, y) => x.or(y),
    };
    Entry {
        id: newer.id,
        modalities: newer.modalities.or(older.modalities),
        tools: newer.tools.or(older.tools),
        reasoning,
        efforts,
        last_updated,
    }
}

/// 解析数据集并写 `remote.json`，返回条目数。**解析不出任何模型就报错，不动旧文件。**
pub fn parse_and_write(body: &str, dir: &Path, fetched_at: &str) -> Result<usize, String> {
    let root: Value =
        serde_json::from_str(body).map_err(|e| format!("返回体不是 JSON：{e}"))?;
    let root = root
        .as_object()
        .ok_or_else(|| "返回体不是 JSON 对象".to_string())?;
    // 实测有两种形状：顶层直接是 provider map，或者包一层 `models`
    let providers = root
        .get("models")
        .and_then(Value::as_object)
        .unwrap_or(root);

    let mut merged: BTreeMap<String, Entry> = BTreeMap::new();
    for provider in providers.values() {
        let Some(provider) = provider.as_object() else { continue };
        let Some(models) = provider.get("models").and_then(Value::as_object) else { continue };
        for (key, model) in models {
            let Some(model) = model.as_object() else { continue };
            let Some(entry) = normalize(key, model) else { continue };
            merged
                .entry(entry.id.clone())
                .and_modify(|existing| {
                    *existing = merge_entries(entry.clone(), existing.clone());
                })
                .or_insert(entry);
        }
    }
    if merged.is_empty() {
        return Err("数据集里没有解析出任何模型（结构可能变了）".to_string());
    }

    let overlay = OverlayFile {
        source: SOURCE.to_string(),
        version: Some(SOURCE.to_string()),
        fetched_at: Some(fetched_at.to_string()),
        entries: merged
            .iter()
            .map(|(id, entry)| (id.clone(), entry.to_overlay()))
            .collect(),
    };
    let json = serde_json::to_string_pretty(&overlay)
        .map_err(|e| format!("覆盖层序列化失败：{e}"))?;

    std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败（{}）：{e}", dir.display()))?;
    let target = dir.join(REMOTE_FILE);
    let tmp = dir.join(format!("{REMOTE_FILE}.tmp"));
    std::fs::write(&tmp, json).map_err(|e| format!("写临时文件失败（{}）：{e}", tmp.display()))?;
    // Windows 上 `fs::rename` 走 `MoveFileEx(MOVEFILE_REPLACE_EXISTING)`，可以直接覆盖
    std::fs::rename(&tmp, &target)
        .map_err(|e| format!("替换 {} 失败：{e}", target.display()))?;
    Ok(merged.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(json: &str) -> Map<String, Value> {
        serde_json::from_str::<Value>(json).unwrap().as_object().unwrap().clone()
    }

    #[test]
    fn 跳过判据三档() {
        // NO_DOWNLOAD 一刀关（优先级最高）
        assert!(skip_reason(true, true, true).unwrap().contains("COMFYHUB_NO_DOWNLOAD=1"));
        // 没强制 + 没过期 → 跳过（后台巡检那条路）
        assert!(skip_reason(false, false, false).unwrap().contains("还没过期"));
        // 强制 → 不跳
        assert!(skip_reason(true, false, false).is_none());
        // 过期 → 不跳（即使没强制）
        assert!(skip_reason(false, false, true).is_none());
    }

    #[test]
    fn normalize_模态映射与_pdf_归到_document() {
        let e = normalize(
            "claude-x",
            &obj(r#"{"id":"Claude-X","modalities":{"input":["text","image","pdf","weird"]},"tool_call":true}"#),
        )
        .unwrap();
        assert_eq!(e.id, "claude-x", "id 统一小写");
        assert_eq!(e.modalities.unwrap(), vec!["text", "image", "document"]);
        assert_eq!(e.tools, Some(true));
        assert_eq!(e.reasoning, None, "数据集没给就是没给（不发明）");
    }

    #[test]
    fn normalize_档位_none_归到_off_且_reasoning_缺省时按有档位推为_true() {
        let e = normalize(
            "m",
            &obj(r#"{"modalities":{"input":["text"]},"reasoning_options":[{"type":"effort","values":["none","low","xhigh","bogus"]}]}"#),
        )
        .unwrap();
        let efforts = e.efforts.unwrap();
        assert_eq!(efforts.get("off").map(String::as_str), Some("none"));
        assert_eq!(efforts.get("low").map(String::as_str), Some("low"));
        assert_eq!(efforts.get("xhigh").map(String::as_str), Some("xhigh"));
        assert!(!efforts.contains_key("bogus"), "不认识的值必须跳过：{efforts:?}");
        assert_eq!(e.reasoning, Some(true), "给了档位 = 支持推理（诚实推断）");
    }

    #[test]
    fn normalize_reasoning_明确_false_的条目不许带档位() {
        let e = normalize(
            "m",
            &obj(r#"{"reasoning":false,"reasoning_options":[{"type":"effort","values":["low"]}]}"#),
        )
        .unwrap();
        assert_eq!(e.reasoning, Some(false));
        assert!(e.efforts.is_none(), "false 与档位互斥");
    }

    #[test]
    fn normalize_一个维度都没有的条目不占地方() {
        assert!(normalize("m", &obj(r#"{"name":"啥也没声明"}"#)).is_none());
        assert!(normalize("", &obj(r#"{"id":"  "}"#)).is_none());
    }

    #[test]
    fn 合并_新的优先旧的补位_且合并后_false_不留档位() {
        let older = Entry {
            id: "m".into(),
            modalities: Some(vec!["text".into(), "image".into()]),
            tools: Some(true),
            reasoning: Some(true),
            efforts: None,
            last_updated: Some("2026-01-01".into()),
        };
        let newer = Entry {
            id: "m".into(),
            modalities: None, // 新条目没给模态 → 旧的要留下
            tools: None,
            reasoning: Some(false),
            efforts: Some(BTreeMap::from([("low".to_string(), "low".to_string())])),
            last_updated: Some("2026-09-01".into()),
        };
        let merged = merge_entries(newer, older);
        assert_eq!(merged.modalities.unwrap(), vec!["text", "image"]);
        assert_eq!(merged.tools, Some(true), "旧声明的工具能力不该被挤掉");
        assert_eq!(merged.reasoning, Some(false));
        assert!(merged.efforts.is_none(), "reasoning=false 时不留档位");
        assert_eq!(merged.last_updated.as_deref(), Some("2026-09-01"));
    }

    #[test]
    fn 解析落盘_两种顶层形状都认_并且不填方言() {
        let dir = std::env::temp_dir().join(format!(
            "comfyhub-capability-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // 形状一：顶层就是 provider map
        let flat = r#"{"openai":{"models":{"gpt-5":{"id":"gpt-5","modalities":{"input":["text","image"]},"tool_call":true,"reasoning":true,"last_updated":"2026-09-01"}}}}"#;
        assert_eq!(parse_and_write(flat, &dir, "2026-09-27T00:00:00Z").unwrap(), 1);
        let written = std::fs::read_to_string(dir.join(REMOTE_FILE)).unwrap();
        let parsed: OverlayFile = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed.source, SOURCE);
        assert_eq!(parsed.fetched_at.as_deref(), Some("2026-09-27T00:00:00Z"));
        let entry = parsed.entries.get("gpt-5").unwrap();
        assert_eq!(entry.thinking_format, None, "方言永远不填（猜错比不填更糟）");
        assert_eq!(entry.tools, Some(true));

        // 形状二：包一层 `models`
        let nested = r#"{"models":{"anthropic":{"models":{"claude":{"modalities":{"input":["text"]},"tool_call":false}}}}}"#;
        assert_eq!(parse_and_write(nested, &dir, "2026-09-27T01:00:00Z").unwrap(), 1);

        // 解析不出任何模型 → 报错，而且**旧文件还在**（上面那份没被弄坏）
        let before = std::fs::read_to_string(dir.join(REMOTE_FILE)).unwrap();
        let err = parse_and_write(r#"{"hello":"world"}"#, &dir, "x").unwrap_err();
        assert!(err.contains("没有解析出任何模型"), "{err}");
        assert_eq!(std::fs::read_to_string(dir.join(REMOTE_FILE)).unwrap(), before);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
