//! Provider 级的**额外请求参数**：自定义请求头与查询参数。
//!
//! 有些 OpenAI 兼容网关在标准鉴权之外还要求固定的请求头（例如 OpenCode Go / Zen
//! 要求 `x-opencode-session`）或查询参数（例如 Azure OpenAI 的 `api-version`）。
//! 这些要求随网关而异，写死任何一个都会把下一家挡在门外，所以统一在 Provider 上
//! 开两个**名称与值都自填**的字典：
//!
//! * 请求头存在 `ai_providers.headers_json`（`{"X-Foo":"bar"}`）；
//! * 查询参数存在 `ai_providers.compat_json` 的 `query` 键下（`{"query":{"api-version":"…"}}`）。
//!
//! 两列在桌面库与手机库里**早就存在**（Kotlin 时代留的），本期只是把它们接上，
//! 所以没有 schema 版本变化。
//!
//! # 三条纪律
//!
//! 1. **只在写侧校验**：名称/值在这里逐条检查，落库的一定是能发出去的值；
//!    读侧遇到坏 JSON 一律当空并打日志（不能让一行坏数据把整页 Provider 打不开）。
//! 2. **自定义头覆盖同名默认头**：`authorization` / `x-api-key` / `accept` 这些由协议
//!    生成的默认头先放，自定义头后放并覆盖 —— 网关要求换一种鉴权写法时才有路可走。
//! 3. **绝不记录这些值**：日志里只记名称与条数（值里可能有会话标识或密钥）。
//!
//! 额外参数作用于这个 Provider 的**每一条**出网请求：对话、连接测试、获取模型、
//! 思考档位校准。四处共用 [`RequestExtras::apply_headers`] 与
//! [`RequestExtras::with_query`]，不要再各拼一份。
//!
//! # 值里的变量（按对话稳定的标识）
//!
//! 有些头必须**每个会话一个**值（OpenCode 的 `x-opencode-session` 就是：
//! 同一对话内固定，不同对话必须不同）。写死一个固定值会让所有对话挤进同一个
//! 上游会话；这里改成两个占位符，发请求前才展开：
//!
//! | 占位符 | 展开成 |
//! | --- | --- |
//! | `{{session}}` | 26 个小写十六进制字符：`sha256(对话 ID)` 的前 13 个字节 |
//! | `{{conversation}}` | 对话 ID 本身（探测等"没有对话"的请求为 `standalone`） |
//!
//! `{{session}}` **只由对话 ID 决定**：同一对话（含重试、工具轮、重新打开）
//! 永远是同一个值，不同对话一定不同；它也是不透明的，不含供应商前缀 ——
//! OpenCode 的写法是 `ses_{{session}}`（前缀由用户填，程序不认识任何厂商）。
//!
//! 这两条替换在**发请求时**做，存进库里的仍是模板原文：手机同步过去之后，
//! 每台设备用自己的对话 ID 展开，不会把桌面那台的会话标识带过去。

use std::collections::{BTreeMap, BTreeSet};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use sha2::{Digest, Sha256};

use crate::web::routes::ApiError;

/// 额外请求头的条数上限（查询参数各自独立计算）。
pub const MAX_ENTRIES: usize = 32;
/// 单个名称的长度上限。
pub const MAX_NAME_CHARS: usize = 128;
/// 单个值的长度上限。
pub const MAX_VALUE_CHARS: usize = 1024;

/// "每个对话一个稳定标识"的占位符；由 [`RequestExtras::resolve`] 在发请求前展开。
pub const SESSION_PLACEHOLDER: &str = "{{session}}";
/// 对话 ID 本身的占位符。
pub const CONVERSATION_PLACEHOLDER: &str = "{{conversation}}";
/// 没有对话的请求（连接测试 / 模型发现 / 思考校准 / 布局翻译）用的固定作用域。
pub const STANDALONE_SCOPE: &str = "standalone";

/// 由 HTTP 客户端自己生成、不允许用户覆盖的头（覆盖它们只会让请求发不出去）。
const FORBIDDEN_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "upgrade",
    "expect",
    "te",
    "trailer",
    "proxy-connection",
    "keep-alive",
];

/// 一个 Provider 的额外请求参数（空表 = 不加任何东西）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestExtras {
    /// 请求头：名称 → 值（保留用户填写的名称大小写，比较时不区分大小写）。
    pub headers: BTreeMap<String, String>,
    /// URL 查询参数：名称 → 值（追加在路径之后，已有查询参数不会被覆盖）。
    pub query: BTreeMap<String, String>,
}

impl RequestExtras {
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty() && self.query.is_empty()
    }

    /// 从两个库列读出额外参数。坏 JSON / 非对象一律当空（并留一行日志）。
    pub fn from_columns(headers_json: Option<&str>, compat_json: Option<&str>) -> Self {
        Self {
            headers: parse_string_object(headers_json, "headers_json"),
            query: query_from_compat(compat_json),
        }
    }

    /// 把值里的 `{{session}}` / `{{conversation}}` 展开成**这一次请求**的值。
    ///
    /// `conversation_id` 为空（连接测试、思考校准、布局翻译）时用
    /// [`STANDALONE_SCOPE`]：这些请求没有对话，同一个固定值就够了。
    ///
    /// ⚠ 只在发请求前调用；库里的 `headers_json` / `compat_json` 永远存**模板原文**，
    /// 这样手机同步过去之后用的是手机自己那条对话的标识。
    pub fn resolve(&self, conversation_id: Option<&str>) -> Self {
        let scope = conversation_id
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .unwrap_or(STANDALONE_SCOPE);
        let session = session_token(scope);
        let expand = |value: &str| -> String {
            value
                .replace(SESSION_PLACEHOLDER, &session)
                .replace(CONVERSATION_PLACEHOLDER, scope)
        };
        Self {
            headers: self
                .headers
                .iter()
                .map(|(name, value)| (name.clone(), expand(value)))
                .collect(),
            query: self
                .query
                .iter()
                .map(|(name, value)| (name.clone(), expand(value)))
                .collect(),
        }
    }

    /// 写侧校验：条数、名称形态、值的可发送性、同名（不区分大小写）重复。
    ///
    /// 空名称 / 空值不算错误：界面上一行没填完时由调用方自己丢掉整行；
    /// 走到这里说明客户端明确提交了它，报错比静默丢掉更容易查。
    pub fn validated(
        headers: BTreeMap<String, String>,
        query: BTreeMap<String, String>,
    ) -> Result<Self, ApiError> {
        if headers.len() > MAX_ENTRIES {
            return Err(config_error(format!(
                "额外请求头最多 {MAX_ENTRIES} 条，现在有 {} 条。",
                headers.len()
            )));
        }
        if query.len() > MAX_ENTRIES {
            return Err(config_error(format!(
                "额外查询参数最多 {MAX_ENTRIES} 条，现在有 {} 条。",
                query.len()
            )));
        }

        let mut seen: BTreeSet<String> = BTreeSet::new();
        for (name, value) in &headers {
            if name.trim().is_empty() {
                return Err(config_error("额外请求头的名称不能为空。"));
            }
            if name.trim() != name {
                return Err(config_error(format!(
                    "额外请求头「{name}」的名称首尾不能有空格。"
                )));
            }
            if name.chars().count() > MAX_NAME_CHARS {
                return Err(config_error(format!(
                    "额外请求头「{name}」的名称不能超过 {MAX_NAME_CHARS} 个字符。"
                )));
            }
            if !name.chars().all(is_header_name_char) {
                return Err(config_error(format!(
                    "额外请求头「{name}」的名称不合法：只能用字母、数字和 !#$%&'*+-.^_`|~。"
                )));
            }
            let lower = name.to_ascii_lowercase();
            if FORBIDDEN_HEADERS.contains(&lower.as_str()) {
                return Err(config_error(format!(
                    "额外请求头「{name}」由程序生成，不能自定义。"
                )));
            }
            if !seen.insert(lower) {
                return Err(config_error(format!(
                    "额外请求头「{name}」重复了（名称不区分大小写）。"
                )));
            }
            validate_header_value(name, value)?;
            validate_template(name, value)?;
        }

        for (name, value) in &query {
            if name.trim().is_empty() {
                return Err(config_error("额外查询参数的名称不能为空。"));
            }
            if name.chars().count() > MAX_NAME_CHARS {
                return Err(config_error(format!(
                    "额外查询参数「{name}」的名称不能超过 {MAX_NAME_CHARS} 个字符。"
                )));
            }
            if name
                .chars()
                .any(|c| c.is_control() || c == '#' || c == '&' || c == '=')
            {
                return Err(config_error(format!(
                    "额外查询参数「{name}」的名称不能包含控制字符或 # & =。"
                )));
            }
            if value.chars().count() > MAX_VALUE_CHARS {
                return Err(config_error(format!(
                    "额外查询参数「{name}」的值不能超过 {MAX_VALUE_CHARS} 个字符。"
                )));
            }
            if value.chars().any(|c| c == '\0' || c == '\r' || c == '\n') {
                return Err(config_error(format!(
                    "额外查询参数「{name}」的值不能包含换行或空字符。"
                )));
            }
            validate_template(name, value)?;
        }

        Ok(Self { headers, query })
    }

    /// 写回 `headers_json`（空表 → `None`，库列留 NULL）。
    pub fn headers_json(&self) -> Option<String> {
        (!self.headers.is_empty()).then(|| string_object_json(&self.headers))
    }

    /// 写回 `compat_json`：只写本模块负责的 `query` 键（空表 → `None`）。
    pub fn compat_json(&self) -> Option<String> {
        if self.query.is_empty() {
            return None;
        }
        let mut root = serde_json::Map::new();
        root.insert(
            "query".into(),
            serde_json::Value::Object(
                self.query
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect(),
            ),
        );
        Some(serde_json::Value::Object(root).to_string())
    }

    /// 把查询参数追加到 URL 上。
    ///
    /// 追加而不是替换：Base URL 本身不允许带查询参数（`normalize_base_url` 挡着），
    /// 但模型列表那种"候选地址"可能自带查询串，覆盖会把它弄坏。
    pub fn with_query(&self, url: &str) -> Result<String, ApiError> {
        if self.query.is_empty() {
            return Ok(url.to_string());
        }
        let mut parsed =
            url::Url::parse(url).map_err(|e| config_error(format!("上游地址无法解析：{e}")))?;
        {
            let mut pairs = parsed.query_pairs_mut();
            for (name, value) in &self.query {
                pairs.append_pair(name, value);
            }
        }
        Ok(parsed.into())
    }

    /// 把自定义请求头写进已经放好默认头的 [`HeaderMap`]（同名覆盖）。
    pub fn apply_headers(&self, headers: &mut HeaderMap) -> Result<(), ApiError> {
        for (name, value) in &self.headers {
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| config_error(format!("额外请求头「{name}」的名称无法发送。")))?;
            let header_value = HeaderValue::from_str(value).map_err(|_| {
                config_error(format!(
                    "额外请求头「{name}」的值无法发送：只能是可见 ASCII 字符。"
                ))
            })?;
            headers.insert(header_name, header_value);
        }
        Ok(())
    }

    /// 日志用：只报条数与名称，**绝不带值**。
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.headers.is_empty() {
            parts.push(format!(
                "请求头 {} 条（{}）",
                self.headers.len(),
                self.headers.keys().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        if !self.query.is_empty() {
            parts.push(format!(
                "查询参数 {} 条（{}）",
                self.query.len(),
                self.query.keys().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        if parts.is_empty() {
            "无".to_string()
        } else {
            parts.join("；")
        }
    }
}

fn config_error(message: impl Into<String>) -> ApiError {
    ApiError::ai("CONFIG_ERROR", message.into())
}

/// `sha256(scope)` 的前 13 个字节（26 个小写十六进制字符）。
///
/// 长度选 26 是为了让 `ses_{{session}}` 满足 OpenCode 那类网关对
/// `ses_<12 hex><14 alnum>` 的形态要求；对其它网关它就是一个不透明标识。
pub fn session_token(scope: &str) -> String {
    let digest = Sha256::digest(scope.as_bytes());
    let mut out = String::with_capacity(26);
    for byte in digest.iter().take(13) {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 值里的 `{{…}}` 只允许 [`SESSION_PLACEHOLDER`] 与 [`CONVERSATION_PLACEHOLDER`]。
///
/// 不认识的变量 / 没闭合的 `{{` 一律报错：静默原样发出去的话，网关会回一个
/// 与配置毫无关系的 400，用户根本看不出是占位符写错了。
fn validate_template(label: &str, value: &str) -> Result<(), ApiError> {
    let mut rest = value;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Err(config_error(format!(
                "「{label}」的值里有没闭合的变量，只支持 {} 和 {}。",
                SESSION_PLACEHOLDER, CONVERSATION_PLACEHOLDER
            )));
        };
        let variable = &after[..end];
        if variable != "session" && variable != "conversation" {
            return Err(config_error(format!(
                "「{label}」的值里有不认识的变量，只支持 {} 和 {}。",
                SESSION_PLACEHOLDER, CONVERSATION_PLACEHOLDER
            )));
        }
        rest = &after[end + 2..];
    }
    Ok(())
}

fn validate_header_value(name: &str, value: &str) -> Result<(), ApiError> {
    if value.chars().count() > MAX_VALUE_CHARS {
        return Err(config_error(format!(
            "额外请求头「{name}」的值不能超过 {MAX_VALUE_CHARS} 个字符。"
        )));
    }
    // `HeaderValue::from_str` 只接受可见 ASCII 与水平制表符；在这里先拦下来，
    // 报错才能指向"哪一个头"，而不是运行期才炸出一句 HTTP 库的话。
    if !value.chars().all(|c| c == '\t' || (' '..='~').contains(&c)) {
        return Err(config_error(format!(
            "额外请求头「{name}」的值只能是可见 ASCII 字符（不能有中文、换行或控制字符）。"
        )));
    }
    Ok(())
}

/// RFC 7230 的 `token`（请求头名称的合法字符）。
fn is_header_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '!' | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '-'
                | '.'
                | '^'
                | '_'
                | '`'
                | '|'
                | '~'
        )
}

fn parse_string_object(raw: Option<&str>, column: &str) -> BTreeMap<String, String> {
    let Some(text) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return BTreeMap::new();
    };
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Object(map)) => map
            .into_iter()
            .filter_map(|(key, value)| scalar_text(&value).map(|value| (key, value)))
            .collect(),
        Ok(_) => {
            eprintln!("[provider] {column} 不是 JSON 对象，按空处理");
            BTreeMap::new()
        }
        Err(error) => {
            eprintln!("[provider] {column} 读不出来（{error}），按空处理");
            BTreeMap::new()
        }
    }
}

/// `compat_json` 里的 `query` 段（其它的兼容开关与这里无关，原样忽略）。
fn query_from_compat(raw: Option<&str>) -> BTreeMap<String, String> {
    let Some(text) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return BTreeMap::new();
    };
    let Ok(serde_json::Value::Object(root)) = serde_json::from_str::<serde_json::Value>(text)
    else {
        eprintln!("[provider] compat_json 不是 JSON 对象，按空处理");
        return BTreeMap::new();
    };
    let Some(serde_json::Value::Object(query)) = root.get("query") else {
        return BTreeMap::new();
    };
    query
        .iter()
        .filter_map(|(key, value)| scalar_text(value).map(|value| (key.clone(), value)))
        .collect()
}

/// JSON 标量 → 文本（数字按字面文本存；对象/数组/ null 丢掉）。
fn scalar_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn string_object_json(map: &BTreeMap<String, String>) -> String {
    serde_json::Value::Object(
        map.iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect(),
    )
    .to_string()
}

#[cfg(test)]
mod tests;
