//! 上游 LLM 客户端（P4 第 29 步）—— HTTP 那一半；协议那一半在 [`crate::provider::protocol`]。
//!
//! # 这个模块只做三件事
//!
//! 1. **拼 URL + 鉴权头**（三家不一样，见 [`crate::provider::protocol::Api`]）；
//! 2. **发出去、把 SSE 逐行喂给适配器**，吐出一串 [`crate::provider::protocol::StreamEvent`]；
//! 3. **把失败归到稳定错误码**（AIH-024）—— 前端与 harness 都不解析供应商文案。
//!
//! 外加一件顺带做的：**把 `usage` 归一化**（[`TokenUsage`]）。三家的形状差别很大、
//! 同一件事叫三个名字，而界面与落库只要"输入 / 输出 / 缓存命中 / 思考"这四个数，
//! 所以归一化**只有这一处实现**。
//! ⚠ `cachedTokens` 与 `reasoningTokens` **一个都不能丢** —— 前者是判断前缀缓存有没有生效的
//! 唯一度量（2026-09 切 Rust 时丢过一次，界面上那行「缓存命中」因此从未出现过）。
//!
//! # 四条硬规则（照抄 Kotlin `AiUpstream` 的文件头）
//!
//! 1. **绝不记录 Authorization / x-api-key**：日志只写 Provider ID、脱敏端点、状态码、稳定码；
//! 2. **不跟随重定向**（`Redirect::NEVER`）—— 免得密钥被带到别的域名；
//! 3. 出网前按 [`crate::infra::outbound`] 复核目标地址（**公网走代理、本机/内网强制直连**）；
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
use serde_json::{json, Value};

use crate::provider::protocol::{adapter_for, Api, ChatTurn, Frame, ReasoningRequest, SseAccumulator, StreamEvent, ToolSpec};
use crate::web::routes::{ApiError, AppState};

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

/// 造一个 HTTP 客户端：**不跟随重定向** + 按 [`crate::infra::outbound`] 选代理。
///
/// 每次请求现造一个：代理是按**目标主机**定的（公网走、内网直连），
/// 而 `reqwest::Client` 的代理是客户端级的。造客户端的开销远小于一次 HTTPS 握手。
pub fn client_for(host: &str) -> Result<reqwest::Client, ApiError> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        // 规则 2：**绝不跟随重定向**（密钥不能被带到别的域名）
        .redirect(reqwest::redirect::Policy::none());
    match crate::infra::outbound::route(host) {
        crate::infra::outbound::Route::Proxy(addr) => {
            let proxy = reqwest::Proxy::all(format!("http://{addr}"))
                .map_err(|e| ApiError::internal(format!("代理地址不合法：{e}")))?;
            builder = builder.proxy(proxy);
        }
        crate::infra::outbound::Route::Direct => {
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
        Some(reference) => Some(crate::mobile::auth::resolve_credential(request.state, reference)?),
        None => None,
    };

    let adapter = adapter_for(request.api);
    let mut reasoning = request.reasoning.clone();
    let mut body = adapter.build_body(
        request.model,
        request.turns,
        request.tools,
        &reasoning,
        request.max_output_tokens,
    );

    let client = client_for(&host)?;
    // 思考模式下的"思维链回传"被上游拒绝时，我们把思考关掉重发**一次**，
    // 并把这个事实交给执行器（这个 Run 余下的请求都沿用 effort=none，见 `Outcome`）。
    let mut thinking_forced_off = false;
    let response = loop {
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
            crate::infra::outbound::route(&host).describe()
        );

        let response = builder.send().await.map_err(|e| {
            ApiError::ai(
                "PROVIDER_UNREACHABLE",
                format!(
                    "连不上上游（{}）：{}｜本次路径：{}",
                    safe_endpoint(&url),
                    trim_error(&e.to_string()),
                    crate::infra::outbound::route(&host).describe()
                ),
            )
        })?;

        let status = response.status().as_u16();
        if (200..=299).contains(&status) {
            break response;
        }

        let raw = response.text().await.unwrap_or_default();
        let code = classify_status(status);
        let detail = redact(&raw, secret.as_deref());
        eprintln!("上游返回 {status}（code={code}）: {detail}");

        // ⚠ **思考模式下"模型这一轮没思考"是救得回来的**（2026-10-25 现场）：
        // DeepSeek 的 Responses 要求工具轮逐轮回传思维链，而模型有些轮次**根本不思考**
        // （实测：只发一个工具调用、2.3 秒返回，`reasoning_tokens` 为 0）——
        // 那一轮没有任何思维链可回传，再按思考模式发下去就是**恒定 400**，
        // 用户的感受是"AI 走了大量弯路、每条消息最后都报同一个错"。
        // 唯一的出路是**把思考关掉**：`effort: none` 那一档不校验回传（同一个请求体 200）。
        if !thinking_forced_off && is_reasoning_passback_error(&detail) {
            thinking_forced_off = true;
            reasoning = ReasoningRequest {
                effort: Some("none".into()),
                wire_value: Some("none".into()),
                format: None,
            };
            body = adapter.build_body(
                request.model,
                request.turns,
                request.tools,
                &reasoning,
                request.max_output_tokens,
            );
            eprintln!(
                "上游要求回传思维链，但这次请求里有工具轮没有思维链（模型没思考）：\
                 改用 effort=none 重发一次（本次 Run 余下的请求也沿用）"
            );
            continue;
        }

        return Err(ApiError::ai(
            code,
            if detail.is_empty() {
                explain(code, status)
            } else {
                format!("{}｜上游返回：{detail}", explain(code, status))
            },
        ));
    };

    let mut accumulator = SseAccumulator::new();
    let mut outcome = Outcome::default();
    outcome.thinking_forced_off = thinking_forced_off;
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
    adapter: &dyn crate::provider::protocol::Adapter,
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
    /// 这一轮的 usage（**已经归一化**，形状见 [`TokenUsage`]）
    pub usage: Option<TokenUsage>,
    /// `finish_reason` / `stop_reason`
    pub finish_reason: Option<String>,
    /// 工具调用分片（**原样**转交给 [`crate::provider::protocol::ToolCallAccumulator`] 拼装）
    pub tool_calls: Vec<(usize, String, Option<String>, Option<String>, Option<String>)>,
    /// 这一次请求是不是**因为思维链回传被拒而关掉思考重发**的（见 [`stream_chat`]）。
    ///
    /// 执行器据此把 `effort=none` 沿用到本次 Run 的余下请求：重发产生的这一轮同样没有
    /// 思维链，继续按思考模式发下去会一轮一次 400（每次都要多花一次输入 token 的钱）。
    pub thinking_forced_off: bool,
}

/// 上游是不是在说"思考模式下必须把思维链回传"（DeepSeek Responses 的原文）：
/// `The \`reasoning_text\` in the thinking mode must be passed back to the API.`
///
/// 判据宽松到"同时提到 reasoning / thinking 与回传"即可（各家网关的措辞会漂），
/// 也认"不允许空的思考内容"那类变体（`reasoning content must not be empty` ——
/// 见 `protocol.rs` 里给"没思考的工具轮"补的那个空 content 项）。
///
/// 但**必须两样都命中** —— 只看到 `reasoning` 就重发，会把"这个模型不支持思考"
/// 那类配置错误也悄悄降档成非思考，用户就再也看不到真正的原因了。
pub fn is_reasoning_passback_error(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    let mentions_reasoning = lower.contains("reasoning") || lower.contains("thinking");
    let mentions_echo = lower.contains("passed back")
        || lower.contains("must be passed")
        || lower.contains("pass back");
    let complains_about_empty = lower.contains("empty") || lower.contains("blank");
    mentions_reasoning && (mentions_echo || complains_about_empty)
}

/// 归一化后的 token 用量 —— **契约的那四个字段**（`docs/run-event-contract.md` §2）。
///
/// # 为什么要有这个类型（而不是把供应商的 JSON 原样透传）
///
/// 三家的形状差别很大，**同一件事在三家叫三个名字**；界面只画那四个数，
/// 所以必须有一条唯一的归一化实现。真源就是这里 ——
/// 前端 `AiTokenUsage.fromRaw`（`lib/models/ai_models.dart`）只是**对库里老数据的兼容读取**，
/// 不是第二份实现（老数据存的是 Kotlin 时代的供应商原文）。
///
/// | 这里 | OpenAI 兼容 | Anthropic |
/// | --- | --- | --- |
/// | `input` | `prompt_tokens` | `input_tokens + cache_read + cache_creation` |
/// | `output` | `completion_tokens` | `output_tokens` |
/// | `cached` | `prompt_tokens_details.cached_tokens` | `cache_read_input_tokens` |
/// | `reasoning` | `completion_tokens_details.reasoning_tokens` | （含在 `output_tokens` 里，无独立字段） |
///
/// # 两条口径（⚠ 改之前先读，前端按这两条画界面）
///
/// 1. **`input` 是"这一次请求一共读了多少 prompt token"，含命中的那一部分。**
///    OpenAI 的 `prompt_tokens` 本来就把命中的算进去了，而 Anthropic 的 `input_tokens`
///    **不含** `cache_read_input_tokens` —— 不把它抬到同一个口径，"输入"与"缓存命中"
///    就不是同一个分母（Anthropic 那边会算出 >100% 的命中率）。
/// 2. **`cached` 只算"读命中"（便宜的那部分），不算"写缓存"。**
///    Anthropic 的 `cache_creation_input_tokens` 按 1.25× 计价，它是**输入但不是命中** ——
///    算进 `cached` 会给出一个虚高的命中率。它已经含在 `input` 里。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    /// 这一次请求一共读了多少 prompt token（**含** `cached`）
    pub input: i64,
    pub output: i64,
    /// 缓存**读命中**的那部分输入
    pub cached: i64,
    /// 思考 token
    pub reasoning: i64,
}

impl TokenUsage {
    /// 从上游给的 usage JSON 读出来。
    ///
    /// 认 OpenAI / Anthropic 两家方言，**也认自己吐出去的那个形状**（`inputTokens` 那一套）——
    /// 后者是必须的：`merge_usage` 是"上一轮的归一化结果 + 这一轮的原样 JSON"，
    /// 而 Run 的 usage 会落库（`ai_messages.usage_json` / `ai_runs.usage_json`），
    /// 回填或重试时拿到手里的可能已经是归一化过的。
    ///
    /// 认不出来的键一律按 0，**不编数字**（契约：没有就是 0，界面不显示占位）。
    pub fn from_raw(raw: &Value) -> Self {
        let num = |key: &str| -> Option<i64> { raw.get(key).and_then(Value::as_i64) };
        let nested = |parent: &str, key: &str| -> i64 {
            raw.get(parent).and_then(|child| child.get(key)).and_then(Value::as_i64).unwrap_or(0)
        };
        // 命中：同一件事有好几个来源，取**最大**的那个（有的网关两个都发）。
        // ⚠ 前端 `AiTokenUsage.fromRaw` 用的是同一条规则（取最大）—— 两边别漂。
        let cached = [
            nested("prompt_tokens_details", "cached_tokens"),
            nested("input_tokens_details", "cached_tokens"),
            num("cache_read_input_tokens").unwrap_or(0),
            num("cachedTokens").unwrap_or(0),
        ]
        .into_iter()
        .max()
        .unwrap_or(0);
        let cache_creation = num("cache_creation_input_tokens").unwrap_or(0);
        // 输入：① 自己归一化过的形状 → ② OpenAI 的 `prompt_tokens`（已含命中）
        //      → ③ Anthropic 的 `input_tokens`（**不含**命中，要把读/写缓存补回去）
        //      → ④ 只给 `total_tokens` 的网关（口径退化，§5.4 记着这条）。
        let input = num("inputTokens")
            .or_else(|| num("prompt_tokens"))
            .or_else(|| {
                let anthropic = num("input_tokens");
                let hit = num("cache_read_input_tokens");
                (anthropic.is_some() || hit.is_some())
                    .then(|| anthropic.unwrap_or(0) + hit.unwrap_or(0) + cache_creation)
            })
            .or_else(|| num("total_tokens"))
            .unwrap_or(0);
        let output = num("outputTokens")
            .or_else(|| num("completion_tokens"))
            .or_else(|| num("output_tokens"))
            .unwrap_or(0);
        let reasoning = [
            nested("completion_tokens_details", "reasoning_tokens"),
            nested("output_tokens_details", "reasoning_tokens"),
            num("reasoning_tokens").unwrap_or(0),
            num("reasoningTokens").unwrap_or(0),
        ]
        .into_iter()
        .max()
        .unwrap_or(0);

        Self { input, output, cached, reasoning }
    }

    /// 逐项相加（AIH-057：多轮工具循环不能只算最后一轮）。
    pub fn merge(self, other: Self) -> Self {
        Self {
            input: self.input + other.input,
            output: self.output + other.output,
            cached: self.cached + other.cached,
            reasoning: self.reasoning + other.reasoning,
        }
    }

    /// 发给界面与落库的形状（**契约 §2 的四个键，一个不多一个不少**）。
    ///
    /// `usage.updated` 的载荷就是它本身（平铺，与手机 harness 的 `EventPayloads.usage` 同形），
    /// `message.completed` 把它挂在 `usage` 键下。
    pub fn to_json(self) -> Value {
        json!({
            "inputTokens": self.input,
            "outputTokens": self.output,
            "cachedTokens": self.cached,
            "reasoningTokens": self.reasoning,
        })
    }
}

/// usage 的累加：把这一轮的**原样 JSON** 归一并加到前面几轮上（AIH-057）。
///
/// ⚠ 别退回"只取最后一轮"或"只累加 prompt/completion 两个数"——
/// 后者正是"缓存命中在桌面上永远是 0"的那次回归：`cached` 与 `reasoning`
/// 是分开的字段，被丢掉之后**界面看不出来、账单也看不出来**。
pub fn merge_usage(previous: Option<TokenUsage>, incoming: &Value) -> TokenUsage {
    previous.unwrap_or_default().merge(TokenUsage::from_raw(incoming))
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
mod tests;
