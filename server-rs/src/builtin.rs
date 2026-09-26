//! 内置模型目录的**冻结副本**与登记（`AiSeedCatalog` + `AiSeeder` 的逐条对照，P4 第 38 步）。
//!
//! # 为什么要有一份"冻结副本"
//!
//! 用户原话："把 `.dsh/settings.yaml` 里的模型挨个抄过来，可以放在数据库里，
//! **不要让程序直接读 settings.yaml**"。装了本项目、没装 DSH 的机器上根本没有那个文件，
//! 所以那份 YAML 的信息被抄成了一份 JSON（31KB），运行期只读这份资源。
//!
//! # 这份 JSON 在哪（P4 收尾起）
//!
//! `server-rs/assets/ai/builtin-catalog.json` —— **Rust 侧的唯一真源**。
//! 灰度期它是"Kotlin 资源 + Rust 用 `include_str!` 指向同一个文件"（当时故意不复制，
//! 怕两份拷贝漂移）；Kotlin 退役（2026-09-26）时按原计划**搬进 `server-rs/`**，
//! 老那份留在 `legacy\server\src\main\resources\ai\` 里当历史，**不再被任何东西读**。
//! ⚠ 改这份 JSON 要**重新编译**（编译期 `include_str!`）。
//!
//! # 两种模式（语义差别**必须严格**）
//!
//! * [`SeedMode::AddMissing`]（默认，启动时走的那条）：**只补库里没有的 model_id**；
//!   已有的行一个字节都不改（`displayName` / `baseURL` / `credentialRef` / `revision` 全不碰），
//!   也不删任何模型（包括用户自己加的）；
//! * [`SeedMode::RefreshCapabilities`]（用户点按钮才走）：只把**目录里已有的那些 model_id**
//!   的能力声明对齐到目录；不新增、不删除、不碰 provider 行、不碰用户自己加的模型。
//!
//! 这条分界的教训是真实的：本机库里早有一份手工导入的 `command-code-goat`（69 个模型，
//! 但能力声明是导入时的乐观值），如果"provider 已存在就 skip"，用户看到的永远是"没登记"；
//! 而如果启动时就对齐能力，用户手工调过的声明会被**悄悄冲掉**。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ai::{AiModelDto, AiProviderDto};
use crate::routes::{ApiError, AppState};

/// `app_settings` 里记"已登记的内置目录版本"的键（`AiSeeder.VERSION_KEY`）。
pub const VERSION_KEY: &str = "ai.builtin.seed.version";

/// 连资源都读不出来时的占位（避免二次抛异常）。
const UNKNOWN_VERSION: &str = "unknown";

/// 冻结副本的正文（**编译期**读进来；见模块头的说明）。
const CATALOG_JSON: &str = include_str!("../assets/ai/builtin-catalog.json");

/// 顶层结构（`BuiltinCatalog`）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinCatalog {
    version: String,
    #[serde(default)]
    #[allow(dead_code)]
    source: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    note: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    agent_default_model: Option<Value>,
    providers: Vec<BuiltinProvider>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinProvider {
    pub id: String,
    pub display_name: String,
    pub api: String,
    #[serde(rename = "baseURL")]
    pub base_url: String,
    #[serde(default)]
    pub credential_ref: Option<String>,
    #[serde(default = "default_endpoint_trust")]
    pub endpoint_trust: String,
    #[serde(default)]
    pub models: Vec<BuiltinModel>,
}

fn default_endpoint_trust() -> String {
    "public".to_string()
}

/// 目录条目：**只声明模型客观能力**（模态 / 工具 / 推理 / 思考档位 / 上下文窗口）。
///
/// `attachmentTransports` / `mimeAllowlist` / 附件大小上限**故意不在目录里** ——
/// 协议适配器还没实现附件传输，写进去只会给出"能发图片"的假承诺（AIH-028/AIH-030）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinModel {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub context_window: Option<i64>,
    #[serde(default = "default_modalities")]
    pub input_modalities: Vec<String>,
    #[serde(default = "default_true")]
    pub tools: bool,
    #[serde(default)]
    pub parallel_tools: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub thinking_format: Option<String>,
    #[serde(default)]
    pub thinking_efforts: std::collections::BTreeMap<String, String>,
}

fn default_modalities() -> Vec<String> {
    vec!["text".to_string()]
}

fn default_true() -> bool {
    true
}

/// 解析冻结副本（每次现解析：31KB、一天几次调用，缓存带来的失效风险不值这点开销）。
fn catalog() -> Result<BuiltinCatalog, ApiError> {
    let parsed: BuiltinCatalog = serde_json::from_str(CATALOG_JSON)
        .map_err(|e| ApiError::ai("CONFIG_ERROR", format!("内置模型目录不是合法 JSON：{e}")))?;
    if parsed.providers.is_empty() {
        return Err(ApiError::ai("CONFIG_ERROR", "内置模型目录里没有任何 provider"));
    }
    if parsed.providers.iter().any(|p| p.models.is_empty()) {
        return Err(ApiError::ai("CONFIG_ERROR", "内置模型目录里有 provider 没有任何模型"));
    }
    Ok(parsed)
}

/// 目录版本（取自资源里的 `version` 字段，**不另硬编码一份**）。
pub fn catalog_version() -> String {
    catalog().map(|c| c.version).unwrap_or_else(|_| UNKNOWN_VERSION.to_string())
}

/// 目录里的第一个 provider（现在只有一个）。
fn first_provider() -> Result<(String, BuiltinProvider), ApiError> {
    let parsed = catalog()?;
    let version = parsed.version;
    let provider = parsed
        .providers
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::ai("CONFIG_ERROR", "内置模型目录里没有任何 provider"))?;
    Ok((version, provider))
}

// ---------------------------------------------------------------------------
//  给界面看的形状（`BuiltinSyncResult`）
// ---------------------------------------------------------------------------

/// 两种模式（`SeedMode`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedMode {
    AddMissing,
    RefreshCapabilities,
}

impl SeedMode {
    /// `?mode=` 的取值：`null` / `""` / `add-missing` → 补齐；`refresh-capabilities` / `refresh` → 对齐。
    ///
    /// 认不出来**直接报错**（不静默按默认走 —— 用户点了「对齐能力」，结果只补了缺的模型，
    /// 而界面会显示"成功"）。
    pub fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        match raw.map(|m| m.trim().to_lowercase()) {
            None => Ok(Self::AddMissing),
            Some(m) if m.is_empty() || m == "add-missing" => Ok(Self::AddMissing),
            Some(m) if m == "refresh-capabilities" || m == "refresh" => Ok(Self::RefreshCapabilities),
            Some(other) => Err(ApiError::ai(
                "CONFIG_ERROR",
                format!("未知的同步模式：{other}（可选 add-missing / refresh-capabilities）"),
            )),
        }
    }

    fn wire(self) -> &'static str {
        match self {
            Self::AddMissing => "add-missing",
            Self::RefreshCapabilities => "refresh-capabilities",
        }
    }
}

/// `BuiltinSyncResult`（`SeedSyncOutcome.toDto()` 的逐字对应物）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinSyncResult {
    /// add-missing / refresh-capabilities
    pub mode: String,
    /// 冻结目录的版本号
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// 这次新增的模型数
    pub added: i64,
    /// 这次被对齐能力声明的模型数（只有 refresh 模式会 > 0）
    pub updated: i64,
    /// 与目录一致、未改动的模型数
    pub kept: i64,
    /// 库里已有但能力声明与内置目录不同的 model_id（保持用户当前设置，等用户决定）
    pub divergent: Vec<String>,
    /// 库里该 provider 当前的模型总数
    pub model_count: i64,
    pub provider_created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl BuiltinSyncResult {
    fn failed(mode: SeedMode, version: String, error: impl Into<String>) -> Self {
        Self {
            mode: mode.wire().to_string(),
            version,
            provider_id: None,
            added: 0,
            updated: 0,
            kept: 0,
            divergent: Vec::new(),
            model_count: 0,
            provider_created: false,
            error: Some(error.into()),
        }
    }
}

// ---------------------------------------------------------------------------
//  纯逻辑（不碰数据库 —— 每个分支都能单测）
// ---------------------------------------------------------------------------

/// 目录 × 库 的对照结论。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SeedPlan {
    /// 库里没有、需要插入的 model_id
    pub added: Vec<String>,
    /// 库里已有且与目录一致、保持不动的 model_id
    pub kept: Vec<String>,
    /// 库里已有但能力声明与目录不同的 model_id（保持用户当前设置，只提示）
    pub divergent: Vec<String>,
}

/// `AiSeeder.planSeed`：**库里多出来的 model_id 不属于这三类里的任何一类** ——
/// 直接忽略，也就是永远不删、不改（这是刻意的）。
pub fn plan_seed(catalog: &[BuiltinModel], existing: &std::collections::BTreeMap<String, AiModelDto>) -> SeedPlan {
    let mut plan = SeedPlan::default();
    for model in catalog {
        match existing.get(&model.id) {
            None => plan.added.push(model.id.clone()),
            Some(row) if capability_differs(model, row) => plan.divergent.push(model.id.clone()),
            Some(_) => plan.kept.push(model.id.clone()),
        }
    }
    plan
}

/// 这条库里的行，能力声明与目录是否不同（`AiSeeder.capabilityDiffers`）。
///
/// 只比**能力**那几样，**不比** `displayName` / `enabled` / 附件上限 —— 那些是用户的地盘。
pub fn capability_differs(model: &BuiltinModel, row: &AiModelDto) -> bool {
    let want = to_model_dto(model, &row.provider_id, None);
    let sorted = |mut list: Vec<String>| {
        list.sort();
        list
    };
    sorted(row.input_modalities.clone()) != sorted(want.input_modalities.clone())
        || row.tools != want.tools
        || row.parallel_tools != want.parallel_tools
        || row.reasoning != want.reasoning
        || row.thinking_efforts != want.thinking_efforts
        || row.thinking_format != want.thinking_format
        || row.context_window != want.context_window
        || row.capability_source != want.capability_source
}

/// 模式 → 动作（`AiSeeder.decideActions`）：ADD_MISSING 只插入、REFRESH 只刷新（目标就是 divergent）。
pub fn decide_actions(mode: SeedMode, plan: &SeedPlan) -> (Vec<String>, Vec<String>) {
    match mode {
        SeedMode::AddMissing => (plan.added.clone(), Vec::new()),
        SeedMode::RefreshCapabilities => (Vec::new(), plan.divergent.clone()),
    }
}

/// 目录条目 → 写库形状（`AiSeeder.toModelDto`）。
///
/// `capability_source` 默认 `builtin`（对齐那条路要按"目录来的"记，否则
/// `capabilityDiffers` 会把每一行都判成"不一致" → 每次对齐都白写一遍）。
pub fn to_model_dto(model: &BuiltinModel, provider_id: &str, capability_source: Option<&str>) -> AiModelDto {
    AiModelDto {
        provider_id: provider_id.to_string(),
        id: model.id.clone(),
        display_name: model.display_name.clone(),
        input_modalities: model.input_modalities.clone(),
        // 附件传输方式 / MIME 白名单**故意留空**（AIH-028）
        attachment_transports: Default::default(),
        mime_allowlist: Vec::new(),
        tools: model.tools,
        parallel_tools: model.parallel_tools,
        reasoning: model.reasoning,
        thinking_efforts: model.thinking_efforts.clone(),
        thinking_format: model.thinking_format.clone(),
        context_window: model.context_window,
        max_output_tokens: None,
        max_attachment_bytes: None,
        max_attachment_count: None,
        capability_source: capability_source.unwrap_or("builtin").to_string(),
        capability_verified_at: None,
        enabled: true,
    }
}

/// 目录条目 → provider 行（`AiSeeder.toProviderDto`）。
fn to_provider_dto(provider: &BuiltinProvider) -> Result<AiProviderDto, ApiError> {
    let api = crate::ai::API_WIRE_VALUES
        .iter()
        .copied()
        .find(|wire| *wire == provider.api)
        .ok_or_else(|| {
            ApiError::ai(
                "CONFIG_ERROR",
                format!(
                    "内置目录里的协议 {} 不受支持（可选 {}）",
                    provider.api,
                    crate::ai::API_WIRE_VALUES.join(" / ")
                ),
            )
        })?;
    let trust = crate::endpoint_guard::parse_trust(&provider.endpoint_trust)
        .map(str::to_string)
        .unwrap_or_else(|| crate::endpoint_guard::infer_trust(&crate::ai::host_of(&provider.base_url)).to_string());
    Ok(AiProviderDto {
        id: crate::ai::require_provider_id(Some(&provider.id))?,
        display_name: crate::ai::require_display_name(Some(&provider.display_name))?,
        api: api.to_string(),
        base_url: crate::ai::normalize_base_url(Some(&provider.base_url), &trust)?,
        credential_ref: crate::ai::require_credential_ref(provider.credential_ref.as_deref())?,
        endpoint_trust: trust,
        enabled: true,
        revision: 1,
        credential: crate::ai::describe_credential(None, |_| None, |_| false),
        created_at: None,
        updated_at: None,
    })
}

// ---------------------------------------------------------------------------
//  读侧（预览）与写侧（同步）
// ---------------------------------------------------------------------------

/// 库里该 provider 的模型（键为 model_id）；provider 不存在 → `None`。
fn existing_models(state: &AppState, id: &str) -> Result<Option<std::collections::BTreeMap<String, AiModelDto>>, ApiError> {
    if crate::ai::get_provider_row(state, id)?.is_none() {
        return Ok(None);
    }
    let list = crate::ai::list_models(state, id)?;
    Ok(Some(list.into_iter().map(|m| (m.id.clone(), m)).collect()))
}

/// `GET /api/ai/builtin/status` —— **只读预览**：不写库，只回答"现在同步会发生什么"。
///
/// 界面按钮在真正对齐前用它弹确认框（"将新增 N 个模型 / 有 M 个模型的能力与内置目录不同"）。
/// 口径与 `AddMissing` 完全一致：`added` = 现在缺的、`kept` = 已存在且一致、
/// `divergent` = 已存在但能力不同（预览里**不会**被改）、`updated` 恒为 0。
pub fn preview(state: &AppState) -> BuiltinSyncResult {
    let version = catalog_version();
    let mode = SeedMode::AddMissing;
    let (version, provider) = match first_provider() {
        Ok(pair) => pair,
        Err(e) => return BuiltinSyncResult::failed(mode, version, e.human()),
    };
    let id = match crate::ai::require_provider_id(Some(&provider.id)) {
        Ok(id) => id,
        Err(e) => return BuiltinSyncResult::failed(mode, version, e.human()),
    };
    // 两条都是 SELECT；读到什么就交给纯函数算，写完的事一件不做。
    let existing = match existing_models(state, &id) {
        Ok(existing) => existing,
        Err(e) => return BuiltinSyncResult::failed(mode, version, e.human()),
    };
    match existing {
        None => BuiltinSyncResult {
            mode: mode.wire().to_string(),
            version,
            provider_id: Some(id),
            added: provider.models.len() as i64,
            updated: 0,
            kept: 0,
            divergent: Vec::new(),
            model_count: provider.models.len() as i64,
            provider_created: true,
            error: None,
        },
        Some(rows) => {
            let plan = plan_seed(&provider.models, &rows);
            BuiltinSyncResult {
                mode: mode.wire().to_string(),
                version,
                provider_id: Some(id),
                added: plan.added.len() as i64,
                updated: 0,
                kept: plan.kept.len() as i64,
                divergent: plan.divergent,
                // 跑完之后的总数 = 现在库里的（含用户自己加的）+ 这次要补的
                model_count: rows.len() as i64 + plan.added.len() as i64,
                provider_created: false,
                error: None,
            }
        }
    }
}

/// `POST /api/ai/builtin/sync` —— 幂等登记；**绝不对用户改过的东西下手**。
///
/// 失败**不抛 5xx**：返回 `ok=false` 语义的 `error` 字段（与 Kotlin 同 ——
/// 登记失败不该让整个设置页报错，用户要能看到"为什么没同步上"）。
pub fn sync(state: &AppState, mode: SeedMode) -> BuiltinSyncResult {
    let fallback_version = catalog_version();
    let (version, provider) = match first_provider() {
        Ok(pair) => pair,
        Err(e) => return BuiltinSyncResult::failed(mode, fallback_version, e.human()),
    };
    match sync_provider(state, &provider, &version, mode) {
        Ok(result) => result,
        Err(e) => BuiltinSyncResult::failed(mode, version, e.human()),
    }
}

fn sync_provider(
    state: &AppState,
    provider: &BuiltinProvider,
    version: &str,
    mode: SeedMode,
) -> Result<BuiltinSyncResult, ApiError> {
    let id = crate::ai::require_provider_id(Some(&provider.id))?;
    let Some(rows) = existing_models(state, &id)? else {
        if mode == SeedMode::RefreshCapabilities {
            // REFRESH 的语义是"只对齐、不新增"：没有 provider 就没有可对齐的行。
            // 不动库、也不写版本号（写一个没生效的版本只会误导人）。
            return Ok(BuiltinSyncResult {
                mode: mode.wire().to_string(),
                version: version.to_string(),
                provider_id: Some(id),
                added: 0,
                updated: 0,
                kept: 0,
                divergent: Vec::new(),
                model_count: 0,
                provider_created: false,
                error: None,
            });
        }
        return insert_all(state, provider, &id, version, mode);
    };
    reconcile(state, provider, &id, version, mode, rows)
}

/// provider 不在：整份插入。
fn insert_all(
    state: &AppState,
    provider: &BuiltinProvider,
    id: &str,
    version: &str,
    mode: SeedMode,
) -> Result<BuiltinSyncResult, ApiError> {
    let dto = to_provider_dto(provider)?;
    let models: Vec<AiModelDto> = provider.models.iter().map(|m| to_model_dto(m, id, None)).collect();
    for model in &models {
        crate::ai::validate_model(model)?;
    }
    crate::ai::insert_provider(state, &dto)?;
    let stored = crate::ai::replace_models(state, id, &models)?;
    put_version(state, version)?;
    Ok(BuiltinSyncResult {
        mode: mode.wire().to_string(),
        version: version.to_string(),
        provider_id: Some(id.to_string()),
        added: stored.len() as i64,
        updated: 0,
        kept: 0,
        divergent: Vec::new(),
        model_count: stored.len() as i64,
        provider_created: true,
        error: None,
    })
}

/// provider 已在：按模式补缺 / 对齐能力，**绝不**碰 provider 行与用户自有模型。
fn reconcile(
    state: &AppState,
    provider: &BuiltinProvider,
    id: &str,
    version: &str,
    mode: SeedMode,
    rows: std::collections::BTreeMap<String, AiModelDto>,
) -> Result<BuiltinSyncResult, ApiError> {
    let plan = plan_seed(&provider.models, &rows);
    let (to_insert, to_refresh) = decide_actions(mode, &plan);
    let by_id: std::collections::BTreeMap<&str, &BuiltinModel> =
        provider.models.iter().map(|m| (m.id.as_str(), m)).collect();

    let mut added = 0i64;
    for model_id in &to_insert {
        let model = by_id
            .get(model_id.as_str())
            .ok_or_else(|| ApiError::internal(format!("目录里找不到 {model_id}")))?;
        let dto = to_model_dto(model, id, None);
        crate::ai::validate_model(&dto)?;
        crate::ai::insert_model(state, &dto)?;
        added += 1;
    }
    let mut updated = 0i64;
    for model_id in &to_refresh {
        let model = by_id
            .get(model_id.as_str())
            .ok_or_else(|| ApiError::internal(format!("目录里找不到 {model_id}")))?;
        let dto = to_model_dto(model, id, None);
        crate::ai::validate_model(&dto)?;
        crate::ai::update_model_capabilities(state, &dto)?;
        updated += 1;
    }
    put_version(state, version)?;
    let total = crate::ai::list_models(state, id)?.len() as i64;
    Ok(BuiltinSyncResult {
        mode: mode.wire().to_string(),
        version: version.to_string(),
        provider_id: Some(id.to_string()),
        added,
        updated,
        kept: plan.kept.len() as i64,
        divergent: plan.divergent,
        model_count: total,
        provider_created: false,
        error: None,
    })
}

/// 记"已登记的内置目录版本"（`SettingsRepo.put(VERSION_KEY, version)`）。
fn put_version(state: &AppState, version: &str) -> Result<(), ApiError> {
    state.write(|tx| {
        tx.execute(
            "INSERT INTO app_settings (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
            rusqlite::params![VERSION_KEY, version],
        )?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(id: &str, modalities: &[&str], tools: bool, reasoning: bool) -> BuiltinModel {
        BuiltinModel {
            id: id.to_string(),
            display_name: id.to_uppercase(),
            context_window: Some(1000),
            input_modalities: modalities.iter().map(|s| s.to_string()).collect(),
            tools,
            parallel_tools: false,
            reasoning,
            thinking_format: None,
            thinking_efforts: Default::default(),
        }
    }

    fn row_of(model: &BuiltinModel, provider_id: &str) -> AiModelDto {
        to_model_dto(model, provider_id, None)
    }

    /// 冻结副本本身要能读、版本号要对得上、每条都有 id（它是**出厂默认值**的唯一来源）。
    #[test]
    fn 冻结副本可读而且不为空() {
        let parsed = catalog().expect("内置目录必须能解析");
        assert!(!parsed.providers.is_empty());
        assert!(!parsed.version.trim().is_empty());
        for provider in &parsed.providers {
            assert!(!provider.models.is_empty(), "{} 一个模型都没有", provider.id);
            for model in &provider.models {
                assert!(!model.id.trim().is_empty());
                assert!(!model.display_name.trim().is_empty());
            }
        }
        // provider 行也要能过我们那套校验（协议 / URL / 信任档 / 凭据引用名）
        for provider in &parsed.providers {
            to_provider_dto(provider).expect("内置 provider 必须过校验");
        }
        // 每条模型也要能过模型校验
        for provider in &parsed.providers {
            for model in &provider.models {
                crate::ai::validate_model(&to_model_dto(model, &provider.id, None))
                    .unwrap_or_else(|e| panic!("{} 过不了模型校验：{}", model.id, e.human()));
            }
        }
    }

    /// `planSeed` 的三条分支 + "库里多出来的不算任何一类"（永远不删）。
    #[test]
    fn 对照结论把缺的_一致的_不一致的分开() {
        let catalog = vec![
            builtin("a", &["text"], true, false),
            builtin("b", &["text", "image"], true, false),
            builtin("c", &["text"], false, true),
        ];
        let mut existing = std::collections::BTreeMap::new();
        existing.insert("b".to_string(), row_of(&catalog[1], "p"));
        // c 的能力与目录不同（库里说它不支持推理）
        let mut c_row = row_of(&catalog[2], "p");
        c_row.reasoning = false;
        existing.insert("c".to_string(), c_row);
        // 用户自己加的：不属于任何一类
        existing.insert("user-only".to_string(), row_of(&builtin("user-only", &["text"], true, false), "p"));

        let plan = plan_seed(&catalog, &existing);
        assert_eq!(plan.added, vec!["a"]);
        assert_eq!(plan.kept, vec!["b"]);
        assert_eq!(plan.divergent, vec!["c"]);
        assert!(!plan.added.contains(&"user-only".to_string()), "用户自己加的不进任何一类");
        assert!(!plan.divergent.contains(&"user-only".to_string()));
    }

    /// 能力不同的判据：**只比能力那八样**，显示名 / 启用状态 / 附件上限都不算。
    #[test]
    fn 只比能力不比用户的地盘() {
        let model = builtin("m", &["text", "image"], true, true);
        let base = row_of(&model, "p");
        assert!(!capability_differs(&model, &base), "同一条不算不同");

        // 用户改过的：显示名 / 启用 / 附件上限 —— 都不该判成"能力不同"
        let mut renamed = base.clone();
        renamed.display_name = "用户改的名字".into();
        renamed.enabled = false;
        renamed.max_attachment_bytes = Some(1024);
        assert!(!capability_differs(&model, &renamed));

        // 模态顺序不同不算不同（比的是排序后的集合）
        let mut reordered = base.clone();
        reordered.input_modalities = vec!["image".into(), "text".into()];
        assert!(!capability_differs(&model, &reordered));

        // 真正的能力不同：少一个模态 / 推理关了 / 上下文变了 / 来源变了
        for mutate in [
            (|m: &mut AiModelDto| m.input_modalities = vec!["text".into()]) as fn(&mut AiModelDto),
            |m: &mut AiModelDto| m.reasoning = false,
            |m: &mut AiModelDto| m.context_window = Some(7),
            |m: &mut AiModelDto| m.capability_source = "manual".into(),
        ] {
            let mut changed = base.clone();
            mutate(&mut changed);
            assert!(capability_differs(&model, &changed), "{changed:?} 应当算不同");
        }
    }

    /// 模式 → 动作：ADD_MISSING 只插入、REFRESH 只刷新。
    #[test]
    fn 两种模式的动作互不重叠() {
        let plan = SeedPlan { added: vec!["a".into()], kept: vec!["b".into()], divergent: vec!["c".into()] };
        let (insert, refresh) = decide_actions(SeedMode::AddMissing, &plan);
        assert_eq!(insert, vec!["a"]);
        assert!(refresh.is_empty(), "补齐模式**绝不对齐**（会冲掉用户改过的能力）");
        let (insert, refresh) = decide_actions(SeedMode::RefreshCapabilities, &plan);
        assert!(insert.is_empty(), "对齐模式**绝不新增**");
        assert_eq!(refresh, vec!["c"]);
    }

    /// `?mode=` 的取值：认可的那几个，其它一律报错（不静默按默认走）。
    #[test]
    fn 同步模式的取值与_kotlin_一致() {
        assert_eq!(SeedMode::parse(None).unwrap(), SeedMode::AddMissing);
        assert_eq!(SeedMode::parse(Some("")).unwrap(), SeedMode::AddMissing);
        assert_eq!(SeedMode::parse(Some("add-missing")).unwrap(), SeedMode::AddMissing);
        assert_eq!(SeedMode::parse(Some("refresh")).unwrap(), SeedMode::RefreshCapabilities);
        assert_eq!(SeedMode::parse(Some("REFRESH-CAPABILITIES")).unwrap(), SeedMode::RefreshCapabilities);
        let err = SeedMode::parse(Some("whatever")).unwrap_err();
        assert_eq!(
            err.human(),
            "未知的同步模式：whatever（可选 add-missing / refresh-capabilities）"
        );
    }

    /// 目录条目 → 写库形状：附件列**故意留空**（写了就是假承诺），能力来源是 builtin。
    #[test]
    fn 写进库的形状不带假的附件能力() {
        let model = builtin("m", &["text", "image"], true, false);
        let dto = to_model_dto(&model, "p", None);
        assert!(dto.attachment_transports.is_empty(), "适配器还没实现附件传输");
        assert!(dto.mime_allowlist.is_empty());
        assert!(dto.max_attachment_bytes.is_none());
        assert_eq!(dto.capability_source, "builtin");
        assert!(dto.enabled);
    }
}
