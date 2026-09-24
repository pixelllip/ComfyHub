//! `/api/ai/*` 的第一批（P4 第 16 步）—— 起手是**只读的 provider 清单**。
//!
//! 为什么从它开始（总纲 §9 ③ 的顺序）：界面打开 AI 工作台第一件事就是拉它，
//! 而它**不含任何密钥**（AIH-012）—— 只有"这个 ref 配没配"这一个事实。
//!
//! # 两个最容易做错的地方（都会**静默**）
//!
//! 1. **键集合**：`AiProviderDto` 有 11 个字段，其中 `credential`（对象）与
//!    `createdAt` / `updatedAt` 是最容易漏的三个 —— Kotlin 的 `Json.kt` 里
//!    `encodeDefaults = true`，所以**它们一定会被序列化**。"少一个键 = 界面上少一块内容"
//!    是这个仓库栽过好几次的那类故障。
//! 2. **凭据状态的三分支**（`CredentialService.describe`）：
//!    ref 为空 → `(false, "none", false)`；环境变量命中（`<ref>` 或 `COMFYHUB_CRED_<ref>`）
//!    → `(值非空, "env", **false**)`；否则看托管凭据文件里有没有这个 ref
//!    → `(有, "managed" / 没有 "none", **true**)`。
//!    ⚠ `source = "env"` 时 `writable = false`（环境变量里的 key 不该由界面改，AIH-013）；
//!    `none` 时 `writable = true`（界面可以写一把新的）。
//!
//! 托管凭据文件是 `<storage>\ai\credentials.dpapi.json`，**值由 DPAPI 加密**。
//! 这里只读**键名**（`configured` / `source` 只需"有没有这个 ref"）——
//! **不解密，也不该解密**；写成"永远 false"则是**假装没配过**，那属于红线 R13。

use serde::Serialize;

use crate::routes::{ApiError, AppState};

/// provider 不存在时 Kotlin 抛的那个错（`AiRoutes.requireProvider`）。
///
/// ⚠ 是 `AiException(CONFIG_ERROR, …)` 而**不是** 404：信封是
/// `{error:"CONFIG_ERROR", detail:"Provider 不存在: xx"}`（400）。
/// 写成 `404 {"error":"Provider 不存在"}` 的话，前端按 `error` 分支会匹配不上任何已知码，
/// 而 `detail` 里又没有人话可显示 —— 正是 Task 1 修掉的那类"界面显示机器码 / 一片空白"。
fn provider_missing(id: &str) -> ApiError {
    ApiError::ai("CONFIG_ERROR", format!("Provider 不存在: {id}"))
}

/// 凭据状态（只有"有没有 / 从哪来 / 能不能改"，**没有值**）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatus {
    pub configured: bool,
    /// env / managed / none
    pub source: String,
    /// 前端是否可写；env 来源只读（AIH-013）
    pub writable: bool,
}

impl CredentialStatus {
    fn none_readonly() -> Self {
        Self { configured: false, source: "none".into(), writable: false }
    }
}

/// provider 清单里的一行（与 Kotlin `AiProviderDto` 逐字同形）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderDto {
    pub id: String,
    pub display_name: String,
    pub api: String,
    /// ⚠ 键名是 **`baseURL`**（Kotlin 的 `AiProviderDto.baseURL` 原样序列化）——
    /// `rename_all = "camelCase"` 会把它变成 `baseUrl`，**差一个字母界面就读不到**。
    /// 这条就是这么被自己的用例抓出来的（断言精确键集合就是为了它）。
    #[serde(rename = "baseURL")]
    pub base_url: String,
    /// Kotlin 是 `credentialRef: String? = null` → 为 null 时**键不出现**（`explicitNulls = false`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
    pub endpoint_trust: String,
    pub enabled: bool,
    pub revision: i64,
    /// 只写凭据的状态，**绝不包含值**（AIH-012）
    pub credential: CredentialStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// 环境变量名的前缀（`CredentialService.ENV_PREFIX`，**改了要同时改两边**）。
const ENV_PREFIX: &str = "COMFYHUB_CRED_";

/// 凭据状态的判定 —— **纯逻辑 + 注入两个查询**，这样三分支都能单测。
///
/// * `env_of(name)`：取环境变量（`None` = 没这个变量，`Some("")` = 有但是空的）；
/// * `stored_has(ref)`：托管凭据文件里有没有这个 ref（**只问键名，不解密**）。
pub fn describe_credential(
    reference: Option<&str>,
    env_of: impl Fn(&str) -> Option<String>,
    stored_has: impl Fn(&str) -> bool,
) -> CredentialStatus {
    let Some(reference) = reference.map(str::trim).filter(|r| !r.is_empty()) else {
        return CredentialStatus::none_readonly();
    };
    // 两个名字都要认：`<ref>` 与 `COMFYHUB_CRED_<ref>`
    let from_env = env_of(reference).or_else(|| env_of(&format!("{ENV_PREFIX}{reference}")));
    if let Some(value) = from_env {
        // ⚠ 环境变量那一档 `writable=false`：界面不该改一个由环境变量给的 key
        return CredentialStatus { configured: !value.is_empty(), source: "env".into(), writable: false };
    }
    let stored = stored_has(reference);
    CredentialStatus {
        configured: stored,
        source: if stored { "managed".into() } else { "none".into() },
        // 走到这里说明没有环境变量挡着：界面可以写（也为"清掉一把配错的 key"留着）
        writable: true,
    }
}

/// `GET /api/ai/providers`（不含密钥）。
pub fn list_providers(state: &AppState) -> Result<Vec<AiProviderDto>, ApiError> {
    let rows: Vec<AiProviderRow> = state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, display_name, api, base_url, credential_ref, endpoint_trust, enabled,
                    revision, created_at, updated_at
             FROM ai_providers ORDER BY display_name, id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(AiProviderRow {
                    id: row.get(0)?,
                    display_name: row.get(1)?,
                    api: row.get(2)?,
                    base_url: row.get(3)?,
                    credential_ref: row.get(4)?,
                    endpoint_trust: row.get(5)?,
                    enabled: row.get::<_, i64>(6)? != 0,
                    revision: row.get(7)?,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })?;

    let stored = read_credential_refs(state);
    Ok(rows
        .into_iter()
        .map(|row| {
            let credential = describe_credential(
                row.credential_ref.as_deref(),
                |name| std::env::var(name).ok(),
                |reference| stored.iter().any(|r| r == reference),
            );
            // 时间列不是透传：Kotlin 先解析再按 Java 规则重排（见 `iso_time.rs` 头注）
            let row_created_at = row.created_at.clone();
            let row_updated_at = row.updated_at.clone();
            AiProviderDto {
                id: row.id,
                display_name: row.display_name,
                api: row.api,
                base_url: row.base_url,
                credential_ref: row.credential_ref,
                endpoint_trust: row.endpoint_trust,
                enabled: row.enabled,
                revision: row.revision,
                credential,
                created_at: crate::iso_time::iso_time(row_created_at.as_deref()),
                updated_at: crate::iso_time::iso_time(row_updated_at.as_deref()),
            }
        })
        .collect())
}

struct AiProviderRow {
    id: String,
    display_name: String,
    api: String,
    base_url: String,
    credential_ref: Option<String>,
    endpoint_trust: String,
    enabled: bool,
    revision: i64,
    created_at: Option<String>,
    updated_at: Option<String>,
}

/// 托管凭据文件里的**键名**（`<storage>\ai\credentials.dpapi.json`）。
///
/// 只读键名：值是用 DPAPI 加密的，**不归我们解**（也不该解）。
/// 文件不存在 / 读坏了一律当"没有"—— 那正是"没配过"的意思，不能因此让整个端点 500。
fn read_credential_refs(state: &AppState) -> Vec<String> {
    let file = credential_store_path(state);
    let Ok(text) = std::fs::read_to_string(&file) else { return Vec::new() };
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    map.keys().cloned().collect()
}

/// `<storage>\ai\credentials.dpapi.json`（`media_dir = <storage>\media`）。
fn credential_store_path(state: &AppState) -> std::path::PathBuf {
    let storage = state.media_dir().parent().unwrap_or(state.media_dir());
    storage.join("ai").join("credentials.dpapi.json")
}

// ---------------------------------------------------------------------------
//  模型目录（P4 第 20 步 = Task 2）
// ---------------------------------------------------------------------------

/// 模型目录里的一行（与 Kotlin `AiModelDto` 逐字同形）。
///
/// # 键集合的实测基线（**不是照 DTO 猜的**）
///
/// 真库副本上 Kotlin（8090）`GET /api/ai/providers/{id}/models` 实测：
/// 81 条 `command-code-goat` 模型 + 2 条 `deepseek` 模型里 ——
///
/// | 键 | 出现情况 |
/// | --- | --- |
/// | `providerId` `id` `displayName` `inputModalities` `attachmentTransports` `mimeAllowlist` `tools` `parallelTools` `reasoning` `thinkingEfforts` `capabilitySource` `enabled` | **永远在**（Kotlin 里非空 + 默认值，`encodeDefaults = true`） |
/// | `thinkingFormat` | 81 条里 **10 条没有这个键** |
/// | `contextWindow` | 时有时无（`deepseek-v4-pro` 就没有） |
/// | `maxOutputTokens` / `maxAttachmentBytes` / `maxAttachmentCount` / `capabilityVerifiedAt` | 本机真库 **81/81 全都没有**（NULL → `explicitNulls = false` → 键不出现） |
///
/// 所以：**前 12 个键一个都不能少，后 6 个一个都不能自己冒出来**。
/// 少一个键 = 界面上少一块内容（附件校验、思考强度选择器全靠它们）；
/// 多一个 `null` 键 = 与 Kotlin 形状不一致（第 18 步那条红线）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiModelDto {
    pub provider_id: String,
    /// ⚠ 是 `model_id` 列，**不是** `id`（`ai_models` 的主键是 `(provider_id, model_id)`）。
    pub id: String,
    pub display_name: String,
    pub input_modalities: Vec<String>,
    /// 键是模态、值是传输方式（`AttachmentProtocol` 的唯一真源，AGENTS §10）
    pub attachment_transports: std::collections::BTreeMap<String, Vec<String>>,
    pub mime_allowlist: Vec<String>,
    pub tools: bool,
    pub parallel_tools: bool,
    pub reasoning: bool,
    /// 等级 → 过线拼写（AIH-056）；空表 UI 上就不显示强度选择器
    pub thinking_efforts: std::collections::BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_attachment_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_attachment_count: Option<i64>,
    pub capability_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_verified_at: Option<String>,
    pub enabled: bool,
}

/// `GET /api/ai/providers/{id}/models`。
///
/// 顺序照抄 Kotlin：`ORDER BY display_name, model_id`（**默认 BINARY 排序，没有 COLLATE NOCASE**
/// —— 顺手加一个"更合理"的 NOCASE 就会让两端顺序不同，而顺序不同在界面上就是"列表位置变了"）。
/// provider 不存在时**先**报错（与 Kotlin 的 `requireProvider` 同一个位置、同一个信封）。
pub fn list_models(state: &AppState, provider_id: &str) -> Result<Vec<AiModelDto>, ApiError> {
    if !provider_exists(state, provider_id)? {
        return Err(provider_missing(provider_id));
    }
    state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT * FROM ai_models WHERE provider_id = ?1 ORDER BY display_name, model_id",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![provider_id], model_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// provider 存不存在 —— 与 `list_models` **分成两件事**：
/// "provider 不存在"（400 CONFIG_ERROR）和"这个 provider 一个模型都没有"（200 `[]`）
/// 是完全不同的语义，混成一个"空数组"会让界面以为配置读到了、只是模型列表为空。
fn provider_exists(state: &AppState, provider_id: &str) -> Result<bool, ApiError> {
    state.with(|conn| {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_providers WHERE id = ?1",
            rusqlite::params![provider_id],
            |row| row.get(0),
        )?;
        Ok(n > 0)
    })
}

/// 一行 `ai_models` → DTO（列名按 `db/schema.sqlite.sql`）。
pub fn model_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AiModelDto> {
    let model_id: String = row.get("model_id")?;
    // `display_name` 为 NULL 时回落成 model_id（Kotlin `toModel` 就是这么写的）——
    // 回落成空串会让列表里出现一行没有名字的模型
    let display_name: Option<String> = row.get("display_name")?;
    let capability_source: Option<String> = row.get("capability_source")?;
    let thinking_format: Option<String> = row.get("thinking_format")?;
    Ok(AiModelDto {
        provider_id: row.get("provider_id")?,
        id: model_id.clone(),
        // NULL → 回落成 model_id（Kotlin `toModel` 就是 `?: getString("model_id")`）。
        // 本 schema 里 `display_name` 是 NOT NULL，所以这条路只在**老 MySQL 库搬过来的脏行**
        // 上才走得到 —— 但"回落"与"回落到空串"是两回事：列表里出现一行没有名字的模型，
        // 用户既点不出所以然，也搜不到。
        display_name: display_name.unwrap_or(model_id),
        input_modalities: parse_string_array(row.get::<_, Option<String>>("input_modalities")?.as_deref()),
        attachment_transports: parse_string_list_map(
            row.get::<_, Option<String>>("attachment_transports")?.as_deref(),
        ),
        mime_allowlist: parse_string_array(row.get::<_, Option<String>>("mime_allowlist")?.as_deref()),
        tools: row.get::<_, Option<i64>>("tools")?.unwrap_or(0) != 0,
        parallel_tools: row.get::<_, Option<i64>>("parallel_tools")?.unwrap_or(0) != 0,
        reasoning: row.get::<_, Option<i64>>("reasoning")?.unwrap_or(0) != 0,
        thinking_efforts: parse_string_map(
            row.get::<_, Option<String>>("thinking_efforts")?.as_deref(),
        ),
        // 空白串也算"没声明"（Kotlin `?.takeIf { it.isNotBlank() }`）
        thinking_format: thinking_format.filter(|s| !s.trim().is_empty()),
        context_window: row.get("context_window")?,
        max_output_tokens: row.get("max_output_tokens")?,
        max_attachment_bytes: row.get("max_attachment_bytes")?,
        max_attachment_count: row.get("max_attachment_count")?,
        capability_source: capability_source.unwrap_or_else(|| "manual".into()),
        capability_verified_at: crate::iso_time::iso_time(
            row.get::<_, Option<String>>("capability_verified_at")?.as_deref(),
        ),
        // ⚠ NULL → **false**（Kotlin 的裸 `getBoolean("enabled")` 在 NULL 上就给 false）。
        // 本 schema 里是 NOT NULL DEFAULT 1，所以只在脏行上才走得到 ——
        // 但"NULL 当成启用"会让一条本该被用户关掉的模型悄悄可用（那是能花钱的）。
        enabled: row.get::<_, Option<i64>>("enabled")?.unwrap_or(0) != 0,
    })
}

/// `["text","image"]` → `Vec<String>`；**坏数据不能让整个设置页打不开**（Kotlin 的纪律）。
///
/// 与 Kotlin `parseStringArray` 同口径：数组里的标量按 `contentOrNull` 取文本
/// （数字 → `"4096"`、布尔 → `"true"`），**JSON null 与对象/数组元素丢掉**。
fn parse_string_array(raw: Option<&str>) -> Vec<String> {
    let Some(text) = raw.map(str::trim).filter(|s| !s.is_empty()) else { return Vec::new() };
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Array(items)) => items.iter().filter_map(scalar_text).collect(),
        _ => Vec::new(),
    }
}

/// `{"image":["inline_base64"]}` → map；值不是数组时按空数组处理（与 Kotlin 同）。
fn parse_string_list_map(raw: Option<&str>) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut out = std::collections::BTreeMap::new();
    let Some(text) = raw.map(str::trim).filter(|s| !s.is_empty()) else { return out };
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(text) {
        for (key, value) in map {
            let list = match value {
                serde_json::Value::Array(items) => items.iter().filter_map(scalar_text).collect(),
                _ => Vec::new(),
            };
            out.insert(key, list);
        }
    }
    out
}

/// `{"high":"high","max":4096}` → map（**数字按字面文本存**，AIH-056）。
fn parse_string_map(raw: Option<&str>) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let Some(text) = raw.map(str::trim).filter(|s| !s.is_empty()) else { return out };
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(text) {
        for (key, value) in map {
            if let Some(text) = scalar_text(&value) {
                out.insert(key, text);
            }
        }
    }
    out
}

/// JSON 标量 → 文本（Kotlin `JsonPrimitive.contentOrNull` 的对应物）。
///
/// `null` / 对象 / 数组一律 `None` —— 那正是 Kotlin 那边 `mapNotNull` 会丢掉的东西。
fn scalar_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn 凭据状态的三个分支一条都不能错() {
        // ① ref 为空：不配置、不可写（连"能不能写"都是 false：没地方写）
        let s = describe_credential(None, no_env, |_| true);
        assert_eq!(s, CredentialStatus { configured: false, source: "none".into(), writable: false });
        let s = describe_credential(Some("   "), no_env, |_| true);
        assert_eq!(s.source, "none", "空白 ref 与没有 ref 同义");

        // ② 环境变量命中 → source=env 且 **writable=false**（AIH-013：界面不许改环境变量给的 key）
        let s = describe_credential(Some("OPENAI_KEY"), |n| (n == "OPENAI_KEY").then(|| "sk-x".into()), |_| false);
        assert_eq!(s, CredentialStatus { configured: true, source: "env".into(), writable: false });
        // 空串的环境变量 = 配了但没值
        let s = describe_credential(Some("OPENAI_KEY"), |_| Some(String::new()), |_| false);
        assert_eq!(s, CredentialStatus { configured: false, source: "env".into(), writable: false });

        // ③ 前缀那个名字也要认（`COMFYHUB_CRED_<ref>`）——认错了就等于"永远读不到那把 key"
        let s = describe_credential(Some("OPENAI_KEY"), |n| (n == "COMFYHUB_CRED_OPENAI_KEY").then(|| "sk-y".into()), |_| false);
        assert_eq!(s.source, "env");
        assert!(s.configured);

        // ④ 托管文件里有没有 → managed / none；**两种都可写**（界面要能改、也要能清一把配错的）
        let s = describe_credential(Some("OPENAI_KEY"), no_env, |_| true);
        assert_eq!(s, CredentialStatus { configured: true, source: "managed".into(), writable: true });
        let s = describe_credential(Some("OPENAI_KEY"), no_env, |_| false);
        assert_eq!(s, CredentialStatus { configured: false, source: "none".into(), writable: true });

        // ⑤ 环境变量优先于托管文件（与 Kotlin 的判定顺序一致）
        let s = describe_credential(Some("K"), |_| Some("v".into()), |_| true);
        assert_eq!(s.source, "env", "环境变量先判");
    }

    // -----------------------------------------------------------------------
    //  模型目录：JSON 列的宽松解析（坏数据不许让整页打不开）
    // -----------------------------------------------------------------------

    #[test]
    fn 模型目录的_json_列解析不炸也不悄悄改口径() {
        // 正常值
        assert_eq!(parse_string_array(Some(r#"["text","image"]"#)), vec!["text", "image"]);
        // 空 / 缺 / 坏 JSON / 类型不对 → 一律空表（Kotlin 那边是 runCatching + getOrDefault）
        for bad in [None, Some(""), Some("   "), Some("not json"), Some("{}"), Some("null")] {
            assert!(parse_string_array(bad).is_empty(), "{bad:?} 应当解析成空表");
        }
        // 数字 / 布尔按**字面文本**取（Kotlin `contentOrNull` 就是这么给的）
        assert_eq!(parse_string_array(Some(r#"[1,true]"#)), vec!["1", "true"]);
        // JSON null 元素丢掉
        assert_eq!(parse_string_array(Some(r#"["a",null]"#)), vec!["a"]);

        // 传输方式表：键是模态、值是数组；值不是数组 → 空数组（**不是**丢掉整个键）
        let map = parse_string_list_map(Some(r#"{"image":["inline_base64"],"audio":"x"}"#));
        assert_eq!(map.get("image").unwrap(), &vec!["inline_base64".to_string()]);
        assert_eq!(map.get("audio").unwrap(), &Vec::<String>::new());
        assert!(parse_string_list_map(Some("[1,2]")).is_empty());

        // 思考等级表：值可能是数字（Anthropic 的预算）→ 按字面文本；null 的键丢掉
        let efforts = parse_string_map(Some(r#"{"max":4096,"low":"low","off":null}"#));
        assert_eq!(efforts.get("max").unwrap(), "4096");
        assert_eq!(efforts.get("low").unwrap(), "low");
        assert!(!efforts.contains_key("off"), "JSON null 不该留下一个空值的等级");
    }

    /// `display_name` / `capability_source` 为 NULL 时的回落 —— 直接对着**计算出的行**测，
    /// 因为真 schema 上这两列都是 NOT NULL，夹具里构造不出来（只有老 MySQL 脏行才会撞上）。
    #[test]
    fn 可空的显示名与能力来源都有回落值() {
        let conn = crate::db::open_memory().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT 'p' AS provider_id, 'm-x' AS model_id, NULL AS display_name,
                        NULL AS input_modalities, NULL AS attachment_transports,
                        NULL AS mime_allowlist, NULL AS tools, NULL AS parallel_tools,
                        NULL AS reasoning, NULL AS thinking_efforts, '  ' AS thinking_format,
                        NULL AS context_window, NULL AS max_output_tokens,
                        NULL AS max_attachment_bytes, NULL AS max_attachment_count,
                        NULL AS capability_source, NULL AS capability_verified_at, NULL AS enabled",
            )
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let model = model_from_row(row).unwrap();

        assert_eq!(model.display_name, "m-x", "显示名空着就回落成 model_id");
        assert_eq!(model.capability_source, "manual");
        assert!(model.thinking_format.is_none(), "空白串等于没声明");
        assert!(model.context_window.is_none());
        // 布尔列 NULL → false（`getBoolean` + `wasNull` 在 Kotlin 那边给的是默认值）
        assert!(!model.tools && !model.enabled);
        // 序列化出来的形状：可空键一个都不出现
        let value = serde_json::to_value(&model).unwrap();
        let mut got: Vec<&str> = value.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                "attachmentTransports", "capabilitySource", "displayName", "enabled", "id",
                "inputModalities", "mimeAllowlist", "parallelTools", "providerId", "reasoning",
                "thinkingEfforts", "tools",
            ]
        );
    }
}
