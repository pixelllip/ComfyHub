//! Provider 连接测试与模型发现（P4 第 53 步，A2）—— Kotlin `AiUpstream` 那两条端点的移植。
//!
//! | 端点 | 干什么 |
//! | --- | --- |
//! | `POST /api/ai/providers/{id}/test` | 只判断"这个 Base URL + 这把 Key 能不能用"，**只回脱敏结果与稳定错误码**（AIH-008） |
//! | `POST /api/ai/providers/{id}/discover-models` | 拉一次 `/models`，**只产生候选、不落库**（AIH-009） |
//!
//! # 四条纪律（每条都有理由）
//!
//!  1. **Base URL 填 `.../v1` 与不填是两个不同的地址**：前者拼出 `.../models`（很多网关 404），
//!     后者拼出 `.../models` 也 404 —— 所以**两个候选都试**，并把"实际可用的是哪个"回报给界面。
//!  2. **`/models` 回 404 不等于鉴权失败**：有些网关根本没有这个端点。这时只要**不是** 401/403
//!     就认为"连通"（`MODEL_LIST_UNAVAILABLE` 是内部标记，不是错误码）。
//!  3. **密钥绝不进日志、不进响应**：错误正文一律先 `redact` 再截断（AIH-051：有些网关会把
//!     请求头原样回显，直接透传等于把 Key 写进界面）。
//!  4. **能力逐维度合并、不猜**（AIH-011）：接口明确声明的那一项用接口的，没声明的维度用内置目录，
//!     两边都没有才用保守默认（仅文本 + 工具默认给上）。"看到任何字段就整体采信"的旧写法会让
//!     内置目录**永远轮不到** —— 用户看到全是"未识别 / 仅文本"。

use serde_json::{json, Map, Value};

use crate::ai::AiProviderRow;
use crate::routes::{ApiError, AppState};

/// 连接测试的超时（Kotlin `httpGet(..., timeoutSeconds = 10)`）。
pub const TEST_TIMEOUT_SECONDS: u64 = 10;
/// 取模型列表的超时（比探测宽松：列表可能很大）。
pub const MODELS_TIMEOUT_SECONDS: u64 = 20;

/// `/models` 返回 404 时的**内部标记**：鉴权已经过了，只是拿不到模型列表。
const MODEL_LIST_STATUS: &str = "MODEL_LIST_UNAVAILABLE";

/// 这个协议的模型列表地址（Kotlin `modelsUrl`）。
pub fn models_url(base_url: &str, api: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    match api {
        "anthropic-messages" => {
            if base.ends_with("/v1") {
                format!("{base}/models")
            } else {
                format!("{base}/v1/models")
            }
        }
        _ => format!("{base}/models"),
    }
}

/// 模型列表地址的候选序列（按可能性从高到低）。
///
/// Base URL 带 `/v1` 就去掉试一个，不带就补上试一个 —— 两种协议同一套规则，
/// 只有**首选**不同（见 [`models_url`]）。
pub fn models_url_candidates(base_url: &str, api: &str) -> Vec<String> {
    let base = base_url.trim().trim_end_matches('/');
    let has_v1 = base.ends_with("/v1");
    let stripped = if has_v1 {
        base.trim_end_matches("/v1").to_string()
    } else {
        format!("{base}/v1")
    };
    let alternate = format!("{stripped}/models");
    let mut out = vec![models_url(base_url, api)];
    if !out.contains(&alternate) {
        out.push(alternate);
    }
    out
}

/// 一次 `GET` 探测的结果。
#[derive(Debug, Clone, Default)]
pub struct Probe {
    pub ok: bool,
    pub status: Option<u16>,
    pub code: String,
    pub detail: String,
    pub model_count: Option<i64>,
    /// 只有确实需要正文（取模型列表）时才带回来
    pub body: Option<String>,
}

/// 上游返回的数组里有多少个模型（OpenAI 的 `data[]` 与富信息的 `models{}` 都认）。
pub fn count_models(body: &str) -> Option<i64> {
    let root: Value = serde_json::from_str(body).ok()?;
    let array = match &root {
        Value::Array(items) => Some(items),
        Value::Object(map) => map
            .get("data")
            .or_else(|| map.get("models"))
            .and_then(Value::as_array),
        _ => None,
    }?;
    Some(array.len() as i64)
}

/// 发一次 `GET` 探测。
#[allow(clippy::too_many_arguments)]
pub async fn probe_get(
    state: &AppState,
    url: &str,
    api: &str,
    secret: Option<&str>,
    trust: &str,
    timeout_seconds: u64,
    with_body: bool,
) -> Probe {
    let Some(host) = crate::upstream::host_of(url) else {
        return Probe {
            ok: false,
            code: "CONFIG_ERROR".to_string(),
            detail: "Base URL 无法解析".to_string(),
            ..Default::default()
        };
    };
    if let Err(message) = crate::endpoint_guard::assert_allowed(&host, trust) {
        return Probe {
            ok: false,
            code: "CONFIG_ERROR".to_string(),
            detail: message,
            ..Default::default()
        };
    }
    let client = match crate::upstream::client_for(&host) {
        Ok(client) => client,
        Err(error) => {
            return Probe {
                ok: false,
                code: "CONFIG_ERROR".to_string(),
                detail: format!("HTTP 客户端创建失败：{}", error.human()),
                ..Default::default()
            }
        }
    };
    let _ = state;

    let parsed = match crate::protocol::Api::parse(api) {
        Some(parsed) => parsed,
        None => {
            return Probe {
                ok: false,
                code: "CONFIG_ERROR".to_string(),
                detail: format!("未知协议：{api}"),
                ..Default::default()
            }
        }
    };
    let mut builder = client
        .get(url)
        .header("accept", "application/json")
        .timeout(std::time::Duration::from_secs(timeout_seconds));
    for (name, value) in crate::upstream::auth_header(parsed, secret) {
        builder = builder.header(name, value);
    }

    let response = match builder.send().await {
        Ok(response) => response,
        Err(error) => {
            let detail = if error.is_timeout() {
                format!("连接超时（{timeout_seconds}秒）")
            } else {
                format!("无法连接：{}", crate::comfy_client::friendly_error(&error, url))
            };
            return Probe {
                ok: false,
                status: None,
                code: "PROVIDER_UNREACHABLE".to_string(),
                detail,
                ..Default::default()
            };
        }
    };
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();

    let base_code = crate::upstream::classify_status(status);
    if base_code == "OK" {
        return Probe {
            ok: true,
            status: Some(status),
            code: String::new(),
            detail: String::new(),
            model_count: count_models(&body),
            body: with_body.then_some(body),
        };
    }
    let code = crate::upstream::refine_from_body(&body).unwrap_or(base_code);
    if status == 404 {
        // 鉴权不在这里判断：有些网关根本没有 /models，给 404 不代表 Key 不对
        return Probe {
            ok: false,
            status: Some(status),
            code: MODEL_LIST_STATUS.to_string(),
            detail: crate::upstream::explain(code, status),
            ..Default::default()
        };
    }
    Probe {
        ok: false,
        status: Some(status),
        code: code.to_string(),
        detail: format!(
            "{}｜上游返回：{}",
            crate::upstream::explain(code, status),
            crate::upstream::redact(&body, secret)
        ),
        ..Default::default()
    }
}

/// 解析出一次上游响应的正文与错误（`getModels`）。
async fn get_models(state: &AppState, provider: &AiProviderRow, secret: Option<&str>) -> Probe {
    let Some(api) = crate::protocol::Api::parse(&provider.api) else {
        return Probe {
            ok: false,
            code: "CONFIG_ERROR".to_string(),
            detail: format!("未知协议：{}", provider.api),
            ..Default::default()
        };
    };
    let trust = crate::endpoint_guard::parse_trust(&provider.endpoint_trust)
        .unwrap_or_else(|| crate::endpoint_guard::infer_trust(""));
    let mut first: Option<Probe> = None;
    for url in models_url_candidates(&provider.base_url, api.wire()) {
        let probe = probe_get(
            state,
            &url,
            api.wire(),
            secret,
            trust,
            MODELS_TIMEOUT_SECONDS,
            true,
        )
        .await;
        if probe.ok {
            return probe;
        }
        // 鉴权失败：换地址也没用
        if probe.code == "MISSING_CREDENTIAL" {
            return probe;
        }
        if first.is_none() {
            first = Some(probe);
        }
    }
    first.unwrap_or(Probe {
        ok: false,
        code: "CONFIG_ERROR".to_string(),
        detail: "Base URL 无法解析".to_string(),
        ..Default::default()
    })
}

/// 解析一个 provider 的密钥（**只在后端内部用，用完即弃**）。
///
/// `pub(crate)`：实测校准（`capability_probe.rs`）也要同一把密钥 —— 两处各写一份
/// "凭据引用怎么解析"迟早会出现"连接测试连得上、校准却说没凭据"。
pub(crate) fn resolve_secret(state: &AppState, provider: &AiProviderRow) -> Option<String> {
    let reference = provider.credential_ref.as_deref().map(str::trim).filter(|r| !r.is_empty())?;
    crate::auth::resolve_credential(state, reference).ok().filter(|s| !s.is_empty())
}

/// `POST /api/ai/providers/{id}/test` 的响应（Kotlin `ProviderTestResponse`，5 个键）。
pub async fn test_connection_json(state: &AppState, provider: &AiProviderRow) -> Value {
    let Some(api) = crate::protocol::Api::parse(&provider.api) else {
        return json!({
            "ok": false,
            "errorCode": "CONFIG_ERROR",
            "message": format!("未知协议：{}", provider.api),
        });
    };
    let secret = resolve_secret(state, provider);
    // 先查凭据（便宜、不需要网络）：缺密钥时应该报得比"地址不可信"更准
    if provider.credential_ref.as_deref().map(str::trim).filter(|r| !r.is_empty()).is_some()
        && secret.is_none()
    {
        return json!({
            "ok": false,
            "errorCode": "MISSING_CREDENTIAL",
            "message": format!(
                "该 Provider 引用了凭据 {}，但本机没有配置",
                provider.credential_ref.clone().unwrap_or_default()
            ),
        });
    }

    let trust = crate::endpoint_guard::parse_trust(&provider.endpoint_trust)
        .unwrap_or_else(|| crate::endpoint_guard::infer_trust(""));
    let candidates = models_url_candidates(&provider.base_url, api.wire());
    let mut first: Option<Probe> = None;
    for url in candidates.iter() {
        let probe = probe_get(
            state,
            url,
            api.wire(),
            secret.as_deref(),
            trust,
            TEST_TIMEOUT_SECONDS,
            false,
        )
        .await;
        if probe.ok {
            let via = if Some(url) == candidates.first() {
                String::new()
            } else {
                format!("（实际可用地址 {url}）")
            };
            return json!({
                "ok": true,
                "message": format!("连接正常（HTTP {}）{via}", probe.status.unwrap_or(0)),
                "httpStatus": probe.status,
                "modelCount": probe.model_count,
            });
        }
        // 鉴权失败：换地址也没用，直接报（这才是真正的"API Key 报错"）
        if probe.code == "MISSING_CREDENTIAL" {
            return json!({
                "ok": false,
                "errorCode": probe.code,
                "message": probe.detail,
                "httpStatus": probe.status,
            });
        }
        if first.is_none() {
            first = Some(probe);
        }
    }

    let Some(first) = first else {
        return json!({ "ok": false, "errorCode": "CONFIG_ERROR", "message": "Base URL 无法解析" });
    };
    // 所有候选都拿不到模型列表：只要不是鉴权问题，就说明端点本身是通的
    if first.code == MODEL_LIST_STATUS {
        return json!({
            "ok": true,
            "message": format!(
                "连接正常（HTTP {}）：该端点不提供模型列表（/models 返回 404），因此「获取可用模型」可能为空，手工添加模型即可。",
                first.status.unwrap_or(0)
            ),
            "httpStatus": first.status,
        });
    }
    json!({
        "ok": false,
        "errorCode": first.code,
        "message": first.detail,
        "httpStatus": first.status,
    })
}

/// `POST /api/ai/providers/{id}/discover-models` 的响应（Kotlin `DiscoverModelsResponse`）。
pub async fn discover_models_json(state: &AppState, provider: &AiProviderRow) -> Value {
    let secret = resolve_secret(state, provider);
    let response = get_models(state, provider, secret.as_deref()).await;
    if !response.ok {
        return json!({
            "ok": false,
            "errorCode": response.code,
            "message": response.detail,
            "candidates": [],
        });
    }
    let overlays = crate::capability_catalog::load_overlays(&crate::capability_catalog::dir_of(
        state.media_dir().parent().unwrap_or(state.media_dir()),
    ));
    let candidates = parse_candidates(response.body.as_deref().unwrap_or(""), &overlays);
    let declared = candidates
        .iter()
        .filter(|c| c.get("capabilitySource").and_then(Value::as_str) == Some("discovered"))
        .count();
    let builtin = candidates
        .iter()
        .filter(|c| c.get("capabilitySource").and_then(Value::as_str) == Some("builtin"))
        .count();
    let unknown = candidates
        .iter()
        .filter(|c| c.get("capabilitySource").and_then(Value::as_str) == Some("unknown"))
        .count();
    let message = if candidates.is_empty() {
        "未发现模型（端点返回空列表）".to_string()
    } else {
        format!(
            "发现 {} 个候选：接口声明 {declared} 个、内置目录 {builtin} 个、未识别 {unknown} 个",
            candidates.len()
        )
    };
    json!({ "ok": true, "message": message, "candidates": candidates })
}

/// 接口**明确**声明的能力：每个维度三态，`None` = 接口没说这一项（Kotlin `DeclaredCapabilities`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Declared {
    pub modalities: Option<Vec<String>>,
    pub tools: Option<bool>,
    pub reasoning: Option<bool>,
}

/// 把一串模态 token 归一（`text+image->text` / `vision` / `pdf` 这些写法都认）。
fn modality_tokens(raw: &str) -> Vec<String> {
    let token = raw.trim().to_lowercase();
    if token.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut push = |value: &str| {
        if !out.iter().any(|m| m == value) {
            out.push(value.to_string());
        }
    };
    for part in token.split(['+', ',', '/', '|', ' ', '>']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if part.starts_with("image") || part == "vision" {
            push("image");
        } else if part.starts_with("video") {
            push("video");
        } else if part.starts_with("audio") {
            push("audio");
        } else if part.starts_with("file") || part.starts_with("pdf") {
            push("document");
        } else if part.starts_with("text") || part == "prompt" {
            push("text");
        }
    }
    out
}

fn primitive_text(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) => Some(text.trim().to_string()).filter(|t| !t.is_empty()),
        Some(Value::Number(number)) => Some(number.to_string()),
        _ => None,
    }
}

fn primitive_bool(value: Option<&Value>) -> Option<bool> {
    match value {
        Some(Value::Bool(flag)) => Some(*flag),
        Some(Value::String(text)) => match text.trim().to_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn primitive_i64(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(number)) => number.as_i64(),
        Some(Value::String(text)) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// 读取接口**自己声明**的能力（逐维度三态）。
///
/// 关键点：**只有真的读到这一项的值，才算"接口说了"** —— 旧写法只要看到任何一个已知字段名
/// （`capabilities: {}`、`reasoning: false`）就把整个模型标成"接口声明"，内置目录永远轮不到。
pub fn parse_declared_capabilities(obj: &Map<String, Value>) -> Option<Declared> {
    let mut declared = Declared::default();

    /// 数组形态：`input_modalities` / `modalities` / `capabilities: ["vision","tools"]`。
    ///
    /// 混装数组里 `"tools"` 是**工具声明**（不是模态）—— 单独拎出来，
    /// 否则会多出一个叫 "tools" 的模态。
    fn parse_array_of(
        target: &Map<String, Value>,
        key: &str,
        declared: &mut Declared,
    ) -> Option<Vec<String>> {
        let items = target.get(key).and_then(Value::as_array)?;
        let mut found: Vec<String> = Vec::new();
        let mut tools_flag: Option<bool> = None;
        for element in items {
            match element {
                Value::String(text) => {
                    let token = text.trim().to_lowercase();
                    // `capabilities: ["vision","tools"]` 这种混装数组里，"tools" 是工具声明
                    if token == "tools" || token == "function_calling" || token == "tool_call" {
                        tools_flag = Some(true);
                    } else {
                        for m in modality_tokens(&token) {
                            if !found.contains(&m) {
                                found.push(m);
                            }
                        }
                    }
                }
                Value::Object(map) => {
                    for key in map.keys() {
                        for m in modality_tokens(key) {
                            if !found.contains(&m) {
                                found.push(m);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if tools_flag == Some(true) && declared.tools.is_none() {
            declared.tools = Some(true);
        }
        if found.is_empty() {
            found.push("text".to_string());
        }
        Some(found)
    }

    // 1) 数组形态：input_modalities / modalities / capabilities: ["vision","tools"]
    for key in [
        "input_modalities",
        "inputModalities",
        "modalities",
        "supported_modalities",
        "capabilities",
    ] {
        // ⚠ 每个键都**必须真的解析一遍**：`capabilities: ["vision","tools"]` 里的 `"tools"`
        // 是**副作用**（它只设 tools、不设模态）—— 只在"模态还没定"时才解析，就会把工具声明
        // 整个漏掉（接口说了 tools，候选里却是默认值，而界面上看不出哪里错）。
        if let Some(found) = parse_array_of(obj, key, &mut declared) {
            if declared.modalities.is_none() {
                declared.modalities = Some(found);
            }
        }
    }

    // 2) OpenRouter：architecture.input_modalities / architecture.modality
    if let Some(arch) = obj.get("architecture").and_then(Value::as_object) {
        if declared.modalities.is_none() {
            if let Some(found) = parse_array_of(arch, "input_modalities", &mut declared) {
                declared.modalities = Some(found);
            }
        }
        if declared.modalities.is_none() {
            if let Some(modality) = primitive_text(arch.get("modality")) {
                let parsed = modality_tokens(&modality);
                if !parsed.is_empty() {
                    declared.modalities = Some(parsed);
                }
            }
        }
    }

    // 3) 布尔开关形态（只有真读到布尔值才算声明）
    let bool_of = |keys: &[&str]| -> Option<bool> {
        keys.iter().find_map(|key| primitive_bool(obj.get(*key)))
    };
    if let Some(vision) = bool_of(&["supports_vision", "vision", "supports_image", "multimodal", "image_input"]) {
        let current = declared.modalities.clone().unwrap_or_default();
        declared.modalities = Some(if vision {
            let mut out = current;
            if !out.iter().any(|m| m == "image") {
                out.push("image".to_string());
            }
            if out.is_empty() {
                vec!["text".to_string(), "image".to_string()]
            } else {
                out
            }
        } else if current.is_empty() {
            vec!["text".to_string()]
        } else {
            current
        });
    }
    if let Some(tools) = bool_of(&["supports_tools", "function_calling", "tool_call", "tools"]) {
        declared.tools = Some(tools);
    }
    if let Some(reasoning) =
        bool_of(&["supports_reasoning", "reasoning", "thinking", "supports_thinking"])
    {
        declared.reasoning = Some(reasoning);
    }

    // 4) 嵌套形态：capabilities: {"vision": true, "function_calling": true, "thinking": true}
    if let Some(caps) = obj.get("capabilities").and_then(Value::as_object) {
        let flag = |key: &str| primitive_bool(caps.get(key));
        if flag("vision") == Some(true) || flag("image") == Some(true) {
            let mut out = declared.modalities.clone().unwrap_or_default();
            if !out.iter().any(|m| m == "image") {
                out.push("image".to_string());
            }
            declared.modalities = Some(out);
        }
        if declared.tools.is_none() {
            if let Some(flag) = flag("function_calling").or_else(|| flag("tools")).or_else(|| flag("tool_call")) {
                declared.tools = Some(flag);
            }
        }
        if declared.reasoning.is_none() {
            if let Some(flag) = flag("reasoning").or_else(|| flag("thinking")) {
                declared.reasoning = Some(flag);
            }
        }
    }

    if declared.modalities.is_none() && declared.tools.is_none() && declared.reasoning.is_none() {
        return None;
    }
    Some(declared)
}

/// 解析模型列表 → 候选（Kotlin `parseCandidates`，AIH-011 的"逐维度合并、不猜能力"）。
pub fn parse_candidates(body: &str, overlays: &crate::capability_catalog::Overlays) -> Vec<Value> {
    let Ok(root) = serde_json::from_str::<Value>(body) else { return Vec::new() };
    let array = match &root {
        Value::Array(items) => items.clone(),
        Value::Object(map) => map
            .get("data")
            .or_else(|| map.get("models"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    let mut out: Vec<Value> = Vec::new();
    for element in array {
        let Some(obj) = element.as_object() else { continue };
        let text = |keys: &[&str]| -> Option<String> {
            keys.iter().find_map(|key| primitive_text(obj.get(*key)))
        };
        let number = |keys: &[&str]| -> Option<i64> {
            keys.iter().find_map(|key| primitive_i64(obj.get(*key)))
        };
        let Some(id) = text(&["id", "name", "model"]) else { continue };
        let declared = parse_declared_capabilities(obj);
        // 内置目录**始终**查：接口只声明一部分时，剩下的维度由它补
        let from_catalog = crate::capability_catalog::lookup(overlays, &id);

        let modalities = declared
            .as_ref()
            .and_then(|d| d.modalities.clone())
            .or_else(|| from_catalog.as_ref().map(|c| c.modalities.clone()))
            .unwrap_or_else(|| vec!["text".to_string()]);
        // 工具：接口声明 > 内置目录 > **默认给上**
        let tools = declared
            .as_ref()
            .and_then(|d| d.tools)
            .or_else(|| from_catalog.as_ref().map(|c| c.tools))
            .unwrap_or(true);
        // 思考：接口声明 > 内置目录 > 不支持
        let reasoning = declared
            .as_ref()
            .and_then(|d| d.reasoning)
            .or_else(|| from_catalog.as_ref().map(|c| c.reasoning))
            .unwrap_or(false);
        // 档位 / 方言只来自内置目录，且必须与"支持推理"一致（否则保存时会被拒绝）
        let thinking_efforts: Vec<(String, String)> = if reasoning {
            from_catalog.as_ref().map(|c| c.thinking_efforts.clone()).unwrap_or_default()
        } else {
            Vec::new()
        };
        let thinking_format =
            if reasoning { from_catalog.as_ref().and_then(|c| c.thinking_format.clone()) } else { None };

        let by_api = declared.is_some();
        let by_catalog = from_catalog.is_some();
        let matched_by = from_catalog.as_ref().map(|c| c.matched_by.clone()).unwrap_or_default();

        let mut candidate = Map::new();
        candidate.insert("id".into(), json!(id));
        candidate.insert(
            "displayName".into(),
            json!(text(&["display_name", "displayName", "name"]).unwrap_or_else(|| id.clone())),
        );
        if let Some(value) = number(&["context_window", "contextWindow", "context_length", "max_context_tokens"]) {
            candidate.insert("contextWindow".into(), json!(value));
        }
        if let Some(value) = number(&["max_output_tokens", "maxOutputTokens", "max_tokens"]) {
            candidate.insert("maxOutputTokens".into(), json!(value));
        }
        candidate.insert("modalities".into(), json!(modalities));
        candidate.insert("tools".into(), json!(tools));
        candidate.insert("reasoning".into(), json!(reasoning));
        if !thinking_efforts.is_empty() {
            let mut map = Map::new();
            for (level, wire) in thinking_efforts.iter() {
                map.insert(level.clone(), json!(wire));
            }
            candidate.insert("thinkingEfforts".into(), Value::Object(map));
        }
        if let Some(format) = thinking_format {
            candidate.insert("thinkingFormat".into(), json!(format));
        }
        candidate.insert(
            "capabilitySource".into(),
            json!(if by_api {
                "discovered"
            } else if by_catalog {
                "builtin"
            } else {
                "unknown"
            }),
        );
        candidate.insert(
            "capabilityNote".into(),
            json!(if by_api && by_catalog {
                format!("模态/思考按接口声明；其余维度参考内置目录规则「{matched_by}」")
            } else if by_api {
                "接口在下发的模型信息里声明了输入模态".to_string()
            } else if by_catalog {
                format!("接口没给能力信息 → 用内置目录规则「{matched_by}」（可能过期，可手工修改）")
            } else {
                "接口未声明、内置目录也没有 → 仅文本 + 默认给工具；需要图片/思考请手工勾选".to_string()
            }),
        );
        let value = Value::Object(candidate);
        // `distinctBy { it.id }`
        if !out.iter().any(|existing| existing.get("id") == value.get("id")) {
            out.push(value);
        }
    }
    out
}

/// 两条端点的路由（挂在 `/api/ai/providers/{id}/...` 下）。
pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route(
            "/api/ai/providers/{id}/test",
            axum::routing::post(test_provider),
        )
        .route(
            "/api/ai/providers/{id}/discover-models",
            axum::routing::post(discover_models),
        )
}

async fn test_provider(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<axum::Json<Value>, ApiError> {
    let provider = crate::ai::require_provider_row(&state, &id)?;
    Ok(axum::Json(test_connection_json(&state, &provider).await))
}

async fn discover_models(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<axum::Json<Value>, ApiError> {
    let provider = crate::ai::require_provider_row(&state, &id)?;
    Ok(axum::Json(discover_models_json(&state, &provider).await))
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
