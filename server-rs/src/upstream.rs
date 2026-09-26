//! 上游 LLM 客户端（P4 第 29 步）—— HTTP 那一半；协议那一半在 [`crate::protocol`]。
//!
//! # 这个模块只做三件事
//!
//! 1. **拼 URL + 鉴权头**（三家不一样，见 [`crate::protocol::Api`]）；
//! 2. **发出去、把 SSE 逐行喂给适配器**，吐出一串 [`crate::protocol::StreamEvent`]；
//! 3. **把失败归到稳定错误码**（AIH-024）—— 前端与 harness 都不解析供应商文案。
//!
//! # 四条硬规则（照抄 Kotlin `AiUpstream` 的文件头）
//!
//! 1. **绝不记录 Authorization / x-api-key**：日志只写 Provider ID、脱敏端点、状态码、稳定码；
//! 2. **不跟随重定向**（`Redirect::NEVER`）—— 免得密钥被带到别的域名；
//! 3. 出网前按 [`crate::outbound`] 复核目标地址（**公网走代理、本机/内网强制直连**）；
//! 4. **上游回显的文本要先脱敏再截断**再给人看：有的网关会把请求头原样回显，
//!    直接透传等于把 Key 写进界面和日志（AIH-051）。
//!
//! # 错误码（`AiErrorCode` 的那几个）
//!
//! | 状态码 | 码 |
//! | --- | --- |
//! | 401 / 403 | `MISSING_CREDENTIAL` |
//! | 404 | `PROTOCOL_ERROR` |
//! | 408 / 504 | `PROVIDER_UNREACHABLE` |
//! | 429 | `RATE_LIMIT` |
//! | 402 | `QUOTA_EXCEEDED` |
//! | 其余 4xx | `CONFIG_ERROR` |
//! | 5xx | `PROVIDER_UNREACHABLE` |
//! | 连不上 / 超时 | `PROVIDER_UNREACHABLE` |
//!
//! ⚠ `MISSING_CREDENTIAL` 与 `CONFIG_ERROR` 不能混：前者是"去配一把 key"，
//! 后者是"地址/模型填错了"—— 界面给用户的下一步完全不一样。

use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;

use crate::protocol::{adapter_for, Api, ChatTurn, Frame, ReasoningRequest, SseAccumulator, StreamEvent, ToolSpec};
use crate::routes::{ApiError, AppState};

/// 连接超时（Kotlin 那边 `connectTimeout = 5s`）。
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 整次请求的超时上限。流式响应会一直有数据，所以这个值只在"上游彻底不动"时兜底。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
/// 上游报错正文的截断上限（超过就没什么信息量了，还会把界面刷屏）。
pub const ERROR_BODY_MAX: usize = 1200;

/// 一次上游请求的全部输入。
pub struct ChatRequest<'a> {
    pub state: &'a AppState,
    pub api: Api,
    pub base_url: &'a str,
    pub model: &'a str,
    /// 凭据的引用名（`credentialRef`）；`None` / 空 = 不带鉴权头
    pub credential_ref: Option<&'a str>,
    pub turns: &'a [ChatTurn],
    pub tools: &'a [ToolSpec],
    pub reasoning: ReasoningRequest,
    pub max_output_tokens: Option<i64>,
}

/// 按正文里的关键字**细化**错误码（Kotlin `AiUpstream.refineFromBody`，AIH-024）。
///
/// 真实网关的 400/403 语义差别很大，只看状态码会把"套餐里没有这个模型"误报成"密钥不对"、
/// 把"这个模型该走另一个端点"误报成"配置随便写错了"。`None` = 正文里没有更具体的线索。
pub fn refine_from_body(body: &str) -> Option<&'static str> {
    let b = body.to_lowercase();
    let has = |needle: &str| b.contains(needle);
    if has("not_in_plan")
        || has("not in plan")
        || has("quota")
        || has("insufficient_quota")
        || has("billing")
        || has("credit")
    {
        return Some("QUOTA_EXCEEDED");
    }
    // 混合网关最常见的坑：模型不属于当前协议的端点
    if has("not supported on this endpoint")
        || has("unsupported_model")
        || (has("use /") && has("provider/v1"))
    {
        return Some("PROTOCOL_ERROR");
    }
    if has("invalid api key") || has("authentication_error") || has("unauthorized") {
        return Some("MISSING_CREDENTIAL");
    }
    if has("rate limit") || has("rate_limit") {
        return Some("RATE_LIMIT");
    }
    None
}

/// 状态码 → 稳定错误码（**纯函数**，便于单测）。
pub fn classify_status(status: u16) -> &'static str {
    match status {
        200..=299 => "OK",
        401 | 403 => "MISSING_CREDENTIAL",
        404 => "PROTOCOL_ERROR",
        408 | 504 => "PROVIDER_UNREACHABLE",
        429 => "RATE_LIMIT",
        402 => "QUOTA_EXCEEDED",
        400..=499 => "CONFIG_ERROR",
        _ => "PROVIDER_UNREACHABLE",
    }
}

/// 把人话给出来（`AiException` 的 `detail` 就是它）。
pub fn explain(code: &str, status: u16) -> String {
    match code {
        "MISSING_CREDENTIAL" => format!("上游拒绝了这次请求（HTTP {status}）：凭据缺失或无效"),
        "PROTOCOL_ERROR" => format!("上游说不认识这个端点（HTTP {status}）：检查 Base URL 与协议是否匹配"),
        "PROVIDER_UNREACHABLE" => format!("连不上上游（HTTP {status}）：网络或对方服务的问题"),
        "RATE_LIMIT" => format!("上游限流了（HTTP {status}）：稍后再试，或换一个模型"),
        "QUOTA_EXCEEDED" => format!("上游说额度用完了（HTTP {status}）"),
        "CONFIG_ERROR" => format!("上游认为这次请求不合法（HTTP {status}）"),
        _ => format!("上游返回了 HTTP {status}"),
    }
}

/// 上游报错正文脱敏：**先把密钥抹掉，再截断**（顺序反了会把密钥的一半留下）。
pub fn redact(raw: &str, secret: Option<&str>) -> String {
    let mut text = raw.trim().to_string();
    if let Some(secret) = secret.map(str::trim).filter(|s| s.len() >= 6) {
        text = text.replace(secret, "***");
    }
    // 常见形状的兜底：`Bearer sk-…` / `"api_key":"…"`
    for marker in ["Bearer ", "sk-", "api_key\":\""] {
        while let Some(index) = text.find(marker) {
            let start = index + marker.len();
            let end = text[start..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == ',' || c == '}')
                .map(|offset| start + offset)
                .unwrap_or(text.len());
            if end <= start {
                break;
            }
            text.replace_range(start..end, "***");
            // 抹过一次之后这个 marker 不再命中（变成 `Bearer ***`）—— 避免死循环
            if text[start..].starts_with("***") {
                break;
            }
        }
    }
    // 按**字符**截断（中文按字节切会切出半个字）
    let truncated: String = text.chars().take(ERROR_BODY_MAX).collect();
    if truncated.chars().count() < text.chars().count() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

/// 造一个 HTTP 客户端：**不跟随重定向** + 按 [`crate::outbound`] 选代理。
///
/// 每次请求现造一个：代理是按**目标主机**定的（公网走、内网直连），
/// 而 `reqwest::Client` 的代理是客户端级的。造客户端的开销远小于一次 HTTPS 握手。
pub fn client_for(host: &str) -> Result<reqwest::Client, ApiError> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        // 规则 2：**绝不跟随重定向**（密钥不能被带到别的域名）
        .redirect(reqwest::redirect::Policy::none());
    match crate::outbound::route(host) {
        crate::outbound::Route::Proxy(addr) => {
            let proxy = reqwest::Proxy::all(format!("http://{addr}"))
                .map_err(|e| ApiError::internal(format!("代理地址不合法：{e}")))?;
            builder = builder.proxy(proxy);
        }
        crate::outbound::Route::Direct => {
            // `no_proxy` 清空 + 显式不设代理：reqwest 自己也不看系统代理，
            // 但环境变量（`HTTPS_PROXY`）它**是看的** —— 所以这里显式覆盖掉，
            // 否则内网目标会被环境变量里的代理接走。
            builder = builder.no_proxy();
        }
    }
    builder.build().map_err(|e| ApiError::internal(format!("HTTP 客户端创建失败：{e}")))
}

/// 鉴权头（三家不一样；`None` 表示不带）。
pub fn auth_header(api: Api, secret: Option<&str>) -> Vec<(&'static str, String)> {
    let Some(secret) = secret.map(str::trim).filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    match api {
        Api::OpenAiCompletions | Api::OpenAiResponses => {
            vec![("authorization", format!("Bearer {secret}"))]
        }
        Api::AnthropicMessages => vec![
            ("x-api-key", secret.to_string()),
            ("anthropic-version", "2023-06-01".to_string()),
        ],
    }
}

/// 发一次流式请求，把事件逐个交给 `on_event`。
///
/// `cancelled()` 每个分片读之前问一次：取消要**在安全点收尾**（主循环负责落终态 +
/// flush 已经说出去的话），直接强杀会把最后那句话丢掉（R6）。
pub async fn stream_chat<F>(
    request: ChatRequest<'_>,
    mut on_event: F,
    cancelled: impl Fn() -> bool,
) -> Result<Outcome, ApiError>
where
    F: FnMut(StreamEvent) -> Result<(), ApiError>,
{
    let url = request.api.chat_url(request.base_url);
    let host = host_of(&url).ok_or_else(|| {
        ApiError::ai("CONFIG_ERROR", format!("Base URL 无法解析出主机名：{}", request.base_url))
    })?;

    let secret = match request.credential_ref.map(str::trim).filter(|s| !s.is_empty()) {
        Some(reference) => Some(crate::auth::resolve_credential(request.state, reference)?),
        None => None,
    };

    let adapter = adapter_for(request.api);
    let body = adapter.build_body(
        request.model,
        request.turns,
        request.tools,
        &request.reasoning,
        request.max_output_tokens,
    );

    let client = client_for(&host)?;
    let mut builder = client
        .post(&url)
        .header("accept", "text/event-stream")
        .header("content-type", "application/json")
        .json(&body);
    for (name, value) in auth_header(request.api, secret.as_deref()) {
        builder = builder.header(name, value);
    }

    // 只记 provider / 端点 / 协议 / 模型，**绝不记 header 与请求体**
    eprintln!(
        "Run 请求 endpoint={} adapter={} model={} tools={} route={}",
        safe_endpoint(&url),
        request.api.wire(),
        request.model,
        request.tools.len(),
        crate::outbound::route(&host).describe()
    );

    let response = builder.send().await.map_err(|e| {
        ApiError::ai(
            "PROVIDER_UNREACHABLE",
            format!(
                "连不上上游（{}）：{}｜本次路径：{}",
                safe_endpoint(&url),
                trim_error(&e.to_string()),
                crate::outbound::route(&host).describe()
            ),
        )
    })?;

    let status = response.status().as_u16();
    if !(200..=299).contains(&status) {
        let raw = response.text().await.unwrap_or_default();
        let code = classify_status(status);
        let detail = redact(&raw, secret.as_deref());
        eprintln!("上游返回 {status}（code={code}）: {detail}");
        return Err(ApiError::ai(
            code,
            if detail.is_empty() {
                explain(code, status)
            } else {
                format!("{}｜上游返回：{detail}", explain(code, status))
            },
        ));
    }

    let mut accumulator = SseAccumulator::new();
    let mut outcome = Outcome::default();
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut seen_done = false;

    while let Some(chunk) = stream.next().await {
        if cancelled() {
            return Err(ApiError::ai("RUN_CANCELLED", "Run 已取消"));
        }
        let chunk = chunk.map_err(|e| {
            ApiError::ai(
                "PROTOCOL_ERROR",
                format!("读上游流失败：{}", trim_error(&e.to_string())),
            )
        })?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        // 按**行**处理：一条 SSE 行可能被 TCP 切成两半，所以只处理到最后一个换行为止
        while let Some(index) = buffer.find('\n') {
            let line = buffer[..index].to_string();
            buffer.drain(..=index);
            if line.trim() == "data: [DONE]" || line.trim() == "data:[DONE]" {
                seen_done = true;
            }
            if let Some(frame) = accumulator.line(&line) {
                consume(&*adapter, &frame, &mut outcome, &mut on_event)?;
            }
        }
    }
    // 有的网关最后不发空行：把残留交给适配器
    if let Some(frame) = accumulator.flush() {
        consume(&*adapter, &frame, &mut outcome, &mut on_event)?;
    }

    // ★ **中途断流必须当场变成错误**，不能当成正常结束。
    //
    // 判据：既没有 `finish_reason` / `stop_reason`，也没有 `[DONE]` —— 三条路（OK / Responses /
    // Anthropic）里**任何一家正常收尾都至少满足一条**（Responses 用 `response.completed`，
    // Anthropic 用 `message_stop`，两者都会被适配器译成 [`StreamEvent::Finished`]）。
    //
    // 漏了这一条的后果很具体：网线被拔、代理掐连接、上游崩进程 —— 一律被当成
    // "它答完了"，于是库里是一条 `complete` 的半截回复。用户看到的是
    // **一句话说到一半就没了，而界面上没有任何异常标记**。
    if !seen_done && outcome.finish_reason.is_none() {
        return Err(ApiError::ai(
            "PROTOCOL_ERROR",
            "上游的流在中途断掉了（没有 finish_reason，也没有 [DONE]）—— 这条回复不完整，\
             已经收到的部分保留在这次 Run 里",
        ));
    }
    Ok(outcome)
}

/// 一帧 → 事件 → 回调，并把「这一轮收尾要用的东西」记进 [`Outcome`]。
fn consume<F>(
    adapter: &dyn crate::protocol::Adapter,
    frame: &Frame,
    outcome: &mut Outcome,
    on_event: &mut F,
) -> Result<(), ApiError>
where
    F: FnMut(StreamEvent) -> Result<(), ApiError>,
{
    if frame.data.trim() == "[DONE]" {
        return Ok(());
    }
    for event in adapter.interpret(frame) {
        match &event {
            StreamEvent::ProviderId(id) => {
                if outcome.provider_response_id.is_none() {
                    outcome.provider_response_id = Some(id.clone());
                }
            }
            StreamEvent::Usage(usage) => {
                outcome.usage = Some(merge_usage(outcome.usage.take(), usage));
            }
            StreamEvent::Finished(reason) => {
                if !reason.is_empty() {
                    outcome.finish_reason = Some(reason.clone());
                }
            }
            StreamEvent::ToolCallDelta { index, key, call_id, name, arguments_fragment } => {
                outcome.tool_calls.push((
                    *index,
                    key.clone(),
                    call_id.clone(),
                    name.clone(),
                    arguments_fragment.clone(),
                ));
            }
            _ => {}
        }
        on_event(event)?;
    }
    Ok(())
}

/// 一轮上游的收尾信息（`Idle`）。
#[derive(Debug, Default, Clone)]
pub struct Outcome {
    /// 上游的响应 id（进 `ai_messages.provider_response_id`，事后追溯用）
    pub provider_response_id: Option<String>,
    /// 这一轮的 usage（**原样 JSON**：形状三家不同，归一化交给界面）
    pub usage: Option<Value>,
    /// `finish_reason` / `stop_reason`
    pub finish_reason: Option<String>,
    /// 工具调用分片（**原样**转交给 [`crate::protocol::ToolCallAccumulator`] 拼装）
    pub tool_calls: Vec<(usize, String, Option<String>, Option<String>, Option<String>)>,
}

/// usage 的累加（AIH-057：多轮工具循环不能只算最后一轮）。
///
/// 归一到 **OpenAI 形状**（`prompt_tokens` / `completion_tokens` / `total_tokens`），
/// 因为界面的 token 显示读的就是它；Anthropic 的 `input_tokens` / `output_tokens`
/// 在这里映射过去。
pub fn merge_usage(previous: Option<Value>, incoming: &Value) -> Value {
    let read = |value: &Value, keys: &[&str]| -> i64 {
        keys.iter()
            .find_map(|key| value.get(*key).and_then(Value::as_i64))
            .unwrap_or(0)
    };
    let normalize = |value: &Value| -> (i64, i64) {
        (
            read(value, &["prompt_tokens", "input_tokens"]),
            read(value, &["completion_tokens", "output_tokens"]),
        )
    };
    let (mut input, mut output) = previous.as_ref().map(normalize).unwrap_or((0, 0));
    let (add_input, add_output) = normalize(incoming);
    input += add_input;
    output += add_output;
    serde_json::json!({
        "prompt_tokens": input,
        "completion_tokens": output,
        "total_tokens": input + output,
    })
}

/// 日志里只出现 `scheme://host:port/path`（**不带 query / fragment / userInfo**）。
pub fn safe_endpoint(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return "<无法解析的地址>".to_string();
    };
    let mut out = format!(
        "{}://{}",
        parsed.scheme(),
        parsed.host_str().unwrap_or("?")
    );
    if let Some(port) = parsed.port() {
        out.push_str(&format!(":{port}"));
    }
    out.push_str(parsed.path());
    out
}

/// 取主机名（决定走不走代理、以及鉴权头之外的一切）。
pub fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.host_str().map(str::to_string)
}

/// 上游/网络错误文本的压缩：换行变空格、按字符截断（进错误信息的只有这一句）。
fn trim_error(raw: &str) -> String {
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    one_line.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 状态码 → 稳定码：这几个档位**一个都不能混**（界面给的下一步完全不一样）。
    #[test]
    fn status_codes_map_to_stable_codes() {
        assert_eq!(classify_status(200), "OK");
        assert_eq!(classify_status(401), "MISSING_CREDENTIAL");
        assert_eq!(classify_status(403), "MISSING_CREDENTIAL");
        assert_eq!(classify_status(404), "PROTOCOL_ERROR");
        assert_eq!(classify_status(408), "PROVIDER_UNREACHABLE");
        assert_eq!(classify_status(429), "RATE_LIMIT");
        assert_eq!(classify_status(402), "QUOTA_EXCEEDED");
        assert_eq!(classify_status(400), "CONFIG_ERROR");
        assert_eq!(classify_status(422), "CONFIG_ERROR");
        assert_eq!(classify_status(500), "PROVIDER_UNREACHABLE");
        assert_eq!(classify_status(503), "PROVIDER_UNREACHABLE");
        // 每一档都要有一句人话（`detail` 不能是空的）
        for status in [400u16, 401, 402, 404, 408, 429, 500, 504] {
            let code = classify_status(status);
            assert!(!explain(code, status).is_empty());
            assert!(explain(code, status).contains(&status.to_string()), "人话里要有状态码");
        }
    }

    /// 脱敏：**先抹密钥再截断**（顺序反了会把密钥的半截留下）；中文按字符截断。
    #[test]
    fn redaction_removes_the_secret_before_truncating() {
        let secret = "sk-abcdef123456";
        let raw = format!("{{\"error\":\"bad key {secret}\",\"echo\":\"Bearer {secret}\"}}");
        let out = redact(&raw, Some(secret));
        assert!(!out.contains(secret), "密钥必须被抹掉：{out}");
        assert!(out.contains("***"));

        // 没给 secret 时，按形状兜底（有的网关回显请求头）
        let out = redact("Authorization: Bearer sk-live-0123456789 done", None);
        assert!(!out.contains("sk-live-0123456789"), "{out}");

        // 截断按字符（中文不能切出半个字）
        let long = "汉".repeat(ERROR_BODY_MAX + 50);
        let out = redact(&long, None);
        assert_eq!(out.chars().filter(|c| *c == '汉').count(), ERROR_BODY_MAX);
        assert!(out.ends_with('…'));

        // 短文本不动
        assert_eq!(redact("  hello  ", None), "hello");
    }

    /// 三家协议的鉴权头：**Bearer 与 x-api-key 不能混**（混了就是 401）。
    #[test]
    fn auth_headers_per_protocol() {
        assert_eq!(
            auth_header(Api::OpenAiCompletions, Some(" k ")),
            vec![("authorization", "Bearer k".to_string())]
        );
        assert_eq!(
            auth_header(Api::OpenAiResponses, Some("k")),
            vec![("authorization", "Bearer k".to_string())]
        );
        let anthropic = auth_header(Api::AnthropicMessages, Some("k"));
        assert_eq!(anthropic[0], ("x-api-key", "k".to_string()));
        assert_eq!(anthropic[1], ("anthropic-version", "2023-06-01".to_string()));
        // 没凭据 → 一个头都不加（有的本机网关不需要 key）
        assert!(auth_header(Api::OpenAiCompletions, None).is_empty());
        assert!(auth_header(Api::OpenAiCompletions, Some("   ")).is_empty());
    }

    /// usage 累加：多家形状归一，多轮相加（AIH-057）。
    #[test]
    fn usage_accumulates_across_rounds() {
        let openai = serde_json::json!({"prompt_tokens": 10, "completion_tokens": 4});
        let merged = merge_usage(None, &openai);
        assert_eq!(merged["prompt_tokens"], 10);
        assert_eq!(merged["completion_tokens"], 4);
        assert_eq!(merged["total_tokens"], 14);

        // 第二轮累加
        let merged = merge_usage(Some(merged), &serde_json::json!({"prompt_tokens": 5, "completion_tokens": 1}));
        assert_eq!(merged["prompt_tokens"], 15);
        assert_eq!(merged["total_tokens"], 20);

        // Anthropic 的键名也要认
        let anthropic = serde_json::json!({"input_tokens": 7, "output_tokens": 2});
        let merged = merge_usage(None, &anthropic);
        assert_eq!(merged["prompt_tokens"], 7);
        assert_eq!(merged["completion_tokens"], 2);

        // 缺键按 0（不 panic）
        let empty = merge_usage(None, &serde_json::json!({}));
        assert_eq!(empty["total_tokens"], 0);
    }

    /// 日志里的端点**不带 query / 凭据**（规则 1：绝不把 key 写进日志）。
    #[test]
    fn safe_endpoint_strips_query_and_credentials() {
        assert_eq!(
            safe_endpoint("https://api.deepseek.com/v1/chat/completions?key=sk-secret"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            safe_endpoint("https://user:pass@api.x.com:8443/v1/messages"),
            "https://api.x.com:8443/v1/messages"
        );
        assert_eq!(safe_endpoint("not a url"), "<无法解析的地址>");
        assert_eq!(
            host_of("https://api.deepseek.com/v1/chat/completions").as_deref(),
            Some("api.deepseek.com")
        );
    }
}
