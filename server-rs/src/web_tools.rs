//! `web_search` / `web_fetch`：Keenable 联网搜索客户端（Kotlin `KeenableSearch.kt` 的逐条对照）。
//!
//! # 为什么是 Keenable
//!
//! 用户 2026-09-20 定的规则：**只用通用方案**（不碰各家的原生内置搜索 —— OpenAI 的
//! `web_search`、Anthropic 的 `web_search_20250305` 那些）。Keenable 是"给 AI Agent
//! 重建的搜索索引"，一次调用就返回排好序的**正文**，关键在于**默认免密钥**。
//!
//! # 三条硬约束（踩过 / 实测过）
//!
//! 1. **免密钥请求必须带 `X-Keenable-Title`**：缺这个头服务端直接 400
//!    （`{"error":"Missing app identifier"}`，实测）。它是归因头，不是密钥。
//! 2. **绝不记录 / 回显密钥**（AIH-051）：错误正文里只允许出现状态码与服务端自己的
//!    message，**一个字符的 key 都不许进**（`SEARCH_CREDENTIAL` 那句也要走同一把闸）。
//! 3. **不跟随重定向**（`Redirect.NEVER`）：`web_fetch` 的目标是模型给的 URL，
//!    照着重定向走等于让它把请求带去任意地方。
//!
//! # 出网前两道闸门（都在这里，别在调用方再补一道）
//!
//! * **`web_fetch` 的目标**：过 [`crate::endpoint_guard::assert_allowed`] + `public` 档 ——
//!   内网 / 回环 / 云元数据地址一律 `FETCH_TARGET_DENIED`，**一个请求都不发**。
//!   系统提示第 17 条那句"不要换写法绕"说的就是这个口子。
//! * **搜索服务自己的地址**：按地址**推断**信任档（回环是给本机假服务 / 自建代理留的），
//!   与 Kotlin 的 `guardBaseHost` 同一个入口。
//!
//! # 两处 JVM 口径的忠实移植（不是"顺手现代化"）
//!
//! * **编码**：Kotlin 用的是 `URLEncoder.encode(_, UTF_8)` —— 它把**空格编成 `+`**
//!   （`application/x-www-form-urlencoded` 口径），保留 `-_.*`。见 [`crate::tools::url_encode`]。
//! * **预算**：`CONTEXT_MAX_CHARS` 那套加减用的是 Kotlin `String.length`，也就是
//!   **UTF-16 单元数**（中文算 1、emoji 算 2）。见 [`utf16_len`]。
//!
//! # 一处与 Kotlin 不同（有意）
//!
//! Kotlin 的错误正文里有 `${e::class.simpleName}`（Java 异常类名）。Rust 没有对应物，
//! 这里用一个**同名的等价分类**（[`crate::tools::java_error_name`]）填回去 —— 文案一个
//! 字符都没改，改的只是那个名字从哪来。

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::routes::AppState;
use crate::tools::{
    java_error_name, outbound_error_kind, url_encode, OutboundError, ToolFailure, ToolOutcome,
};

/// 默认站点（`KEENABLE_API_URL` 可覆盖 —— 这条不只是给测试用的：官方 SDK 也认它）。
pub const DEFAULT_BASE_URL: &str = "https://api.keenable.ai";

/// 可选密钥在凭据库里的引用名（`KeenableSearch.CREDENTIAL_REF`）。
///
/// ⚠ **不认**官方 SDK 的 `KEENABLE_API_KEY`：密钥入口只留一个，界面上的"已配置/未配置"
/// 状态必须与实际行为一致 —— 多认一路环境变量，状态就会说谎。
pub const CREDENTIAL_REF: &str = crate::credentials::KEENABLE_REF;

/// `X-Keenable-Title` 的值：出问题时对方能认出流量来自本项目。
pub const CLIENT_SOURCE: &str = "ComfyHub";

/// 官方免密钥额度，只用于错误提示里的说明文案。
pub const KEYLESS_RATE_LIMIT_PER_HOUR: i64 = 1000;

pub const CONNECT_TIMEOUT_SECONDS: u64 = 5;
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 25;
pub const MAX_TIMEOUT_SECONDS: u64 = 60;

/// 回给模型的搜索结果**总预算**（字符）。
///
/// 取官方 `to_context()` 的默认值 12000：它在那边是经过验证的"够模型引用、又不至于把
/// 上下文塞爆"的档位。[`crate::tools::MAX_RESULT_CHARS`] 是最后一道闸，这里先按预算
/// 整块取舍 —— **宁可不给某一条，也不要给半句话**。
pub const CONTEXT_MAX_CHARS: usize = 12_000;

/// 每条结果向服务端要的正文上限；总预算由 [`CONTEXT_MAX_CHARS`] 兜。
pub const SNIPPET_MAX_LENGTH: i64 = 2_500;

/// 单次 `web_fetch` 返回正文的默认 / 最大字符数。
pub const DEFAULT_FETCH_CHARS: i64 = 20_000;
pub const MAX_FETCH_CHARS: i64 = 60_000;

/// 覆盖站点地址的环境变量（本机假服务 / 企业自建代理用）。
pub const ENV_BASE_URL: &str = "KEENABLE_API_URL";

/// 一条搜索结果（`snippet` 是页面正文抽取，不是 meta description）。
#[derive(Debug, Clone)]
pub struct WebResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub published_at: Option<String>,
}

/// 一次搜索的结果：`context` 是**已经渲染好**的、给模型看的块；
/// 界面要用的话读 `meta` / `results`，不要去解析 `context` 的文本。
#[derive(Debug)]
pub struct WebSearchOutcome {
    pub query: String,
    pub results: Vec<WebResult>,
    pub context: String,
    pub meta: Value,
}

impl WebSearchOutcome {
    pub fn is_empty(&self) -> bool {
        self.results.is_empty()
    }
}

/// 一个被抓取的页面。
#[derive(Debug)]
pub struct WebPage {
    pub url: String,
    pub title: String,
    pub content: String,
    pub published_at: Option<String>,
    pub truncated: bool,
}

/// 一次联网搜索的客户端（唯一出网口）。
///
/// `base` / `key` 都**显式传**（不在这里读环境）—— 用例要能指向本机假服务，
/// 而环境变量是**进程级**的、并行用例之间会互相污染。环境那一层在 [`Self::from_env`]。
pub struct KeenableClient {
    base: String,
    client_source: String,
    timeout: Duration,
    /// **绝不进日志、绝不进错误正文**（AIH-051）。
    key: Option<String>,
}

impl KeenableClient {
    pub fn new(
        base_url: &str,
        client_source: &str,
        timeout_seconds: u64,
        key: Option<String>,
    ) -> Result<Self, ToolFailure> {
        // 末尾斜杠一律去掉：否则拼出来是 `//v1/search/public`
        let base = base_url.trim().trim_end_matches('/').to_string();
        if base.is_empty() {
            return Err(ToolFailure::new(
                "SEARCH_FAILED",
                "搜索服务地址是空的（这是本机的 bug，请如实告诉用户）",
            ));
        }
        Ok(Self {
            base,
            client_source: client_source.to_string(),
            timeout: Duration::from_secs(timeout_seconds.clamp(1, MAX_TIMEOUT_SECONDS)),
            key: key.filter(|k| !k.trim().is_empty()),
        })
    }

    /// 按环境变量与凭据库造一个（`KEENABLE_API_URL` + `keenable` 那把可选 key）。
    pub fn from_env(state: &AppState) -> Result<Self, ToolFailure> {
        let base = std::env::var(ENV_BASE_URL)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let key = resolve_key(state)?;
        Self::new(&base, CLIENT_SOURCE, DEFAULT_TIMEOUT_SECONDS, key)
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub fn is_keyless(&self) -> bool {
        self.key.is_none()
    }

    // -----------------------------------------------------------------------
    //  搜索
    // -----------------------------------------------------------------------

    /// 搜一次网。`query` 用**自然语言描述你想要的那一页**，不要堆关键词。
    pub async fn search(
        &self,
        query: &str,
        site: Option<&str>,
        published_after: Option<&str>,
        published_before: Option<&str>,
        mode: Option<&str>,
    ) -> Result<WebSearchOutcome, ToolFailure> {
        // 手拼 JSON 的**键取舍与顺序**照抄 Kotlin `buildJsonObject`：
        // 可空的 site / published_* 是**省掉键**（不是写 null），mode 永远在（默认 pro）。
        let mut payload = Map::new();
        payload.insert("query".into(), json!(query));
        payload.insert(
            "mode".into(),
            json!(mode.filter(|m| !m.trim().is_empty()).unwrap_or("pro")),
        );
        if let Some(value) = site.filter(|s| !s.trim().is_empty()) {
            payload.insert("site".into(), json!(value));
        }
        if let Some(value) = published_after.filter(|s| !s.trim().is_empty()) {
            payload.insert("published_after".into(), json!(value));
        }
        if let Some(value) = published_before.filter(|s| !s.trim().is_empty()) {
            payload.insert("published_before".into(), json!(value));
        }
        payload.insert("snippet_max_length".into(), json!(SNIPPET_MAX_LENGTH));

        let body = self
            .request("search", "POST", Some(&Value::Object(payload).to_string()), None)
            .await?;

        let results: Vec<WebResult> = body
            .get("results")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let obj = item.as_object()?;
                        Some(WebResult {
                            title: str_of(obj, "title").unwrap_or_default(),
                            url: str_of(obj, "url").unwrap_or_default(),
                            snippet: str_of(obj, "snippet").unwrap_or_default(),
                            published_at: str_of(obj, "published_at"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let context = render(&results);
        let query_out = str_of(&body, "query").unwrap_or_else(|| query.to_string());
        let meta = json!({
            "count": results.len(),
            "query": query_out,
            "keyless": self.key.is_none(),
        });
        Ok(WebSearchOutcome { query: query_out, results, context, meta })
    }

    // -----------------------------------------------------------------------
    //  取正文
    // -----------------------------------------------------------------------

    /// 取一个 URL 的正文（服务端已去掉样板、转成 markdown）。
    ///
    /// 目标 URL 由模型给，所以发出去之前**先过 [`crate::endpoint_guard`]**。
    pub async fn page(&self, url: &str, max_chars: i64) -> Result<WebPage, ToolFailure> {
        let raw = url.trim();
        let (_scheme, host) = split_http_url(raw)?;
        guard_public_host(&host)?;

        let limit = max_chars.clamp(1, MAX_FETCH_CHARS) as usize;
        let query_string = format!("url={}", url_encode(raw));
        let body = self.request("fetch", "GET", None, Some(&query_string)).await?;

        let content = str_of(&body, "content").unwrap_or_default();
        let truncated = utf16_len(&content) > limit;
        let text = if truncated { take_utf16(&content, limit) } else { content };
        Ok(WebPage {
            url: str_of(&body, "url").unwrap_or_else(|| raw.to_string()),
            title: str_of(&body, "title").unwrap_or_default(),
            content: text,
            published_at: str_of(&body, "published_at"),
            truncated,
        })
    }

    // -----------------------------------------------------------------------
    //  出网
    // -----------------------------------------------------------------------

    /// 发一次请求并解析 JSON 对象。
    ///
    /// 免密钥走 `/public` 后缀（带 key 时去掉它），这是官方 SDK 的路径约定：
    /// 同一个端点名，两种调用面。
    async fn request(
        &self,
        endpoint: &str,
        method: &str,
        body: Option<&str>,
        query_string: Option<&str>,
    ) -> Result<Map<String, Value>, ToolFailure> {
        let path = format!("/v1/{endpoint}{}", if self.key.is_none() { "/public" } else { "" });
        let full = match query_string {
            Some(q) => format!("{}{path}?{q}", self.base),
            None => format!("{}{path}", self.base),
        };
        let parsed = reqwest::Url::parse(&full).map_err(|_| {
            ToolFailure::new("SEARCH_FAILED", format!("搜索服务地址无法解析：{}", self.base))
        })?;
        if let Some(host) = parsed.host_str() {
            self.guard_base_host(host)?;
        }
        let host = parsed.host_str().unwrap_or(&self.base).to_string();
        let route = crate::outbound::route(&host).describe();

        let client = self.http_client(&host)?;
        let mut request = client
            .request(
                reqwest::Method::from_bytes(method.as_bytes())
                    .map_err(|_| ToolFailure::new("SEARCH_FAILED", format!("不认识的请求方法：{method}")))?,
                parsed,
            )
            .header("Accept", "application/json")
            // 约束 ①：免密钥请求没有这个头会被服务端 400 掉
            .header("X-Keenable-Title", &self.client_source)
            .timeout(self.timeout);
        if let Some(key) = self.key.as_deref() {
            request = request.header("X-API-Key", key);
        }
        if let Some(body) = body {
            request = request.header("Content-Type", "application/json").body(body.to_string());
        }

        let response = request.send().await.map_err(|e| {
            // 与 CivitaiSearch / FindModel 同一口径：连接超时 ≠ "回得慢"，出网路径也一并写出来。
            // 连接被立刻拒（本机最常见的那种：直连不通）落在最后一档，与 Kotlin 的
            // `ConnectException` 分支同形。
            match outbound_error_kind(&e) {
                OutboundError::ConnectTimeout => ToolFailure::new(
                    "SEARCH_TIMEOUT",
                    format!(
                        "连不上搜索服务：{host} 在 {CONNECT_TIMEOUT_SECONDS} 秒内没连上（本次出网路径：{route}）。\
                         本机若需要代理才能出网，请确认系统代理可用（或设环境变量 {}）",
                        crate::outbound::ENV_PROXY
                    ),
                ),
                OutboundError::ReadTimeout => ToolFailure::new(
                    "SEARCH_TIMEOUT",
                    format!(
                        "联网搜索超时（连上了但 {} 秒没回完），稍后再试；本次出网路径：{route}",
                        self.timeout.as_secs()
                    ),
                ),
                OutboundError::Other => ToolFailure::new(
                    "SEARCH_FAILED",
                    format!(
                        "连不上搜索服务（{}）；本机需要能访问 {host}（本次出网路径：{route}）",
                        java_error_name(&e)
                    ),
                ),
            }
        })?;

        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            // 约束 ②：只带服务端自己的 message，绝不回显我们发出去的 key
            let detail = detail_of(&text);
            let (code, hint) = match status {
                401 | 403 => (
                    "SEARCH_AUTH",
                    "密钥被拒绝；请检查设置里的 Keenable 密钥".to_string(),
                ),
                402 => ("SEARCH_QUOTA", "搜索服务余额不足".to_string()),
                429 => {
                    if self.key.is_none() {
                        (
                            "SEARCH_RATE_LIMIT",
                            format!(
                                "免密钥模式每小时 {KEYLESS_RATE_LIMIT_PER_HOUR} 次，已超出；\
                                 稍后再试，或在设置里填一个 Keenable 密钥提高额度"
                            ),
                        )
                    } else {
                        ("SEARCH_RATE_LIMIT", "该密钥的搜索额度已用尽（429）；稍后再试".to_string())
                    }
                }
                400 => ("SEARCH_BAD_REQUEST", "请求被拒绝（多数是查询本身不合法）".to_string()),
                500..=599 => ("SEARCH_FAILED", format!("搜索服务暂时不可用（{status}）")),
                _ => ("SEARCH_FAILED", format!("搜索服务返回 {status}")),
            };
            let message = if detail.is_empty() { hint } else { format!("{hint}｜返回：{detail}") };
            return Err(ToolFailure::new(code, message));
        }

        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(obj)) => Ok(obj),
            _ => Err(ToolFailure::new("SEARCH_FAILED", "搜索服务返回了无法解析的内容")),
        }
    }

    /// 与 [`crate::danbooru`] 同一条代理纪律（公网走代理、本机/内网直连），
    /// 只是超时按搜索服务的口径（连接 5 秒 / 整次 25 秒）。
    fn http_client(&self, host: &str) -> Result<reqwest::Client, ToolFailure> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECONDS))
            .timeout(self.timeout)
            // 约束 ③：模型给的 URL 不能被重定向带跑
            .redirect(reqwest::redirect::Policy::none());
        match crate::outbound::route(host) {
            crate::outbound::Route::Proxy(addr) => {
                let proxy = reqwest::Proxy::all(format!("http://{addr}"))
                    .map_err(|e| ToolFailure::new("SEARCH_FAILED", format!("代理地址不合法：{e}")))?;
                builder = builder.proxy(proxy);
            }
            crate::outbound::Route::Direct => {
                // 环境变量里的 `HTTPS_PROXY` reqwest **是看的** —— 显式覆盖掉，
                // 否则本机/内网目标会被它接走。
                builder = builder.no_proxy();
            }
        }
        builder
            .build()
            .map_err(|e| ToolFailure::new("SEARCH_FAILED", format!("HTTP 客户端创建失败：{e}")))
    }

    /// 搜索服务自身地址：按地址推断信任级别（回环是给本机假服务 / 自建代理留的口子）。
    fn guard_base_host(&self, host: &str) -> Result<(), ToolFailure> {
        let trust = crate::endpoint_guard::infer_trust(host);
        crate::endpoint_guard::assert_allowed(host, trust)
            .map_err(|e| ToolFailure::new("SEARCH_FAILED", format!("搜索服务地址不被允许：{e}")))
    }
}

/// `web_fetch` 的目标一律按 `public` 判：回环 / 内网 / 云元数据全部拒绝。
pub fn guard_public_host(host: &str) -> Result<(), ToolFailure> {
    crate::endpoint_guard::assert_allowed(host, "public").map_err(|_| {
        ToolFailure::new(
            "FETCH_TARGET_DENIED",
            format!("拒绝抓取 {host}：只允许公开网络地址（内网 / 本机 / 云元数据地址会被拒绝）"),
        )
    })
}

// ---------------------------------------------------------------------------
//  渲染：编号可引用的块
// ---------------------------------------------------------------------------

/// 把结果渲染成"编号 + 标题 + 链接 + 正文"的块。
///
/// 这个形状**照抄官方 SDK 的 `to_context()`**，两个理由：
///  1. 编号引用是模型的天性 —— `[1]` 比裸 URL 更容易被正确引用回正文；
///  2. 它以**整条**为单位取舍，宁可少给一条，也不会给模型半句话。
fn render(results: &[WebResult]) -> String {
    let mut kept: Vec<String> = Vec::new();
    let mut used = 0usize;
    for r in results {
        // 编号按**实际保留的条数**走：中间某条超预算被跳过时不能留下 [1][2][4] 这种空洞，
        // 模型引用的是这里的编号，空洞会让"第 3 条"指向不存在的结果
        let mut header = format!("[{}] {}", kept.len() + 1, if r.title.trim().is_empty() { "(无标题)" } else { &r.title });
        header.push_str(&format!(" ({})", r.url));
        if let Some(published) = r.published_at.as_deref() {
            header.push_str(&format!(" - 发布 {published}"));
        }
        let body = crate::tools::collapse_spaces(&r.snippet);
        let block = if body.is_empty() { header } else { format!("{header}\n{body}") };
        let cost = utf16_len(&block) + if kept.is_empty() { 0 } else { 2 };
        if !kept.is_empty() && used + cost > CONTEXT_MAX_CHARS {
            continue;
        }
        kept.push(block);
        used += cost;
    }
    kept.join("\n\n")
}

/// 服务端错误正文里的 `message` / `error` / `detail`，截断后用（**绝不是我们的 key**）。
fn detail_of(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(obj)) => {
            for key in ["message", "error", "detail"] {
                if let Some(value) = str_of(&obj, key) {
                    return take_utf16(&value, 300);
                }
            }
            String::new()
        }
        _ => take_utf16(text, 300),
    }
}

/// `JsonObject.str`（Kotlin）的对应物：任意 JSON 基本类型取字面量，**空白算没给**，
/// 对象 / 数组 / null 一律 `None`。
fn str_of(obj: &Map<String, Value>, key: &str) -> Option<String> {
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

/// 拆出 `(scheme, host)` —— Java `URI` 那三档判定的对应物。
///
/// 三档文案与 Kotlin **逐字一致**（它们会原样喂回模型，模型据此决定怎么办）：
/// ① 连 URL 都解析不出来 → `url 无法解析：…`；② scheme 不是 http/https →
/// `url 必须是 http/https 链接`；③ 没有主机名 → `url 里没有主机名：…`。
///
/// ⚠ **不能拿 WHATWG 解析器（`reqwest::Url`）判"有没有主机名"**：它会把
/// `http:///path` 解析成 host = `path`（多出来的斜杠被吞掉），而 Java 的 `URI.host`
/// 在那种写法下是 `null` —— 于是该报"没有主机名"的输入会**静默**变成一个真请求。
/// 这里自己按 RFC 3986 的 authority 拆，判据与 Java 一致。
fn split_http_url(raw: &str) -> Result<(String, String), ToolFailure> {
    // Java `URI(String)` 会拒绝空白与控制字符（这里是它的粗粒度对应物）
    if raw.is_empty() || raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ToolFailure::new("INVALID_ARGUMENT", format!("url 无法解析：{raw}")));
    }
    let Some((scheme, rest)) = raw.split_once(':') else {
        return Err(ToolFailure::new("INVALID_ARGUMENT", "url 必须是 http/https 链接"));
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(ToolFailure::new("INVALID_ARGUMENT", "url 必须是 http/https 链接"));
    }
    let no_host = || ToolFailure::new("INVALID_ARGUMENT", format!("url 里没有主机名：{raw}"));
    // authority 必须存在（Java `URI.host` 对 `http:/path` 与 `http:///path` 都是 null）
    let Some(after_slashes) = rest.strip_prefix("//") else { return Err(no_host()) };
    let authority = after_slashes.split(['/', '?', '#']).next().unwrap_or_default();
    // 去掉 user:pass@（Java 的 `URI.host` 也不含它）
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if host_port.starts_with('[') {
        // IPv6：Java 的 `URI.host` **保留方括号**（`EndpointGuard` 那边会自己剥）
        match host_port.split_once(']') {
            Some((inner, _)) => format!("{inner}]"),
            None => host_port.to_string(),
        }
    } else {
        match host_port.rsplit_once(':') {
            // 只有"冒号后面全是数字"才算端口；否则整段都是主机名（IPv6 字面量除外，上面已处理）
            Some((head, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
                head.to_string()
            }
            _ => host_port.to_string(),
        }
    };
    if host.is_empty() {
        return Err(no_host());
    }
    Ok((scheme.to_ascii_lowercase(), host.to_ascii_lowercase()))
}

/// Kotlin `String.length` 数的是 **UTF-16 单元**（中文算 1、emoji 算 2）——
/// 预算加减与 `chars` 字段必须与它同口径。真源在 [`crate::tools::utf16_len`]。
pub use crate::tools::{take_utf16, utf16_len};

/// 解析可选的 Keenable 密钥。
///
/// 解密失败（换过 Windows 用户等）**不能被当成"没有 key"**：那样会静默降级成免密钥，
/// 用户以为在用 key，实际在裸奔。所以只有"**桌面上没有配**"才回 `None`。
pub fn resolve_key(state: &AppState) -> Result<Option<String>, ToolFailure> {
    use axum::http::StatusCode;
    match crate::auth::resolve_credential(state, CREDENTIAL_REF) {
        Ok(value) => Ok(Some(value).filter(|v| !v.trim().is_empty())),
        Err(error) if error.status == StatusCode::NOT_FOUND => Ok(None),
        Err(error) => Err(ToolFailure::new(
            "SEARCH_CREDENTIAL",
            format!(
                "联网搜索的密钥无法解密（{}）；请在设置里重新填写，或移除它以改用免密钥模式",
                error.human()
            ),
        )),
    }
}

// ---------------------------------------------------------------------------
//  工具入口
// ---------------------------------------------------------------------------

/// `web_search`：搜一次网（结果带编号与链接，一律当**不可信数据**）。
pub async fn tool_search(state: &AppState, arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let Some(query) = crate::tools::str_arg(arguments, "query") else {
        return Err(ToolFailure::new("INVALID_ARGUMENT", "缺少参数 query"));
    };
    let client = KeenableClient::from_env(state)?;
    let outcome = client
        .search(
            &query,
            crate::tools::str_arg(arguments, "site").as_deref(),
            crate::tools::str_arg(arguments, "publishedAfter").as_deref(),
            crate::tools::str_arg(arguments, "publishedBefore").as_deref(),
            None,
        )
        .await?;
    if outcome.is_empty() {
        return Err(ToolFailure::new(
            "NO_RESULTS",
            format!(
                "「{query}」没有搜到结果。换个说法再试（描述得更像你想读的文章），\
                 或者告诉用户你没搜到 —— 不要凭记忆编造。"
            ),
        ));
    }
    let content = format!(
        "{}\n\n{}",
        crate::tools::untrusted_banner("联网搜索结果", Some(&query)),
        outcome.context
    );
    Ok(ToolOutcome::new(content, outcome.meta))
}

/// `web_fetch`：把一个公开网页取回来读全文。
pub async fn tool_fetch(state: &AppState, arguments: &Value) -> Result<ToolOutcome, ToolFailure> {
    let Some(url) = crate::tools::str_arg(arguments, "url") else {
        return Err(ToolFailure::new("INVALID_ARGUMENT", "缺少参数 url"));
    };
    let client = KeenableClient::from_env(state)?;
    let max_chars = crate::tools::int_arg(arguments, "maxChars").unwrap_or(DEFAULT_FETCH_CHARS);
    let page = client.page(&url, max_chars).await?;

    let mut head = String::new();
    head.push_str(&crate::tools::untrusted_banner("网页正文", None));
    head.push_str("\n\n");
    head.push_str(&format!(
        "# {}\n{}",
        if page.title.trim().is_empty() { "(无标题)" } else { &page.title },
        page.url
    ));
    if let Some(published) = page.published_at.as_deref() {
        head.push_str(&format!("（发布 {published}）"));
    }
    let tail = if page.truncated { "\n\n…（正文超长已截断）" } else { "" };
    let content = format!("{head}\n\n{}{tail}", page.content);
    let payload = json!({
        "url": page.url,
        "title": page.title,
        "chars": utf16_len(&page.content),
        "truncated": page.truncated,
    });
    Ok(ToolOutcome::new(content, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    //  纯函数
    // -----------------------------------------------------------------------

    /// 编码是 **Java `URLEncoder`** 的口径：空格变 `+`（不是 `%20`）、`*` 保留。
    ///
    /// 这一条错了**不会报错**：`url=https%3A%2F%2F…` 那一段依然"看起来对"，
    /// 只是有些目标抓不回来。
    #[test]
    fn 编码是_java_urlencoder_的口径() {
        assert_eq!(url_encode("Most Downloaded"), "Most+Downloaded");
        assert_eq!(url_encode("a b"), "a+b");
        assert_eq!(url_encode("https://example.com/page?id=42&lang=zh"),
                   "https%3A%2F%2Fexample.com%2Fpage%3Fid%3D42%26lang%3Dzh");
        assert_eq!(url_encode("水墨风"), "%E6%B0%B4%E5%A2%A8%E9%A3%8E");
        // `-_.*` 是 Java 明确保留的那几个
        assert_eq!(url_encode("krea2_turbo-int8.safetensors*"), "krea2_turbo-int8.safetensors*");
        // 大写十六进制（小写会被某些服务端拒）
        assert_eq!(url_encode("~"), "%7E");
    }

    #[test]
    fn 空白压平与_java_trim_都是_ascii_口径() {
        assert_eq!(crate::tools::collapse_spaces("  把照片转成\n\n <b>水墨画</b> 风格  "), "把照片转成 <b>水墨画</b> 风格");
        assert_eq!(crate::tools::collapse_spaces("\t\n"), "");
        // Java 的 `String.trim()` 只去 `<= U+0020`：全角空格**不去**（`\s` 也不认它）
        assert_eq!(crate::tools::java_trim("  x\u{3000}  "), "x\u{3000}");
    }

    #[test]
    fn 正文截断按_utf16_单元数() {
        assert_eq!(utf16_len("水墨风"), 3);
        assert_eq!(utf16_len("🙂"), 2, "emoji 在 Kotlin 的 String.length 里算 2");
        assert_eq!(take_utf16("水墨风", 2), "水墨");
        // 不能把一个代理对切成半个字符
        assert_eq!(take_utf16("a🙂b", 2), "a");
        assert_eq!(take_utf16("abc", 9), "abc");
    }

    #[test]
    fn 服务端错误正文按_message_error_detail_取且截到_300() {
        assert_eq!(detail_of(r#"{"message":"先看 message"}"#), "先看 message");
        assert_eq!(detail_of(r#"{"error":"再看 error"}"#), "再看 error");
        assert_eq!(detail_of(r#"{"detail":"最后 detail"}"#), "最后 detail");
        assert_eq!(detail_of(r#"{"message":"","error":"空的不算"}"#), "空的不算");
        // 一个都没有 → 空串（调用方据此省掉"｜返回："那一段）
        assert_eq!(detail_of(r#"{"ok":true}"#), "");
        // 解析不出来 → 原样截断
        assert_eq!(detail_of("boom"), "boom");
        assert_eq!(detail_of(&"字".repeat(400)).chars().count(), 300);
    }

    /// 渲染是**按整条取舍**的：超预算的那条整个跳过，编号不许留空洞。
    #[test]
    fn 渲染按整条取舍且编号无空洞() {
        let results = vec![
            WebResult { title: "标题一".into(), url: "https://a/1".into(), snippet: "片段一".into(), published_at: Some("2026-08-01".into()) },
            WebResult { title: "".into(), url: "https://a/2".into(), snippet: " 多  空白\n片段 ".into(), published_at: None },
        ];
        let text = render(&results);
        assert_eq!(
            text,
            "[1] 标题一 (https://a/1) - 发布 2026-08-01\n片段一\n\n[2] (无标题) (https://a/2)\n多 空白 片段"
        );

        // 塞一条超预算的进去：它被跳过，后面的编号接着数（[2] 而不是 [3]）
        let huge = WebResult { title: "巨大".into(), url: "https://a/huge".into(), snippet: "字".repeat(CONTEXT_MAX_CHARS), published_at: None };
        let skipped = render(&[results[0].clone(), huge, results[1].clone()]);
        assert!(skipped.contains("[2] (无标题) (https://a/2)"), "{skipped}");
        assert!(!skipped.contains("巨大"), "超预算的那条整个不给：{skipped}");

        // 第一条自己也超预算时**照样留着**（`kept.isEmpty()` 那一档，与 Kotlin 同）
        let only = render(&[WebResult { title: "长".into(), url: "u".into(), snippet: "字".repeat(CONTEXT_MAX_CHARS * 2), published_at: None }]);
        assert!(only.starts_with("[1] 长 (u)"));
    }

    /// `web_fetch` 的三档 URL 报错与 Kotlin 逐字一致（模型靠它自己改对写法）。
    #[test]
    fn url_三档判定的文案与_kotlin_一致() {
        assert_eq!(split_http_url("http://a b/x").unwrap_err().code, "INVALID_ARGUMENT");
        assert_eq!(split_http_url("http://a b/x").unwrap_err().message, "url 无法解析：http://a b/x");
        assert_eq!(split_http_url("ftp://example.com/x").unwrap_err().message, "url 必须是 http/https 链接");
        assert_eq!(split_http_url("notaurl").unwrap_err().message, "url 必须是 http/https 链接");
        assert_eq!(split_http_url("http:///path").unwrap_err().message, "url 里没有主机名：http:///path");
        assert_eq!(split_http_url("http:/path").unwrap_err().message, "url 里没有主机名：http:/path");
        assert_eq!(
            split_http_url("http://user:pass@Example.COM:8080/a?b#c").unwrap(),
            ("http".to_string(), "example.com".to_string()),
            "user:pass@ 与端口都要去掉，主机名小写"
        );
        assert_eq!(split_http_url("https://[::1]:8188/x").unwrap().1, "[::1]", "IPv6 保留方括号（Java URI 同）");
        assert_eq!(split_http_url("https://Example.COM/a").unwrap(), ("https".to_string(), "example.com".to_string()));
    }

    /// 内网 / 回环 / 云元数据**一律拒绝**（这是 `web_fetch` 唯一的安全边界）。
    #[test]
    fn 取正文的目标不许是内网与本机() {
        for host in ["127.0.0.1", "localhost", "192.168.1.1", "169.254.169.254", "metadata.google.internal", "0.0.0.0", "[::1]"] {
            let err = guard_public_host(host).unwrap_err();
            assert_eq!(err.code, "FETCH_TARGET_DENIED", "{host}");
            assert!(err.message.contains("只允许公开网络地址"), "{host}: {}", err.message);
        }
        assert!(guard_public_host("example.com").is_ok());
        assert!(guard_public_host("8.8.8.8").is_ok());
    }

    /// 搜索服务**自己**的地址按推断档判：回环放行（本机假服务 / 自建代理），
    /// 云元数据仍然拒（那条红线任何档位都不放）。
    #[test]
    fn 搜索服务地址按推断档判() {
        let local = KeenableClient::new("http://127.0.0.1:9/api", CLIENT_SOURCE, 5, None).unwrap();
        assert_eq!(local.base_url(), "http://127.0.0.1:9/api", "末尾斜杠要去掉");
        assert!(local.guard_base_host("127.0.0.1").is_ok());
        assert!(local.guard_base_host("api.keenable.ai").is_ok());
        let err = local.guard_base_host("169.254.169.254").unwrap_err();
        assert_eq!(err.code, "SEARCH_FAILED");
        assert!(err.message.contains("搜索服务地址不被允许"), "{}", err.message);
        // 空地址：本机的 bug，如实报（Kotlin 那边会抛一个非 ToolFailure 的异常）
        assert!(KeenableClient::new("   ", CLIENT_SOURCE, 5, None).is_err());
    }

    // -----------------------------------------------------------------------
    //  真 socket（进程内假 Keenable，**不出网**）
    // -----------------------------------------------------------------------

    use std::sync::{Arc, Mutex};

    use axum::extract::{Request, State as AxumState};
    use axum::response::{IntoResponse, Response};
    use axum::Router;

    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        path: String,
        query: String,
        title: Option<String>,
        api_key: Option<String>,
        body: String,
    }

    #[derive(Clone)]
    struct Fake {
        seen: Arc<Mutex<Vec<Seen>>>,
        status: u16,
        body: String,
    }

    /// 起一台进程内的假 Keenable，返回 `(base, 记录)`。
    async fn fake_keenable(status: u16, body: &str) -> (String, Arc<Mutex<Vec<Seen>>>) {
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
        let fake = Fake { seen: seen.clone(), status, body: body.to_string() };
        let app = Router::new().fallback(axum::routing::any(
            |AxumState(f): AxumState<Fake>, request: Request| async move {
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, 4 * 1024 * 1024).await.unwrap_or_default();
                f.seen.lock().unwrap().push(Seen {
                    method: parts.method.to_string(),
                    path: parts.uri.path().to_string(),
                    query: parts.uri.query().unwrap_or_default().to_string(),
                    title: parts
                        .headers
                        .get("x-keenable-title")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string),
                    api_key: parts
                        .headers
                        .get("x-api-key")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string),
                    body: String::from_utf8_lossy(&bytes).to_string(),
                });
                Response::builder()
                    .status(f.status)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(f.body.clone()))
                    .unwrap()
                    .into_response()
            },
        ))
        .with_state(fake);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("绑定假 Keenable");
        let addr = listener.local_addr().expect("端口");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("假 Keenable 服务");
        });
        std::mem::forget(handle);
        (format!("http://{addr}"), seen)
    }

    const SEARCH_BODY: &str = r#"{"query":"flux 提示词技巧","mode":"pro","results":[
      {"title":"Flux 提示词指南","url":"https://example.com/flux","snippet":"正文片段一","published_at":"2026-08-01"},
      {"title":"第二条","url":"https://example.com/2","snippet":"片段二"}]}"#;
    // ⚠ 正文里有 `"#`（markdown 的 `# 全文`），所以裸字符串要用 `r##"…"##` 收尾
    const FETCH_BODY: &str = r##"{"url":"https://example.com/flux","title":"Flux 指南","content":"# 全文\n正文内容"}"##;

    fn client(base: &str, key: Option<&str>) -> KeenableClient {
        KeenableClient::new(base, CLIENT_SOURCE, 10, key.map(str::to_string)).expect("客户端")
    }

    /// 免密钥：路径必须带 `/public`、必须带 `X-Keenable-Title`、**不许**带 `X-API-Key`。
    #[tokio::test]
    async fn 免密钥搜索打_public_并带标题头() {
        let (base, seen) = fake_keenable(200, SEARCH_BODY).await;
        let outcome = client(&base, None).search("flux 提示词技巧", None, None, None, None).await.unwrap();

        let hits = seen.lock().unwrap().clone();
        assert_eq!(hits.len(), 1, "一次搜索只该发一个请求：{hits:?}");
        let hit = &hits[0];
        assert_eq!(hit.method, "POST");
        assert_eq!(hit.path, "/v1/search/public", "免密钥必须走 /public");
        assert_eq!(hit.title.as_deref(), Some(CLIENT_SOURCE), "缺这个头会被服务端 400");
        assert_eq!(hit.api_key, None, "免密钥请求不该带 X-API-Key");
        assert!(hit.body.contains("\"query\""), "{}", hit.body);
        assert!(hit.body.contains("flux"), "{}", hit.body);
        assert!(hit.body.contains("\"mode\":\"pro\""), "mode 默认 pro：{}", hit.body);
        assert!(hit.body.contains("\"snippet_max_length\":2500"), "{}", hit.body);
        assert!(!hit.body.contains("site"), "没给的键不许出现：{}", hit.body);

        assert_eq!(outcome.results.len(), 2);
        assert!(outcome.context.contains("[1] Flux 提示词指南 (https://example.com/flux)"), "{}", outcome.context);
        assert!(outcome.context.contains("- 发布 2026-08-01"), "{}", outcome.context);
        assert!(outcome.context.contains("正文片段一"), "{}", outcome.context);
        assert_eq!(outcome.meta["keyless"], true);
        assert_eq!(outcome.meta["count"], 2);
        assert_eq!(outcome.meta["query"], "flux 提示词技巧");
        assert!(!outcome.is_empty());
    }

    /// 配了 key：走 `/v1/search`（**不带 `/public`**）并带 `X-API-Key`。
    #[tokio::test]
    async fn 带密钥走非_public_路径() {
        let (base, seen) = fake_keenable(200, SEARCH_BODY).await;
        let outcome = client(&base, Some("keen_test")).search("任意查询", None, None, None, None).await.unwrap();
        let hit = seen.lock().unwrap()[0].clone();
        assert_eq!(hit.path, "/v1/search", "带 key 时不能走 /public");
        assert_eq!(hit.api_key.as_deref(), Some("keen_test"));
        assert_eq!(outcome.meta["keyless"], false);
    }

    /// 可选参数（site / 日期）只在给了的时候进 body。
    #[tokio::test]
    async fn 可选参数只在给了的时候进_body() {
        let (base, seen) = fake_keenable(200, SEARCH_BODY).await;
        client(&base, None)
            .search("q", Some("arxiv.org"), Some("2026-01-01"), Some("2026-02-01"), None)
            .await
            .unwrap();
        let body = seen.lock().unwrap()[0].body.clone();
        assert!(body.contains("\"site\":\"arxiv.org\""), "{body}");
        assert!(body.contains("\"published_after\":\"2026-01-01\""), "{body}");
        assert!(body.contains("\"published_before\":\"2026-02-01\""), "{body}");
    }

    /// 429：免密钥说清额度与出路；带 key 说清是密钥额度 —— **两处都绝不回显密钥**。
    #[tokio::test]
    async fn 限流的两种文案且绝不回显密钥() {
        let (base, _) = fake_keenable(429, r#"{"error":"quota"}"#).await;
        let err = client(&base, None).search("q", None, None, None, None).await.unwrap_err();
        assert_eq!(err.code, "SEARCH_RATE_LIMIT");
        assert!(err.message.contains("1000"), "要说明免密钥额度：{}", err.message);
        assert!(err.message.contains("｜返回：quota"), "服务端的 message 要带上：{}", err.message);

        let (base, _) = fake_keenable(429, r#"{"message":"too many"}"#).await;
        let err = client(&base, Some("keen_secret_value")).search("q", None, None, None, None).await.unwrap_err();
        assert_eq!(err.code, "SEARCH_RATE_LIMIT");
        assert!(err.message.contains("额度"), "带 key 时要说清是密钥额度用尽：{}", err.message);
        assert!(!err.message.contains("keen_secret_value"), "绝不回显密钥：{}", err.message);
    }

    /// 其余状态码各自映射到稳定码（模型按码分支，不是按那句话）。
    #[tokio::test]
    async fn 状态码映射到稳定错误码() {
        for (status, expected) in [
            (401_u16, "SEARCH_AUTH"),
            (403, "SEARCH_AUTH"),
            (402, "SEARCH_QUOTA"),
            (400, "SEARCH_BAD_REQUEST"),
            (503, "SEARCH_FAILED"),
            (418, "SEARCH_FAILED"),
        ] {
            let (base, _) = fake_keenable(status, r#"{"detail":"why"}"#).await;
            let err = client(&base, None).search("q", None, None, None, None).await.unwrap_err();
            assert_eq!(err.code, expected, "status={status}: {}", err.message);
        }
        // 2xx 但正文不是 JSON 对象
        for body in ["<html>Just a moment...</html>", "[]", "{}"] {
            let (base, _) = fake_keenable(200, body).await;
            let outcome = client(&base, None).search("q", None, None, None, None).await;
            if body == "{}" {
                // 空对象是合法的（就是"一条都没有"）—— 调用方按 NO_RESULTS 处理
                assert!(outcome.unwrap().is_empty());
            } else {
                let err = outcome.unwrap_err();
                assert_eq!(err.code, "SEARCH_FAILED", "{body}");
                assert!(err.message.contains("无法解析"), "{body}: {}", err.message);
            }
        }
    }

    /// `web_fetch`：GET `/v1/fetch/public`，目标 URL 编进查询串（`url=` 那一段是钉住的）。
    #[tokio::test]
    async fn 取正文把目标_url_编码进查询串() {
        let (base, seen) = fake_keenable(200, FETCH_BODY).await;
        let page = client(&base, None).page("https://example.com/page?id=42&lang=zh", 20_000).await.unwrap();
        let hit = seen.lock().unwrap()[0].clone();
        assert_eq!(hit.path, "/v1/fetch/public");
        assert_eq!(hit.method, "GET");
        assert_eq!(hit.title.as_deref(), Some(CLIENT_SOURCE));
        assert!(
            hit.query.starts_with("url=https%3A%2F%2Fexample.com%2Fpage%3Fid%3D42%26lang%3Dzh"),
            "目标 URL 必须整段编码：{}",
            hit.query
        );
        assert_eq!(page.title, "Flux 指南");
        assert!(page.content.contains("正文内容"), "{}", page.content);
        assert!(!page.truncated);
        assert_eq!(page.url, "https://example.com/flux", "服务端给了归一后的 url 就用它");
    }

    /// 正文超上限：切到上限并**如实标 truncated**（不许假装给全了）。
    #[tokio::test]
    async fn 取正文超上限时截断并标出来() {
        let long = "字".repeat(100);
        let body = format!(r#"{{"url":"https://example.com/x","title":"t","content":"{long}"}}"#);
        let (base, _) = fake_keenable(200, &body).await;
        let page = client(&base, None).page("https://example.com/x", 10).await.unwrap();
        assert_eq!(page.content.chars().count(), 10);
        assert!(page.truncated);
        // 上限被夹到 [1, 60000]
        let (base, _) = fake_keenable(200, &body).await;
        let page = client(&base, None).page("https://example.com/x", 0).await.unwrap();
        assert_eq!(page.content.chars().count(), 1, "下限是 1");
    }

    /// **被拒的目标一个请求都不发**（这条是 SSRF 防线的回归）。
    #[tokio::test]
    async fn 内网目标在出网之前就被拒() {
        let (base, seen) = fake_keenable(200, FETCH_BODY).await;
        let client = client(&base, None);
        for target in [
            "http://127.0.0.1:9222/x",
            "http://192.168.1.1/x",
            "http://169.254.169.254/latest/meta-data",
            "http://localhost:8188/object_info",
        ] {
            let err = client.page(target, 1000).await.unwrap_err();
            assert_eq!(err.code, "FETCH_TARGET_DENIED", "{target}");
        }
        assert!(seen.lock().unwrap().is_empty(), "被拒的目标一个请求都不该发出去");
    }

    /// 连不上：**本机最常见的形态**是连接被立刻拒（不是超时）—— 文案要说清"该配代理"。
    #[tokio::test]
    async fn 连不上时报出出网路径而不是假装没搜到() {
        // 127.0.0.1 上一个没有服务的端口：连接被立刻拒绝
        let err = client("http://127.0.0.1:1", None)
            .search("q", None, None, None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code, "SEARCH_FAILED");
        assert!(err.message.contains("连不上搜索服务"), "{}", err.message);
        assert!(err.message.contains("本次出网路径"), "要写清本次是直连还是代理：{}", err.message);
        assert!(!err.message.contains("没有搜到"), "不许把'没查成'说成'没搜到'：{}", err.message);
    }
}
