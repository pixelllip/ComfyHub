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

use serde::{Deserialize, Serialize};

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
pub(crate) const ENV_PREFIX: &str = "COMFYHUB_CRED_";

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
    Ok(rows.into_iter().map(|row| row.into_dto(&stored)).collect())
}

pub struct AiProviderRow {
    pub id: String,
    pub display_name: String,
    pub api: String,
    pub base_url: String,
    pub credential_ref: Option<String>,
    pub endpoint_trust: String,
    pub enabled: bool,
    pub revision: i64,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// 查一条 provider（`AiRepo.getProvider` 的对应物）。`None` = 没有这个 id。
///
/// 为什么单列一条：**凭据端点与写侧都要"先确认这个 provider 在不在"**，
/// 而 `list_providers` 那条路会顺手把整表读出来（写侧用它做存在性判断就会
/// "为了问一句在不在，把 80 个模型的行全拉一遍"）。
pub fn get_provider_row(state: &AppState, id: &str) -> Result<Option<AiProviderRow>, ApiError> {
    state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, display_name, api, base_url, credential_ref, endpoint_trust, enabled,
                    revision, created_at, updated_at
             FROM ai_providers WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], |row| {
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
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    })
}

/// `AiRoutes.requireProvider` 的逐字对应物：不存在 → **400 `CONFIG_ERROR` + `detail`**。
///
/// ⚠ 是 `AiException(CONFIG_ERROR, …)` 而**不是** 404 —— Kotlin 那边就是这么抛的，
/// 写成 404 就没有 `detail`，界面上又一次显示不出人话（P4 第 17 步那条对齐）。
pub fn require_provider_row(state: &AppState, id: &str) -> Result<AiProviderRow, ApiError> {
    let trimmed = id.trim();
    get_provider_row(state, trimmed)?.ok_or_else(|| provider_missing(trimmed))
}

impl AiProviderRow {
    /// 行 → DTO（`credential` 是**只写凭据的状态**，绝不带值）。
    pub fn into_dto(self, stored_refs: &[String]) -> AiProviderDto {
        let credential = describe_credential(
            self.credential_ref.as_deref(),
            |name| std::env::var(name).ok(),
            |reference| stored_refs.iter().any(|r| r == reference),
        );
        // 时间列不是透传：Kotlin 先解析再按 Java 规则重排（见 `iso_time.rs` 头注）
        let created_at = crate::iso_time::iso_time(self.created_at.as_deref());
        let updated_at = crate::iso_time::iso_time(self.updated_at.as_deref());
        AiProviderDto {
            id: self.id,
            display_name: self.display_name,
            api: self.api,
            base_url: self.base_url,
            credential_ref: self.credential_ref,
            endpoint_trust: self.endpoint_trust,
            enabled: self.enabled,
            revision: self.revision,
            credential,
            created_at,
            updated_at,
        }
    }
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
///
/// `pub(crate)`：`auth.rs` 的**密钥下行**要读同一份文件（那边会真的解 DPAPI）。
pub(crate) fn credential_store_path(state: &AppState) -> std::path::PathBuf {
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
/// ⚠ **读写共用一个形状**（`Deserialize` 也是它）：`PUT /providers/{id}/models` 的 body
/// 就是界面从 GET 拿到的那份（原样改几个字段再传回来）。
/// 所以两份 DTO 是**同一个结构体**，不是"照着再写一个入参"——
/// 后者迟早会出现"GET 给的键 PUT 不认"（用户改了名字却存不进去）。
///
/// 可空键与集合键都要 `default`：界面只传改过的字段时，缺的键按 Kotlin 的默认值走
/// （`inputModalities = emptyList()` / `capabilitySource = "manual"` / `enabled = true`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiModelDto {
    #[serde(default)]
    pub provider_id: String,
    /// ⚠ 是 `model_id` 列，**不是** `id`（`ai_models` 的主键是 `(provider_id, model_id)`）。
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub input_modalities: Vec<String>,
    /// 键是模态、值是传输方式（`AttachmentProtocol` 的唯一真源，AGENTS §10）
    #[serde(default)]
    pub attachment_transports: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub mime_allowlist: Vec<String>,
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub parallel_tools: bool,
    #[serde(default)]
    pub reasoning: bool,
    /// 等级 → 过线拼写（AIH-056）；空表 UI 上就不显示强度选择器
    #[serde(default)]
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
    #[serde(default = "default_capability_source")]
    pub capability_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_verified_at: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_capability_source() -> String {
    "manual".to_string()
}

fn default_true() -> bool {
    true
}

/// `AiModelUpsert`：`{models:[…]}`。
#[derive(Debug, Deserialize)]
pub struct AiModelUpsert {
    #[serde(default)]
    pub models: Vec<AiModelDto>,
}

/// `AiProviderUpsert`（创建与更新**共用**一个形状，与 Kotlin 同）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderUpsert {
    #[serde(default)]
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub api: Option<String>,
    /// ⚠ 键名是 `baseURL`（Kotlin 的字段就叫这个，`rename_all` 会把它变成 `baseUrl`）
    #[serde(rename = "baseURL")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub credential_ref: Option<String>,
    #[serde(default)]
    pub endpoint_trust: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub revision: Option<i64>,
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

/// `thinking_efforts` 那一列 → 等级表（`run_lifecycle` 起 Run 时要复核请求的强度）。
///
/// 一行 `ai_models` 的 `input_modalities`（附件准入与历史内联都要它）。
///
/// 与 [`model_from_row`] 走**同一个** `parse_string_array`：两处各写一份解析，
/// 迟早出现"目录页说这模型能吃图、起 Run 时说它不能"这种自相矛盾。
pub fn model_input_modalities_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<Vec<String>> {
    Ok(parse_string_array(row.get::<_, Option<String>>("input_modalities")?.as_deref()))
}

/// 与 [`model_from_row`] 走**同一个**解析函数：两处各写一份 `parse_string_map` 调用，
/// 迟早会出现"目录页显示有的等级、起 Run 时说没声明"这种自相矛盾。
pub fn thinking_efforts_of(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<std::collections::BTreeMap<String, String>> {
    Ok(parse_string_map(row.get::<_, Option<String>>("thinking_efforts")?.as_deref()))
}

/// 一行 `ai_models` → DTO（列名按 `db/schema.sqlite.sql`）。
pub fn model_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AiModelDto> {    let model_id: String = row.get("model_id")?;
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

// ---------------------------------------------------------------------------
//  校验（`AiValidation` 的逐条对照）
// ---------------------------------------------------------------------------

/// provider id：小写 kebab-case、≤96（AIH-007 的"创建后不可修改"靠它 + 路由层判重）。
///
/// ⚠ 三条判定**顺序与文案**都照抄 Kotlin：空 → 太长 → 形态。
/// 把"形态"提到"太长"前面的话，一个 100 字符的合法 kebab-case 会收到"只能是小写 kebab-case"，
/// 而它其实是合法的 —— 用户会去改一个根本没错的地方。
pub fn require_provider_id(raw: Option<&str>) -> Result<String, ApiError> {
    let id = raw.unwrap_or("").trim().to_string();
    if id.is_empty() {
        return Err(ApiError::ai("CONFIG_ERROR", "Provider ID 不能为空"));
    }
    if id.chars().count() > 96 {
        return Err(ApiError::ai("CONFIG_ERROR", "Provider ID 不能超过 96 个字符"));
    }
    if !is_kebab(&id) {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            "Provider ID 只能是小写 kebab-case（如 my-gateway），且创建后不可修改",
        ));
    }
    Ok(id)
}

/// `^[a-z0-9]+(-[a-z0-9]+)*$`（**ASCII** —— 不能用 `char::is_alphanumeric`，那会认中文）。
fn is_kebab(text: &str) -> bool {
    let mut last_dash = true;
    let mut any = false;
    for ch in text.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            last_dash = false;
            any = true;
        } else if ch == '-' && !last_dash {
            last_dash = true;
        } else {
            return false;
        }
    }
    any && !last_dash
}

/// 显示名：非空、≤128。
pub fn require_display_name(raw: Option<&str>) -> Result<String, ApiError> {
    let name = raw.unwrap_or("").trim().to_string();
    if name.is_empty() {
        return Err(ApiError::ai("CONFIG_ERROR", "显示名不能为空"));
    }
    if name.chars().count() > 128 {
        return Err(ApiError::ai("CONFIG_ERROR", "显示名不能超过 128 个字符"));
    }
    Ok(name)
}

/// 凭据引用名：空 = 不配置（`None`）；否则 `^[A-Z][A-Z0-9_]{0,127}$`。
pub fn require_credential_ref(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let reference = raw.unwrap_or("").trim().to_string();
    if reference.is_empty() {
        return Ok(None);
    }
    let bytes = reference.as_bytes();
    let ok = bytes.len() <= 128
        && bytes[0].is_ascii_uppercase()
        && bytes.iter().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_');
    if !ok {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            "凭据引用名只能是大写字母、数字和下划线，且以字母开头",
        ));
    }
    Ok(Some(reference))
}

/// 模型 ID：只要求非空、无控制字符、≤191。**不根据 ID 猜能力**（AIH-011）。
pub fn require_model_id(raw: &str) -> Result<String, ApiError> {
    let id = raw.trim().to_string();
    if id.is_empty() {
        return Err(ApiError::ai("CONFIG_ERROR", "模型 ID 不能为空"));
    }
    if id.chars().count() > 191 {
        return Err(ApiError::ai("CONFIG_ERROR", "模型 ID 不能超过 191 个字符"));
    }
    if id.chars().any(|c| c.is_control()) {
        return Err(ApiError::ai("CONFIG_ERROR", "模型 ID 不能包含控制字符"));
    }
    Ok(id)
}

/// 校验并规范化 Base URL：**只去掉末尾的 `/`**，不擅自改写用户路径
/// （Anthropic 兼容网关的路径经常不是 `/v1`）。
///
/// # 为什么不用 `url::Url` 当裁判（**实测出来的四处不一致**）
///
/// Kotlin 用的是 Java `java.net.URI`，它的严格程度与 `url` crate 差得不小。
/// 2026-09-25 拿真 Kotlin 基线（8098）逐条问出来一张真值表，下面这四处**必须自己判**：
///
/// | Base URL | Java `URI` | `url` crate 单独用会怎样 |
/// | --- | --- | --- |
/// | `https://api.example.com:99999/x` | **通过**（不校验端口范围） | `Err(InvalidPort)` |
/// | `https://api.example.com:-1/x` | 通过但 **host 为 null** → "缺少主机名" | `Err(InvalidPort)` |
/// | `http://` | **抛异常** → "不是合法 URL" | `Err(EmptyHost)` |
/// | `http:///x` / `http:/foo` | 通过但 host 为 null → "缺少主机名" | `Ok`（会把 `x`/`foo` 当主机！） |
/// | `https://api.example.com/%zz` | **抛异常**（非法转义） | `Ok`（原样留着） |
/// | `https://api.example.com/a b`（空格 `<` `>` `"` `{` `}` `\|` `\` `^` `` ` `` `[` `]` 同理） | **抛异常** | `Ok`（悄悄百分号编码） |
/// | `https://api.example.com/路径` | **通过**（非 ASCII 是允许的） | `Ok` |
///
/// 也就是说**光靠 `url` crate 会放过一批 Java 拒绝的串，还会拒掉一批 Java 接受的串** ——
/// 前者是"用户填了个我们发不出去的地址却存进了库"，后者是"用户填对了却存不进去"。
/// 所以这里按 Java 的规则**自己解析**（纯函数、逐条对着真值表有用例）。
///
/// ⚠ 返回的是**原文 trim 过**（去掉末尾 `/`），不是解析器重排后的 URL：
/// `url` crate 会补 `/`、小写化 host、百分号编码，拿它当返回值就会**悄悄改写用户的路径**。
pub fn normalize_base_url(raw: Option<&str>, trust: &str) -> Result<String, ApiError> {
    let text = raw.unwrap_or("").trim().to_string();
    if text.is_empty() {
        return Err(ApiError::ai("CONFIG_ERROR", "Base URL 不能为空"));
    }
    if text.encode_utf16().count() > 1024 {
        return Err(ApiError::ai("CONFIG_ERROR", "Base URL 过长"));
    }
    // ① Java `URI(text)` 的**语法准入**（不合法一律"不是合法 URL"，与"缺主机名"分开）
    if !uri_syntax_ok(&text) {
        return Err(ApiError::ai("CONFIG_ERROR", "Base URL 不是合法 URL"));
    }
    // ② scheme：`//api.example.com` 这种没有 scheme 的也要走到这里（Java 的 scheme 是 null）
    let Some(colon) = text.find(':') else {
        return Err(ApiError::ai("CONFIG_ERROR", "Base URL 只支持 http/https"));
    };
    let scheme = text[..colon].to_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(ApiError::ai("CONFIG_ERROR", "Base URL 只支持 http/https"));
    }
    // ③ authority：Java 里 `http:/foo` / `http:foo` 解析得出来但 host 为 null
    let rest = &text[colon + 1..];
    if !rest.starts_with("//") {
        return Err(ApiError::ai("CONFIG_ERROR", "Base URL 缺少主机名"));
    }
    // `http://` 恰好是 Java 抛 "Expected authority" 的那个形状
    if rest.len() == 2 {
        return Err(ApiError::ai("CONFIG_ERROR", "Base URL 不是合法 URL"));
    }
    let after = &rest[2..];
    let authority_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = &after[..authority_end];
    let (user_info, host_port) = match authority.rfind('@') {
        Some(at) => (Some(&authority[..at]), &authority[at + 1..]),
        None => (None, authority),
    };
    let host = match parse_authority_host(host_port) {
        Ok(host) => host,
        // 端口不是数字 / 主机为空 → Java 的 "registry-based authority"（host 为 null）
        Err(HostError::Missing) => return Err(ApiError::ai("CONFIG_ERROR", "Base URL 缺少主机名")),
        // IPv6 字面量写坏了 → Java 在 URI 解析阶段就抛
        Err(HostError::Syntax) => return Err(ApiError::ai("CONFIG_ERROR", "Base URL 不是合法 URL")),
    };
    if user_info.is_some() {
        return Err(ApiError::ai("CONFIG_ERROR", "Base URL 不能包含用户名密码，请使用凭据配置"));
    }
    // Java 的 `getQuery()` / `getFragment()`：`?` / `#` 后面为空串也算"有"（不是 null）
    if text.contains('?') {
        return Err(ApiError::ai("CONFIG_ERROR", "Base URL 不能带查询参数"));
    }
    if text.contains('#') {
        return Err(ApiError::ai("CONFIG_ERROR", "Base URL 不能带 fragment"));
    }
    if trust == "public" && scheme != "https" {
        return Err(ApiError::ai("CONFIG_ERROR", "公网端点必须使用 https"));
    }
    crate::endpoint_guard::assert_allowed(&host, trust).map_err(|e| ApiError::ai("CONFIG_ERROR", e))?;
    Ok(text.trim_end_matches('/').to_string())
}

/// authority 里主机那半边的解析结果。
enum HostError {
    /// 解析得出来但 host 为 null（Java 的 registry-based authority）
    Missing,
    /// IPv6 字面量语法错（Java 在 URI 解析阶段就抛）
    Syntax,
}

/// `[::1]:9` / `api.example.com:99999` / `api.example.com` → 主机名。
///
/// ⚠ **端口不校验范围**（Java `URI` 也不校验：`:99999` 照样通过），但必须是**数字**；
/// 否则整个 authority 退化成 registry-based（host = null）—— 这是 `:-1` 那条实测行为。
fn parse_authority_host(host_port: &str) -> Result<String, HostError> {
    if let Some(rest) = host_port.strip_prefix('[') {
        let Some(close) = rest.find(']') else { return Err(HostError::Syntax) };
        let literal = &rest[..close];
        if literal.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(HostError::Syntax);
        }
        match &rest[close + 1..] {
            "" => return Ok(format!("[{literal}]")),
            tail => {
                let Some(port) = tail.strip_prefix(':') else { return Err(HostError::Syntax) };
                if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(HostError::Missing);
                }
                return Ok(format!("[{literal}]"));
            }
        }
    }
    if host_port.contains(['[', ']']) {
        return Err(HostError::Syntax);
    }
    let (host, port) = match host_port.rfind(':') {
        Some(colon) => (&host_port[..colon], Some(&host_port[colon + 1..])),
        None => (host_port, None),
    };
    if host.is_empty() {
        return Err(HostError::Missing);
    }
    if let Some(port) = port {
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
            return Err(HostError::Missing);
        }
    }
    Ok(host.to_string())
}

/// Java `URI(String)` 的语法准入（实测真值表见 [`normalize_base_url`] 的注释）。
///
/// 三条规则：
/// 1. **被排除的 ASCII 字符**（空格 / 控制符 / `" < > { } | \ ^ ` `` ` ``）→ 拒；
///    **非 ASCII 一律放行**（Java 允许，实测 `.../路径` 是 201）；
/// 2. `%` 后面必须是两位十六进制（`%zz` Java 直接抛）；
/// 3. `[` `]` 只允许出现在 authority 里当 IPv6 字面量（`/a[b]` Java 抛）。
fn uri_syntax_ok(text: &str) -> bool {
    // ①② 扫描：非法字符 / 坏的百分号转义
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        let ch = bytes[i];
        if ch.is_whitespace() || ch.is_control() {
            return false;
        }
        if matches!(ch, '"' | '<' | '>' | '{' | '}' | '|' | '\\' | '^' | '`') {
            return false;
        }
        if ch == '%' {
            let hex = |c: char| c.is_ascii_hexdigit();
            if i + 2 >= bytes.len() || !hex(bytes[i + 1]) || !hex(bytes[i + 2]) {
                return false;
            }
            i += 3;
            continue;
        }
        i += 1;
    }
    // ③ 方括号：只许一对、且必须落在 authority 里（`[` `]` 出现在路径上 Java 会抛）
    let opens: Vec<usize> = text.match_indices('[').map(|(i, _)| i).collect();
    let closes: Vec<usize> = text.match_indices(']').map(|(i, _)| i).collect();
    if opens.is_empty() && closes.is_empty() {
        return true;
    }
    if opens.len() != 1 || closes.len() != 1 || closes[0] < opens[0] {
        return false;
    }
    let Some(scheme_end) = text.find("//") else { return false };
    let authority_start = scheme_end + 2;
    let authority_end = text[authority_start..]
        .find(['/', '?', '#'])
        .map(|p| authority_start + p)
        .unwrap_or(text.len());
    opens[0] >= authority_start && closes[0] <= authority_end
}

/// `hostOf(url)`：`URI(url).host`，解析不出来就是空串（`EndpointTrust.infer` 用它）。
pub fn host_of(url: &str) -> String {
    url::Url::parse(url.trim())
        .ok()
        .and_then(|parsed| parsed.host_str().map(|h| h.to_string()))
        .unwrap_or_default()
}

/// 支持的三种协议（`AiApi.wireValues`，**不含已淘汰的文本 /completions**）。
pub const API_WIRE_VALUES: [&str; 3] = ["openai-completions", "openai-responses", "anthropic-messages"];

/// 输入模态（`Modality.wireValues`）。
const MODALITIES: [&str; 5] = ["text", "image", "video", "audio", "document"];
/// 附件传输方式（`Transport.wireValues`）。
const TRANSPORTS: [&str; 3] = ["inline_base64", "url", "comfy_input"];
/// 能力来源（`AiModelDto.capabilitySource` 的四个合法值）。
const CAPABILITY_SOURCES: [&str; 4] = ["builtin", "discovered", "manual", "tested"];
/// 思考等级（`ReasoningEffort` 的**全部 7 档**，`off` 也在内 —— 少一档就等于
/// "用户在界面上选得了、存不进去"）。
const REASONING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// 保存模型目录时的逐条校验（`AiRoutes.validateModel` + `AiValidation.validateThinkingEfforts`）。
///
/// **每一档都有稳定码 + 人话**：模型目录是附件准入与思考强度的唯一真源（AIH-011/AIH-028），
/// 少校验一条，用户就能存进去一个"看着配好了、一用就 400"的模型。
pub fn validate_model(model: &AiModelDto) -> Result<(), ApiError> {
    require_model_id(&model.id)?;
    require_display_name(Some(&model.display_name))?;
    let bad: Vec<&String> = model
        .input_modalities
        .iter()
        .filter(|m| !MODALITIES.contains(&m.as_str()))
        .collect();
    if !bad.is_empty() {
        let list = bad.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
        return Err(ApiError::ai("CONFIG_ERROR", format!("未知的输入模态: {list}")));
    }
    for (key, list) in &model.attachment_transports {
        if !MODALITIES.contains(&key.as_str()) {
            return Err(ApiError::ai("CONFIG_ERROR", format!("未知的传输方式键: {key}")));
        }
        for transport in list {
            if !TRANSPORTS.contains(&transport.as_str()) {
                return Err(ApiError::ai("CONFIG_ERROR", format!("未知的传输方式: {transport}")));
            }
        }
    }
    if !CAPABILITY_SOURCES.contains(&model.capability_source.as_str()) {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            format!("非法的能力来源: {}", model.capability_source),
        ));
    }
    // 思考等级：等级必须认识、表达必须非空且不能带控制字符（`LevelSpec.of` 的三档）
    for (level, value) in &model.thinking_efforts {
        if !REASONING_LEVELS.contains(&level.as_str()) {
            return Err(ApiError::ai("CONFIG_ERROR", format!("未知的思考等级: {level}")));
        }
        let trimmed = value.trim();
        // 空 / 0 / 负数一律"表达不能为空"（Kotlin `LevelSpec.of` 就是这么判的：
        // 数字必须 > 0，否则当"没写"）；≥2^31 的数字在 Kotlin 那边会退化成 `Name` 而通过，
        // 这里同样通过（都算"能用"），差别只在归类，不在结果。
        let empty_or_nonpositive = trimmed.is_empty()
            || trimmed.parse::<i64>().map(|n| n <= 0).unwrap_or(false);
        if empty_or_nonpositive {
            return Err(ApiError::ai(
                "CONFIG_ERROR",
                format!("思考等级 {level} 的表达不能为空"),
            ));
        }
        if value.encode_utf16().count() > 64 || value.chars().any(|c| c.is_control()) {
            return Err(ApiError::ai(
                "CONFIG_ERROR",
                format!("思考等级 {level} 的表达不合法"),
            ));
        }
    }
    if let Some(format) = model.thinking_format.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        if crate::protocol::ThinkingFormat::parse(format).is_none() {
            let all: Vec<&str> = crate::protocol::ThinkingFormat::ALL.iter().map(|f| f.wire()).collect();
            return Err(ApiError::ai(
                "CONFIG_ERROR",
                format!("未知的思考方言: {format}（可选 {}）", all.join(" / ")),
            ));
        }
    }    if !model.thinking_efforts.is_empty() && !model.reasoning {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            "声明了思考等级却没勾选「支持推理」，两者必须一致",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
//  Provider 与模型目录的**写侧**（P4 第 37 步）
// ---------------------------------------------------------------------------

/// 新增一个 provider（`AiRepo.insertProvider`）：`revision` 由库给 **1**。
pub fn insert_provider(state: &AppState, dto: &AiProviderDto) -> Result<AiProviderDto, ApiError> {
    state.write(|tx| {
        tx.execute(
            "INSERT INTO ai_providers
               (id, display_name, api, base_url, credential_ref, endpoint_trust, enabled, revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
            rusqlite::params![
                dto.id,
                dto.display_name,
                dto.api,
                dto.base_url,
                dto.credential_ref,
                dto.endpoint_trust,
                dto.enabled as i64,
            ],
        )?;
        Ok(())
    })?;
    read_back_provider(state, &dto.id)
}

/// 乐观锁更新（`AiRepo.updateProvider`，AIH-007）。
///
/// 三条顺序**不能动**：先 `expected_revision > 0`（缺 revision 是"客户端没读就写"）→
/// provider 在不在 → revision 对不对。换个顺序就会把"两个窗口互相覆盖"报成"provider 不存在"。
pub fn update_provider(
    state: &AppState,
    dto: &AiProviderDto,
    expected_revision: i64,
) -> Result<AiProviderDto, ApiError> {
    if expected_revision <= 0 {
        return Err(ApiError::ai("CONFIG_ERROR", "缺少 revision，无法进行并发安全更新"));
    }
    let current = get_provider_row(state, &dto.id)?
        .ok_or_else(|| provider_missing(&dto.id))?;
    if current.revision != expected_revision {
        return Err(ApiError::ai(
            "CONFIG_ERROR",
            format!(
                "配置已被其他窗口修改（当前 revision={}，提交的={expected_revision}），请刷新后重试",
                current.revision
            ),
        ));
    }
    state.write(|tx| {
        tx.execute(
            "UPDATE ai_providers
                SET display_name = ?1, api = ?2, base_url = ?3, credential_ref = ?4,
                    endpoint_trust = ?5, enabled = ?6, revision = revision + 1
              WHERE id = ?7 AND revision = ?8",
            rusqlite::params![
                dto.display_name,
                dto.api,
                dto.base_url,
                dto.credential_ref,
                dto.endpoint_trust,
                dto.enabled as i64,
                dto.id,
                expected_revision,
            ],
        )?;
        Ok(())
    })?;
    read_back_provider(state, &dto.id)
}

/// 删除 provider（`deleteProvider`）。`ai_models` 靠 `ON DELETE CASCADE` 一起走（R1）。
///
/// **密钥跟着走**（2026-09-28 用户 bug「删除 provider 时，密钥被设置成不自动删除」）：
/// provider 没了，它那把托管 key 再也没有界面入口能摸到，却还留在
/// `credentials.dpapi.json` 里 —— 删 provider 时把**不再被任何 provider 引用**的那把
/// 一并清掉。三条边界：
///  · env 来源的 key 是只读的（`credentials::unset` 也动不了它），直接跳过；
///  · ref 被别的 provider 共用时留着（删的是引用，不是 key 本身）；
///  · 清理是**尽力而为**：provider 已经删掉了，key 那步失败不再把整个请求打成 5xx
///    （那只会让用户面对"提示失败、provider 却真没了"的中间态）。
pub fn delete_provider(state: &AppState, id: &str) -> Result<bool, ApiError> {
    let reference = get_provider_row(state, id)?
        .and_then(|row| row.credential_ref)
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty());
    let removed = state.write(|tx| {
        let n = tx.execute("DELETE FROM ai_providers WHERE id = ?1", rusqlite::params![id])?;
        Ok(n)
    })?;
    if removed > 0 {
        if let Some(reference) = reference {
            unset_orphan_credential(state, &reference);
        }
    }
    Ok(removed > 0)
}

/// provider 删掉之后，把它那把**已无人引用**的托管 key 清掉（见 [`delete_provider`]）。
fn unset_orphan_credential(state: &AppState, reference: &str) {
    // env 来源只读（AIH-013）：env 挡着的时候 managed 库里就算有同名条目也轮不到我们动
    if crate::credentials::env_source(reference).is_some() {
        return;
    }
    let still_used: bool = state
        .with(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ai_providers WHERE credential_ref = ?1",
                [reference],
                |row| row.get(0),
            )?;
            Ok(n > 0)
        })
        .unwrap_or(true); // 读不出来就当"还有人在用"：宁可留一把孤儿 key，也不误删在用的
    if still_used {
        return;
    }
    let _ = crate::credentials::unset(state, reference);
}

/// 全量替换某 Provider 的模型目录（`AiRepo.replaceModels`，AIH-011）。
///
/// **一个事务里删旧插新**：分开做的话，中途失败就会留下一个"模型全没了"的 provider ——
/// 而界面显示的是一份合法的空目录（"这个网关没有模型"），用户会去查网关。
///
/// ⚠ `providerId` 一律以路径参数为准（Kotlin `m.copy(providerId = providerId)` 同）——
/// body 里那一列是客户端抄来的，信它就会出现"给 A 存了一堆写着 B 的模型"。
///
/// ★ **一处有意的行为差异**（2026-09-25 双端对照时量到的）：body 里**少写** `providerId` 时，
/// Kotlin 的 `AiModelDto.providerId` 没有默认值 → 反序列化失败 → `400 {"error":"BadRequestException",
/// "detail":"Failed to convert request body to class …"}`（一个**机器码**，界面显示不出人话）；
/// 这里因为反正要用路径参数覆盖它，缺了就按空串收下 —— 结果是**同一件事做得更好**：
/// 客户端少传一个我们本来就要覆盖的字段，不该变成一句用户看不懂的报错。
/// 真实客户端（Flutter）永远是把 GET 到的那份原样改几个字段传回来，所以不会走到这条岔路。

pub fn replace_models(
    state: &AppState,
    provider_id: &str,
    models: &[AiModelDto],
) -> Result<Vec<AiModelDto>, ApiError> {
    let mut normalized: Vec<AiModelDto> = Vec::with_capacity(models.len());
    for model in models {
        let id = require_model_id(&model.id)?;
        let mut copy = model.clone();
        copy.id = id;
        copy.provider_id = provider_id.to_string();
        normalized.push(copy);
    }
    state.write(|tx| {
        tx.execute("DELETE FROM ai_models WHERE provider_id = ?1", rusqlite::params![provider_id])?;
        for model in &normalized {
            tx.execute(
                "INSERT INTO ai_models
                   (provider_id, model_id, display_name, input_modalities, attachment_transports,
                    mime_allowlist, tools, parallel_tools, reasoning, thinking_efforts, thinking_format,
                    context_window, max_output_tokens,
                    max_attachment_bytes, max_attachment_count, capability_source,
                    capability_verified_at, enabled)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
                rusqlite::params![
                    model.provider_id,
                    model.id,
                    model.display_name,
                    to_json_array(&model.input_modalities),
                    to_json_list_map(&model.attachment_transports),
                    to_json_array(&model.mime_allowlist),
                    model.tools as i64,
                    model.parallel_tools as i64,
                    model.reasoning as i64,
                    to_json_string_map(&model.thinking_efforts),
                    model.thinking_format.as_deref().map(str::trim).filter(|f| !f.is_empty()),
                    model.context_window,
                    model.max_output_tokens,
                    model.max_attachment_bytes,
                    model.max_attachment_count,
                    model.capability_source,
                    model.capability_verified_at,
                    model.enabled as i64,
                ],
            )?;
        }
        Ok(())
    })?;
    list_models(state, provider_id)
}

/// **只插一条**模型（`AiRepo.insertModel`，内置目录"补齐"用）。
///
/// 与 [`replace_models`] 的区别就在这：主键是 `(provider_id, model_id)`，所以"库里已经有
/// 同 id 的行"时会直接主键冲突 —— 调用方必须先查库、只把**缺失**的那些传进来。
/// 那正是"绝不覆盖用户改过的行"所需要的保护（`builtin.rs` 就是这么用的）。
pub fn insert_model(state: &AppState, model: &AiModelDto) -> Result<AiModelDto, ApiError> {
    state.write(|tx| {
        tx.execute(
            "INSERT INTO ai_models
               (provider_id, model_id, display_name, input_modalities, attachment_transports,
                mime_allowlist, tools, parallel_tools, reasoning, thinking_efforts, thinking_format,
                context_window, max_output_tokens,
                max_attachment_bytes, max_attachment_count, capability_source,
                capability_verified_at, enabled)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
            rusqlite::params![
                model.provider_id,
                model.id,
                model.display_name,
                to_json_array(&model.input_modalities),
                to_json_list_map(&model.attachment_transports),
                to_json_array(&model.mime_allowlist),
                model.tools as i64,
                model.parallel_tools as i64,
                model.reasoning as i64,
                to_json_string_map(&model.thinking_efforts),
                model.thinking_format.as_deref().map(str::trim).filter(|f| !f.is_empty()),
                model.context_window,
                model.max_output_tokens,
                model.max_attachment_bytes,
                model.max_attachment_count,
                model.capability_source,
                model.capability_verified_at,
                model.enabled as i64,
            ],
        )?;
        Ok(())
    })?;
    read_back_model(state, &model.provider_id, &model.id)
}

/// **只更新"能力声明"相关的列**（`AiRepo.updateModelCapabilities`，内置目录对齐用）。
///
/// 刻意不碰 `display_name` / `enabled` / 附件相关列 / `capability_verified_at`：
/// 用户改过的显示名、用户关掉的模型、用户的实测时间，都不是"对齐能力"该动的东西。
pub fn update_model_capabilities(state: &AppState, model: &AiModelDto) -> Result<AiModelDto, ApiError> {
    state.write(|tx| {
        tx.execute(
            "UPDATE ai_models
                SET input_modalities = ?1, tools = ?2, parallel_tools = ?3, reasoning = ?4,
                    thinking_efforts = ?5, thinking_format = ?6, context_window = ?7,
                    capability_source = ?8
              WHERE provider_id = ?9 AND model_id = ?10",
            rusqlite::params![
                to_json_array(&model.input_modalities),
                model.tools as i64,
                model.parallel_tools as i64,
                model.reasoning as i64,
                to_json_string_map(&model.thinking_efforts),
                model.thinking_format.as_deref().map(str::trim).filter(|f| !f.is_empty()),
                model.context_window,
                model.capability_source,
                model.provider_id,
                model.id,
            ],
        )?;
        Ok(())
    })?;
    read_back_model(state, &model.provider_id, &model.id)
}

/// 写完立刻回读（返回库里的那份，不是我们手上这份）。
fn read_back_model(state: &AppState, provider_id: &str, model_id: &str) -> Result<AiModelDto, ApiError> {
    let found = state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT * FROM ai_models WHERE provider_id = ?1 AND model_id = ?2",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![provider_id, model_id], model_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    })?;
    found.ok_or_else(|| ApiError::internal(format!("模型写入后读不到: {provider_id}/{model_id}")))
}

/// 写完立刻回读（**返回的是库里的那份**，不是我们手上这份）——
/// `revision` / `updated_at` 都是库算的，回我们那份就会给出一组过期的数字。
fn read_back_provider(state: &AppState, id: &str) -> Result<AiProviderDto, ApiError> {
    let row = get_provider_row(state, id)?
        .ok_or_else(|| ApiError::internal(format!("Provider 写入后读不到: {id}")))?;
    let stored = read_credential_refs(state);
    Ok(row.into_dto(&stored))
}

/// 三份 JSON 列的编码（与 Kotlin `AppJson.encodeToString` 同口径：
/// **数组/对象都按紧凑分隔符**，中文不转义）。
fn to_json_array(items: &[String]) -> String {
    let values: Vec<serde_json::Value> =
        items.iter().map(|s| serde_json::Value::String(s.clone())).collect();
    serde_json::Value::Array(values).to_string()
}

fn to_json_list_map(map: &std::collections::BTreeMap<String, Vec<String>>) -> String {
    let mut out = serde_json::Map::new();
    for (key, list) in map {
        out.insert(
            key.clone(),
            serde_json::Value::Array(
                list.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
            ),
        );
    }
    serde_json::Value::Object(out).to_string()
}

fn to_json_string_map(map: &std::collections::BTreeMap<String, String>) -> String {
    let mut out = serde_json::Map::new();
    for (key, value) in map {
        out.insert(key.clone(), serde_json::Value::String(value.clone()));
    }
    serde_json::Value::Object(out).to_string()
}

#[cfg(test)]
mod tests;
