//! `civitai_search`：Civitai 模型库搜索（Kotlin `CivitaiSearch.kt` 的逐条对照，
//! 参见建议 ④/⑦）。
//!
//! # 需求原文与实现的对应关系
//!
//! > "设计一个工具，允许 AI 搜索 civitai（注：civitai 如要搜索 NSFW 内容需要登录或者令牌，
//! >  且域名变成 .red；此时可以询问，如允许则模拟真机搜索）"
//! >
//! > "或许可以让 ai 联网搜索 comfy 库里各种模型信息（civitai，或者 hugging face 等等），
//! >  贴上简短的说明（比如它拿来做什么），登记在数据库中"
//!
//! 所以这里做三件事：
//!  1. **普通（安全）搜索**走 `civitai.com/api/v1/models`，免令牌；
//!  2. **NSFW 搜索**走 `civitai.red`，**域名真的切过去**并把 `nsfw=true` 明写进查询串
//!     （"模拟真机搜索"即此意）；工具侧默认 `ASK` —— 对应"此时可以询问"；
//!  3. 每条结果都带上**这个模型是干什么的**：类型 + 基础模型 + 触发词 + 简介。
//!
//! # ⚠ 与线上实测不一致的假设（2026-09-20 在本机实打 civitai.com / civitai.red 核对过）
//!
//! 原始需求的前提是"NSFW 需要登录 / 令牌"。实测 `/api/v1/models` 这条**搜索**接口
//! **并不是**这样：匿名就返回 `nsfw=true` 的条目，`.red` 匿名同样 200，连**假的**
//! `Authorization: Bearer` 也不会被拒 —— 这条接口根本不校验令牌。
//!
//! 因此"NSFW 必须有令牌"属于**既定策略**（表示"这是登录态才做的事"，并天然包含一次
//! 审批），**不是**上游的要求。这个区别要留着：别在注释、文档或给模型的提示里把它说成
//! "不给令牌就搜不到"—— 该说法**不成立**，模型会据此给出错误说明或放弃搜索。
//! 上游真正会 400 的是别的东西：`types` 大小写敏感、`limit` 上限 100。
//!
//! # 出网纪律（与 [`crate::tools::web_tools`] 同规矩）
//!
//! * 只打两个**写死**的域名（`civitai.com` / `civitai.red`），不接受任意 Host；
//! * **绝不记录 / 回显令牌**（AIH-051）：错误正文里一个字符都不带；
//! * 返回的正文一律当**不可信数据**（工具侧套 [`crate::tools::tools::untrusted_banner`]）。
//!
//! # 两处 JVM 口径照抄
//!
//! 查询串过 [`crate::tools::tools::url_encode`]（Java `URLEncoder`：**空格变 `+`**）——
//! `sort=Most+Downloaded` 那个空格是本机实测抓到过一次 `URISyntaxException` 的地方；
//! 简介的 HTML 去标签 + 空白压平也照 Kotlin 的 `Regex` 口径（[`strip_tags`]）。

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::tools::tools::{
    collapse_spaces, java_error_name, outbound_error_kind, str_arg, url_encode, OutboundError,
    ToolFailure, ToolOutcome,
};
use crate::web::routes::AppState;

pub const SAFE_BASE_URL: &str = "https://civitai.com";
pub const NSFW_BASE_URL: &str = "https://civitai.red";

/// 令牌在凭据库里的引用名（与 `keenable` 同一约定）。
pub const CREDENTIAL_REF: &str = "civitai";

/// Civitai 生态中通行的环境变量名（通常已有配置）。
pub const ENV_TOKEN: &str = "CIVITAI_API_KEY";
/// 两个根地址的覆盖口子（本机假服务 / 自建代理）。
pub const ENV_SAFE_BASE_URL: &str = "CIVITAI_BASE_URL";
pub const ENV_NSFW_BASE_URL: &str = "CIVITAI_NSFW_BASE_URL";

pub const CONNECT_TIMEOUT_SECONDS: u64 = 5;
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 25;
pub const MAX_TIMEOUT_SECONDS: u64 = 60;

/// 一次最多返回几条（再多也不会被查看，只会浪费 token）。
pub const MAX_LIMIT: i64 = 20;

/// 上游自己允许的 `limit` 上限（2026-09-20 实测：`limit=100` 200、`limit=101` 400）。
///
/// 记在这里是为了让 [`MAX_LIMIT`] 看起来是"我们自己的取舍"而不是"上游只给这么多"——
/// 哪天有人想放宽到 50，不必再去试探一遍线上。
pub const UPSTREAM_MAX_LIMIT: i64 = 100;

/// 每条简介的字符上限 —— 需要"简短说明"，而不是整页 README。
pub const MAX_DESCRIPTION_CHARS: usize = 600;

/// 允许的模型类型（Civitai 的 `types` 参数取值）。
///
/// **线上大小写敏感**（实测：`LORA` 200，`lora` / `Lora` 400），所以这里的拼写就是官方拼写，
/// [`CivitaiClient::search`] 会先把调用方给的值归一成它。这里每一个值都实测过 200。
/// 顺序就是报错文案里列出来的顺序（Kotlin 那边是 `setOf` = 插入序）。
pub const TYPES: [&str; 8] = [
    "Checkpoint",
    "LORA",
    "TextualInversion",
    "Controlnet",
    "Upscaler",
    "VAE",
    "LoCon",
    "DoRA",
];

/// 一条 Civitai 模型 / LoRA（只留"用得上"的那几个字段）。
#[derive(Debug, Clone)]
pub struct CivitaiModel {
    pub id: i64,
    pub name: String,
    pub r#type: String,
    pub base_model: String,
    pub nsfw: bool,
    pub creator: Option<String>,
    pub downloads: i64,
    pub thumbs_up: i64,
    pub page_url: String,
    /// 去掉 HTML 标签、压成一行、截断后的简介 —— 就是"它拿来做什么"那句话。
    pub description: String,
    pub trained_words: Vec<String>,
    /// 主文件的字节数；**拿不到就是 `None`**（老版本 / 结构变了 / 假服务）——
    /// 那时界面与正文都不写这一行，绝不用一个猜出来的数字去撑场面。
    pub size_bytes: Option<i64>,
    pub download_url: Option<String>,
    pub file_name: Option<String>,
    /// 主文件的 **sha256**（上游 `files[primary].hashes.SHA256`）：手动下载后
    /// 拿它比对盘上那份文件，判断"是不是同一个模型"。上游没给就是 `None` —— **不编**。
    pub sha256: Option<String>,
}

impl CivitaiModel {
    /// 给人看的体积标签。
    ///
    /// 单位分档是有意的：LoRA 常见 100~300MB，用 GB 显示会变成 "0.1GB"（看不出差别）；
    /// 而 checkpoints 动辄 6~14GB，用 MB 显示就是 "13490MB"（数不清有几个零）。
    /// **拿不到体积时返回"未知（作者没给）"**，不是空串 —— 空着会让模型自己编一个。
    pub fn size_label(&self) -> String {
        match self.size_bytes {
            Some(bytes) => {
                let gb = bytes as f64 / 1024.0 / 1024.0 / 1024.0;
                if gb >= 1.0 {
                    format!("{gb:.2} GB")
                } else {
                    format!("{:.0} MB", bytes as f64 / 1024.0 / 1024.0)
                }
            }
            None => "未知（作者没给文件体积）".to_string(),
        }
    }
}

/// 一次 Civitai 搜索的结果。
///
/// `context` 是**已经渲染好**的、给模型看的块；界面要用就读 `meta` / `models`，
/// 不要去解析 `context` 的文本（与 [`crate::tools::web_tools::WebSearchOutcome`] 同约定）。
#[derive(Debug)]
pub struct CivitaiOutcome {
    pub query: String,
    pub models: Vec<CivitaiModel>,
    pub context: String,
    pub meta: Value,
}

impl CivitaiOutcome {
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// 给前端 / 入库用的结构化结果：摘要 + 每条模型的 id / 名字 / 说明。
    ///
    /// 键的顺序与取舍照抄 Kotlin `toJson()`（手拼 JSON，可空键**省掉**而不是写 null）。
    pub fn to_json(&self) -> Value {
        let models: Vec<Value> = self
            .models
            .iter()
            .map(|m| {
                let mut out = Map::new();
                out.insert("id".into(), json!(m.id));
                out.insert("name".into(), json!(m.name));
                out.insert("type".into(), json!(m.r#type));
                out.insert("baseModel".into(), json!(m.base_model));
                out.insert("nsfw".into(), json!(m.nsfw));
                if let Some(creator) = m.creator.as_deref() {
                    out.insert("creator".into(), json!(creator));
                }
                out.insert("downloads".into(), json!(m.downloads));
                out.insert("thumbsUp".into(), json!(m.thumbs_up));
                out.insert("pageUrl".into(), json!(m.page_url));
                out.insert("description".into(), json!(m.description));
                // 体积：拿不到就**不写这个键**（比写一个 null / 0 更不容易被误读成"很小"）
                if let Some(bytes) = m.size_bytes {
                    out.insert("sizeBytes".into(), json!(bytes));
                    out.insert("sizeLabel".into(), json!(m.size_label()));
                }
                if let Some(url) = m.download_url.as_deref() {
                    out.insert("downloadUrl".into(), json!(url));
                }
                if let Some(name) = m.file_name.as_deref() {
                    out.insert("fileName".into(), json!(name));
                }
                if let Some(sha) = m.sha256.as_deref() {
                    out.insert("sha256".into(), json!(sha));
                }
                if !m.trained_words.is_empty() {
                    out.insert("trainedWords".into(), json!(m.trained_words));
                }
                Value::Object(out)
            })
            .collect();
        json!({ "query": self.query, "count": self.models.len(), "models": models })
    }
}

/// 一次 Civitai 搜索的客户端（唯一出网口）。
///
/// 两个根地址都**显式传**（不在这里读环境）：用例要能把安全档与 NSFW 档都指到同一台
/// 本机假服务上 —— **判据是客户端选了哪个 base**，而不是真去连 `civitai.red`。
pub struct CivitaiClient {
    safe_base: String,
    nsfw_base: String,
    timeout: Duration,
    /// **绝不进日志、绝不进错误正文**（AIH-051）。
    token: Option<String>,
}

impl CivitaiClient {
    pub fn new(
        safe_base_url: &str,
        nsfw_base_url: &str,
        timeout_seconds: u64,
        token: Option<String>,
    ) -> Result<Self, ToolFailure> {
        let safe = safe_base_url.trim().trim_end_matches('/').to_string();
        let nsfw = nsfw_base_url.trim().trim_end_matches('/').to_string();
        if safe.is_empty() || nsfw.is_empty() {
            return Err(ToolFailure::new(
                "CIVITAI_FAILED",
                "Civitai 地址是空的（这是本机的 bug，请如实告诉用户）",
            ));
        }
        Ok(Self {
            safe_base: safe,
            nsfw_base: nsfw,
            timeout: Duration::from_secs(timeout_seconds.clamp(1, MAX_TIMEOUT_SECONDS)),
            token: token.filter(|t| !t.trim().is_empty()),
        })
    }

    /// 按环境变量与凭据库造一个。
    ///
    /// 令牌两路都认：受管凭据库（`civitai`，界面里填的那份，含它的环境变量写法）
    /// 优先，`CIVITAI_API_KEY` 兜底 —— 那是 Civitai 生态里通行的名字。
    pub fn from_env(state: &AppState) -> Result<Self, ToolFailure> {
        let safe = crate::tools::tools::env_or(ENV_SAFE_BASE_URL, SAFE_BASE_URL);
        let nsfw = crate::tools::tools::env_or(ENV_NSFW_BASE_URL, NSFW_BASE_URL);
        Self::new(&safe, &nsfw, DEFAULT_TIMEOUT_SECONDS, resolve_token(state))
    }

    /// 有没有配置令牌（工具描述与错误提示都靠它说实话）。
    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    pub fn safe_base_url(&self) -> &str {
        &self.safe_base
    }

    /// 搜一次 Civitai 的模型 / LoRA 库。
    pub async fn search(
        &self,
        query: &str,
        allow_nsfw: bool,
        type_filter: Option<&str>,
        limit: i64,
    ) -> Result<CivitaiOutcome, ToolFailure> {
        let q = query.trim();
        if q.is_empty() {
            return Err(ToolFailure::new("INVALID_ARGUMENT", "缺少搜索词 query"));
        }

        // `types` 线上是**大小写敏感的枚举**：实测 `LORA` 200，而 `lora` / `Lora` 都是 400。
        // 模型把类型写成小写是很自然的，所以这里先归一成官方拼写；真不认识的值当场报清楚，
        // 别让它变成一句"上游 400"—— 模型看不出自己哪里写错了，只会反复试。
        let type_filter = match type_filter.map(str::trim).filter(|raw| !raw.is_empty()) {
            None => None,
            Some(raw) => match TYPES.iter().find(|t| t.eq_ignore_ascii_case(raw)) {
                Some(found) => Some(*found),
                None => {
                    return Err(ToolFailure::new(
                        "INVALID_ARGUMENT",
                        format!("不认识的模型类型 \"{raw}\"；可选：{}", TYPES.join(" / ")),
                    ))
                }
            },
        };

        // NSFW 档：走成人站 `.red`，**有没有令牌都照搜**。
        //
        // 2026-09-20 之前的写法是"没令牌当场抛 CIVITAI_TOKEN_REQUIRED、一个请求都不发"，
        // 依据是"`.red` 只认登录态"。真机核对把这个前提推翻了（见模块头）。所以那条硬门槛
        // 等于"实际可搜，工具却报告无法搜索"，因此放宽为当前行为：
        //   · 显式 `allowNsfw=true` 仍然必须（工具默认 ASK，确认后才执行到此）；
        //   · 配置了令牌则带上（登录态更稳定，也符合"模拟真机搜索"的语义）；
        //   · 没配也不拒绝，但**必须如实说明这次是匿名通道**（见 [`render`] 那行提示）。
        let base = if allow_nsfw {
            &self.nsfw_base
        } else {
            &self.safe_base
        };
        let mut url = String::new();
        url.push_str(base);
        url.push_str("/api/v1/models?");
        url.push_str("query=");
        url.push_str(&url_encode(q));
        url.push_str(&format!("&limit={}", limit.clamp(1, MAX_LIMIT)));
        // ⚠️ 这里**必须**过 enc：`Most Downloaded` 里有个空格，
        // 直接拼进 URI 会抛 `URISyntaxException: Illegal character in query`
        url.push_str("&sort=");
        url.push_str(&url_encode("Most Downloaded"));
        url.push_str("&period=AllTime");
        if let Some(filter) = type_filter {
            url.push_str("&types=");
            url.push_str(&url_encode(filter));
        }
        // 走 .red 时把 nsfw 显式写进查询串：即"模拟真机搜索"所需的一步
        if allow_nsfw {
            url.push_str("&nsfw=true");
        }

        let body = self.get(&url).await?;
        let models: Vec<CivitaiModel> = body
            .get("items")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_object().and_then(to_model))
                    .collect()
            })
            .unwrap_or_default();

        let tokenless = self.token.is_none();
        let context = render(&models, q, allow_nsfw, tokenless);
        let host = reqwest::Url::parse(base)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string));
        let meta = json!({
            "count": models.len(),
            "query": q,
            "nsfw": allow_nsfw,
            "host": host.unwrap_or_default(),
            "tokenless": tokenless,
        });
        Ok(CivitaiOutcome {
            query: q.to_string(),
            models,
            context,
            meta,
        })
    }

    // -----------------------------------------------------------------------
    //  出网
    // -----------------------------------------------------------------------

    async fn get(&self, url: &str) -> Result<Map<String, Value>, ToolFailure> {
        // URL 拼错了（空格、中文没编码…）不该变成一个裸的解析异常冒到 Run 外面：
        // 那就是一次"工具失败"，按稳定错误码如实报给模型。
        let parsed = reqwest::Url::parse(url).map_err(|_| {
            ToolFailure::new(
                "CIVITAI_FAILED",
                "搜索地址拼装失败（URISyntaxException）；这是本机的 bug，请如实告诉用户",
            )
        })?;
        let host = parsed.host_str().unwrap_or("civitai.com").to_string();
        let route = crate::infra::outbound::route(&host).describe();

        let client = self.http_client(&host)?;
        let mut request = client
            .get(parsed)
            .header("Accept", "application/json")
            .timeout(self.timeout);
        // 日志与错误里**只出现状态码**，令牌一个字符都不落
        if let Some(token) = self.token.as_deref() {
            request = request.header("Authorization", format!("Bearer {token}"));
        }

        let response = request
            .send()
            .await
            .map_err(|e| match outbound_error_kind(&e) {
                OutboundError::ConnectTimeout => ToolFailure::new(
                    "CIVITAI_TIMEOUT",
                    format!(
                        "连不上 Civitai：{host} 在 {CONNECT_TIMEOUT_SECONDS} 秒内没连上。\
                     本次出网路径是「{route}」。\
                     如果本机需要代理才能访问 civitai.com，请确认系统代理可用（或设环境变量 {}，\
                     写 direct 表示强制直连）；**不要**反复重试同一个查询，\
                     如实告诉用户「本机连不上 Civitai」。",
                        crate::infra::outbound::ENV_PROXY
                    ),
                ),
                OutboundError::ReadTimeout => ToolFailure::new(
                    "CIVITAI_TIMEOUT",
                    format!(
                    "搜 Civitai 超时（连上了但 {} 秒没回完），稍后再试；本次出网路径是「{route}」",
                    self.timeout.as_secs()
                ),
                ),
                OutboundError::Other => ToolFailure::new(
                    "CIVITAI_FAILED",
                    format!(
                        "连不上 Civitai（{}）；本机需要能访问 {host}（本次出网路径：{route}）",
                        java_error_name(&e)
                    ),
                ),
            })?;

        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            let (code, hint) = match status {
                401 | 403 => (
                    "CIVITAI_AUTH",
                    "Civitai 拒绝了令牌；如果开的是 NSFW 搜索，请让用户重新填一个有效的 Civitai API Key"
                        .to_string(),
                ),
                429 => ("CIVITAI_RATE_LIMIT", "Civitai 限流了（429）；等一下再试，别连着搜".to_string()),
                500..=599 => ("CIVITAI_FAILED", format!("Civitai 暂时不可用（{status}）")),
                _ => ("CIVITAI_FAILED", format!("Civitai 返回 {status}")),
            };
            // 5xx 是"对方回了"而不是"我们没出去"——路径必须跟着一起说（同 danbooru）。
            return Err(ToolFailure::new(
                code,
                format!("{hint}（本次出网路径：{route}）"),
            ));
        }

        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(obj)) => Ok(obj),
            _ => Err(ToolFailure::new(
                "CIVITAI_FAILED",
                format!("Civitai 返回了无法解析的内容（本次出网路径：{route}）"),
            )),
        }
    }

    /// 与 [`crate::tools::web_tools`] / [`crate::tools::danbooru`] 同一条代理纪律
    /// （公网走代理、本机/内网直连），超时按 Civitai 的口径。
    fn http_client(&self, host: &str) -> Result<reqwest::Client, ToolFailure> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECONDS))
            .timeout(self.timeout)
            // 与 KeenableSearch 同规矩：不跟随重定向（目标是我们写死的域名，没必要被带走）
            .redirect(reqwest::redirect::Policy::none());
        match crate::infra::outbound::route(host) {
            crate::infra::outbound::Route::Proxy(addr) => {
                let proxy = reqwest::Proxy::all(format!("http://{addr}")).map_err(|e| {
                    ToolFailure::new("CIVITAI_FAILED", format!("代理地址不合法：{e}"))
                })?;
                builder = builder.proxy(proxy);
            }
            crate::infra::outbound::Route::Direct => {
                builder = builder.no_proxy();
            }
        }
        builder
            .build()
            .map_err(|e| ToolFailure::new("CIVITAI_FAILED", format!("HTTP 客户端创建失败：{e}")))
    }
}

/// 取令牌：受管凭据库优先（解不开就当没有，与 Kotlin 的 `runCatching{}.getOrNull()` 同），
/// 再 `CIVITAI_API_KEY` 兜底。
fn resolve_token(state: &AppState) -> Option<String> {
    let managed = crate::mobile::auth::resolve_credential(state, CREDENTIAL_REF)
        .ok()
        .filter(|v| !v.trim().is_empty());
    managed.or_else(|| {
        std::env::var(ENV_TOKEN)
            .ok()
            .filter(|v| !v.trim().is_empty())
    })
}

// ---------------------------------------------------------------------------
//  一条条目 → 结构化字段
// ---------------------------------------------------------------------------

/// 从 `files[primary].hashes` 里取 SHA256。
///
/// 键名大小写不敏感（线上给的是 `SHA256`，别的实现可能写 `sha256`）；
/// **只认 64 位十六进制**，别把 `AutoV2` 那种短码当 sha256 ——
/// 用短码校验盘上文件始终无法匹配，比"未知"更差。
fn primary_sha256(hashes: Option<&Map<String, Value>>) -> Option<String> {
    let raw = hashes?
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("SHA256"))
        .and_then(|(_, value)| value.as_str())?;
    let value = raw.trim();
    crate::tools::find_model::is_sha256(Some(value)).then(|| value.to_string())
}

/// 一个 Civitai 条目 → [`CivitaiModel`]（认不出 id 就丢掉这一条）。
fn to_model(obj: &Map<String, Value>) -> Option<CivitaiModel> {
    let id = int_field(obj, "id")?;
    let name = match str_field(obj, "name") {
        Some(value) => value,
        None => format!("#{id}"),
    };
    let r#type = str_field(obj, "type").unwrap_or_default();
    // 第一个版本里第一个文件就是"主文件"，它的 baseModel 才是"这份模型跑在哪个底模上"——
    // 顶层没有 baseModel 字段，这一步不取的话界面上就是一片空白。
    //
    // 字段名与线上核对过：`items[].modelVersions[0].baseModel` = `"SD 1.5"`，与本条顶层的
    // `baseModels: ["SD 1.5"]` 一致 —— 后者是**数组**（一个模型可以跨多个底模），
    // 所以只在 `modelVersions` 缺失/为空时拿它兜底，别拿它当主来源。
    let first_version = obj
        .get("modelVersions")
        .and_then(Value::as_array)
        .and_then(|versions| versions.first())
        .and_then(Value::as_object);
    let base_model = match first_version.and_then(|v| str_field(v, "baseModel")) {
        Some(value) => value,
        None => obj
            .get("baseModels")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    };
    let trained_words: Vec<String> = first_version
        .and_then(|v| v.get("trainedWords"))
        .and_then(Value::as_array)
        .map(|words| {
            words
                .iter()
                .filter_map(|w| w.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    // 体积与下载地址：线上 `modelVersions[0].files[]` 里 `primary=true` 那条就是主文件，
    // 带 `sizeKB` 与 `downloadUrl`。为什么要它：真实的门槛就是**体积**，
    // 而"要不要装"这个决定没有体积根本没法做。
    //
    // 取舍：只报**主文件**，不在服务端做"挑最小的那个量化版"这类判断 ——
    // 那是模型该做的事（它能看到候选清单），代码只把事实摆出来。
    let files: Vec<&Map<String, Value>> = first_version
        .and_then(|v| v.get("files"))
        .and_then(Value::as_array)
        .map(|files| files.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default();
    let primary = files
        .iter()
        .find(|f| loose_bool(f.get("primary")))
        .copied()
        .or_else(|| files.first().copied());
    let size_kb = primary
        .and_then(|f| f.get("sizeKB"))
        .and_then(double_field)
        .filter(|v| v.is_finite());
    // sha256：线上形如 `files[primary].hashes = {"SHA256":"<64位>","AutoV2":"…"}`。
    // 只在 `hashes` 里找 SHA256 —— 别把 `AutoV2` 当成 sha256 报出去。
    let sha256 = primary_sha256(
        primary
            .and_then(|f| f.get("hashes"))
            .and_then(Value::as_object),
    );
    let stats = obj.get("stats").and_then(Value::as_object);
    Some(CivitaiModel {
        id,
        name,
        r#type,
        base_model,
        nsfw: loose_bool(obj.get("nsfw")),
        creator: obj
            .get("creator")
            .and_then(Value::as_object)
            .and_then(|creator| str_field(creator, "username")),
        downloads: stats
            .and_then(|s| int_field(s, "downloadCount"))
            .unwrap_or(0),
        thumbs_up: stats
            .and_then(|s| int_field(s, "thumbsUpCount"))
            .unwrap_or(0),
        // 链接给**正式站**，不是本次请求用的 base：base 可以被环境变量指到本机假服务 /
        // 自建代理（测试中即如此），将该地址写入返回的链接是错误的
        page_url: format!("{SAFE_BASE_URL}/models/{id}"),
        description: crate::tools::tools::take_utf16(
            crate::tools::tools::java_trim(&collapse_spaces(&strip_tags(
                &str_field(obj, "description").unwrap_or_default(),
            ))),
            MAX_DESCRIPTION_CHARS,
        ),
        trained_words,
        size_bytes: size_kb.map(|kb| (kb * 1024.0) as i64),
        download_url: primary.and_then(|f| str_field(f, "downloadUrl")),
        file_name: primary.and_then(|f| str_field(f, "name")),
        sha256,
    })
}

/// `JsonObject.str`：任意基本类型取字面量，空白算没给。
fn str_field(obj: &Map<String, Value>, key: &str) -> Option<String> {
    let raw = match obj.get(key)? {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    if raw.trim().is_empty() {
        None
    } else {
        Some(raw)
    }
}

/// `JsonPrimitive.intOrNull` 的对应物（数字与数字字符串都认，认不出就是 `None`）。
fn int_field(obj: &Map<String, Value>, key: &str) -> Option<i64> {
    match obj.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

/// `JsonPrimitive.content.toDouble()` 的对应物（数字与数字字符串都认）。
fn double_field(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

/// Kotlin `content.toBoolean()`（**宽松**：忽略大小写的 `"true"` 才算真）。
fn loose_bool(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        Some(Value::Number(n)) => n.to_string().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// Kotlin `Regex("<[^>]+>").replace(text, " ")` 的对应物。
///
/// ⚠ 判据要留住两条细节：`[^>]` **含换行**（所以标签可以跨行匹配），
/// 而 `+` 要求**至少一个字符**（`<>` 不算标签）。
fn strip_tags(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            match chars[i + 1..].iter().position(|c| *c == '>') {
                Some(rel) if rel >= 1 => {
                    out.push(' ');
                    i += rel + 2;
                    continue;
                }
                _ => {
                    out.push('<');
                    i += 1;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 把结果渲染成"条目 + 一句话说明"的块（给模型看的那一份）。
///
/// 需要的是"**简短说明**（例如其用途）"，因此每条固定几行。
/// `tokenless` 且 `nsfw` 时在抬头补一句"本次是匿名通道"：这不是装饰，
/// 而是让模型知道**这次没带登录态**，免得它把"匿名可见"当成"这就是全部"。
fn render(models: &[CivitaiModel], query: &str, nsfw: bool, tokenless: bool) -> String {
    if models.is_empty() {
        return String::new();
    }
    let mut head = format!("Civitai 搜索「{query}」");
    if nsfw {
        head.push_str("（NSFW 通道）");
    }
    head.push_str(&format!("：{} 条", models.len()));
    if nsfw && tokenless {
        head.push_str(
            "\n（本次没带 Civitai 令牌，走的是匿名通道 —— 结果照常可用；\
             要更稳的登录态可以提醒用户去「设置 → AI 工具 / 凭据」填一个，但不必停下来等。）",
        );
    }

    let blocks: Vec<String> = models
        .iter()
        .enumerate()
        .map(|(index, m)| {
            let mut out = format!("[{}] {}", index + 1, m.name);
            out.push_str(&format!(
                "（{}",
                if m.r#type.trim().is_empty() {
                    "未知类型"
                } else {
                    &m.r#type
                }
            ));
            if !m.base_model.trim().is_empty() {
                out.push_str(&format!(" · 底模 {}", m.base_model));
            }
            out.push('）');
            if m.nsfw {
                out.push_str(" [NSFW]");
            }
            out.push('\n');
            out.push_str(&format!(
                "用途：{}",
                if m.description.trim().is_empty() {
                    "（作者没写简介）"
                } else {
                    &m.description
                }
            ));
            out.push('\n');
            if !m.trained_words.is_empty() {
                out.push_str(&format!("触发词：{}", m.trained_words.join(", ")));
                out.push('\n');
            }
            if m.size_bytes.is_some() {
                out.push_str(&format!("体积：{}", m.size_label()));
                out.push('\n');
            }
            out.push_str(&m.page_url);
            out.push_str(&format!("（下载 {} · 赞 {}）", m.downloads, m.thumbs_up));
            if let Some(url) = m.download_url.as_deref() {
                out.push_str(&format!("\n直链：{url}"));
            }
            out
        })
        .collect();
    format!("{head}\n\n{}", blocks.join("\n\n"))
}

// ---------------------------------------------------------------------------
//  工具入口
// ---------------------------------------------------------------------------

/// `civitai_search`：搜 Civitai 的模型 / LoRA 库。
pub async fn tool_search(state: &AppState, arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let Some(query) = str_arg(arguments, "query") else {
        return Err(ToolFailure::new("INVALID_ARGUMENT", "缺少参数 query"));
    };
    let raw_type = str_arg(arguments, "type").map(|value| value.trim().to_string());
    if let Some(raw) = raw_type.as_deref().filter(|raw| !raw.is_empty()) {
        if !TYPES.iter().any(|t| t.eq_ignore_ascii_case(raw)) {
            // 类型写错了就当场说清楚可选值，别让模型拿到一份空结果自己猜
            return Err(ToolFailure::new(
                "INVALID_ARGUMENT",
                format!("type 只能是 {}；收到的是「{raw}」", TYPES.join(" / ")),
            ));
        }
    }
    let type_filter = raw_type
        .as_deref()
        .and_then(|raw| TYPES.iter().find(|t| t.eq_ignore_ascii_case(raw)).copied());

    let client = CivitaiClient::from_env(state)?;
    let outcome = client
        .search(
            &query,
            crate::tools::tools::bool_arg(arguments, "allowNsfw").unwrap_or(false),
            type_filter,
            crate::tools::tools::int_arg(arguments, "limit").unwrap_or(8),
        )
        .await?;
    if outcome.is_empty() {
        return Err(ToolFailure::new(
            "NO_RESULTS",
            format!(
                "Civitai 上没搜到「{query}」。换个关键词再试一次；\
                 如果用户要的是成人内容，确认一下 allowNsfw 打开了 —— \
                 仍然没有就如实告诉用户没搜到，不要编造模型名。"
            ),
        ));
    }
    let content = format!(
        "{}\n\n{}",
        crate::tools::tools::untrusted_banner("Civitai 模型搜索结果", Some(&query)),
        outcome.context
    );
    Ok(ToolOutcome::new(content, outcome.to_json()))
}

#[cfg(test)]
mod tests;
