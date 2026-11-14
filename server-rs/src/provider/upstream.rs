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
//! 后者是"地址/模型填错了"—— 界面显示的下一步完全不同。

use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::provider::protocol::{
    adapter_for, Adapter, Api, ChatTurn, Frame, ReasoningRequest, SseAccumulator, StreamEvent,
    ToolSpec,
};
use crate::web::routes::{ApiError, AppState};

/// 连接超时（Kotlin 那边 `connectTimeout = 5s`）。
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 整次请求的超时上限。流式响应会一直有数据，所以这个值只在"上游彻底不动"时兜底。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
/// 上游报错正文的截断上限（超过就没什么信息量了，还会把界面刷屏）。
pub const ERROR_BODY_MAX: usize = 1200;

/// **流在传输层断掉了**（不是协议 / 配置问题，也不是 HTTP 状态码拒绝）。
///
/// 两种现场都归这一档：① `bytes_stream()` 吐出一个 `reqwest::Error`（对端在回复中途
/// 把连接掐掉、代理 RST、读超时）；② 流"干净地"结束了，但既没有 `finish_reason`
/// 也没有 `[DONE]`（有的网关最后不发终止帧就 close）。
///
/// 单独一个码的理由：这两件事**改配置解决不了**，而界面上的 `PROTOCOL_ERROR` 会让人
/// 去核对 Base URL（2026-11-14 用户看到的正是这个误导，真正的原文是
/// `error decoding response body: connection closed before message completed`）。
/// 执行器认这个码决定"接着写"（见 [`crate::ai::executor`] 的断流续写）。
pub const STREAM_INTERRUPTED: &str = "STREAM_INTERRUPTED";

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
    /// Provider 级的额外请求头与查询参数（每一条请求都要带）
    pub extras: &'a crate::provider::request_extras::RequestExtras,
    /// 这次请求所属的对话 ID：额外参数里的 `{{session}}` / `{{conversation}}`
    /// 用它展开（同一对话固定、不同对话不同）。
    ///
    /// 没有对话的请求（布局翻译）传 `None`，按固定作用域展开。
    pub conversation_id: Option<&'a str>,
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
    // 混合网关最常见的问题：模型不属于当前协议的端点
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
        "PROTOCOL_ERROR" => {
            format!("上游说不认识这个端点（HTTP {status}）：检查 Base URL 与协议是否匹配")
        }
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
    builder
        .build()
        .map_err(|e| ApiError::internal(format!("HTTP 客户端创建失败：{e}")))
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

/// 等"这次 Run 被取消"：先看广播里已有的值，没有再等变化。
///
/// 之所以能当 `select!` 的一支：`watch::Receiver::changed` 是**取消安全**的
/// （被丢掉不会少收一次通知），而且接收者后建也能读到已经置过的 `true`。
async fn wait_cancelled(rx: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            // 发送端没了（执行器已收尾）：不再当成取消，一直等下去 —— 此时这次请求
            // 本来也已经被丢掉了，不会留在这里空转。
            std::future::pending::<()>().await;
        }
    }
}

/// 取消这个 Run 的统一错误（执行器按 `RUN_CANCELLED` 记 `cancelled` 终态）。
fn cancelled_error() -> ApiError {
    ApiError::ai("RUN_CANCELLED", "Run 已取消")
}

/// 发一次流式请求，把事件逐个交给 `on_event`。
///
/// `cancelled()` 是同步旗标（工具循环 / 回调里都能随手问一次）；`cancel` 是**取消广播**
/// —— 两者都要：只靠旗标时，主循环卡在"等上游开口 / 等下一个分片"里就没人看它，
/// 点了「停止」也只能等到上游吐下一个字（2026-11-13 现场：等了 10 分钟）。
/// 取消一律在**安全点**收尾（主循环负责落终态 + flush 已经说出去的话），
/// 直接强杀会把最后那句话丢掉（R6）。
pub async fn stream_chat<F>(
    request: ChatRequest<'_>,
    mut on_event: F,
    cancelled: impl Fn() -> bool,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<Outcome, ApiError>
where
    F: FnMut(StreamEvent) -> Result<(), ApiError>,
{
    let url = request.api.chat_url(request.base_url);
    // 额外参数里的变量按**这一条对话**展开：同一对话固定、不同对话不同。
    let extras = request.extras.resolve(request.conversation_id);
    // 查询参数在拼好端点之后再追加（Base URL 不允许自带查询串）
    let url = extras.with_query(&url)?;
    let host = host_of(&url).ok_or_else(|| {
        ApiError::ai(
            "CONFIG_ERROR",
            format!("Base URL 无法解析出主机名：{}", request.base_url),
        )
    })?;

    let secret = match request
        .credential_ref
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(reference) => Some(crate::mobile::auth::resolve_credential(
            request.state,
            reference,
        )?),
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
    // 默认头 → 鉴权头 → 自定义头（后者覆盖同名的前两者，大小写不敏感）
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT,
        reqwest::header::HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    for (name, value) in auth_header(request.api, secret.as_deref()) {
        let name = reqwest::header::HeaderName::from_static(name);
        let value = reqwest::header::HeaderValue::from_str(&value).map_err(|_| {
            ApiError::ai(
                "CONFIG_ERROR",
                "凭据里包含无法放进 HTTP 请求头的字符。请重新填写 API Key。",
            )
        })?;
        headers.insert(name, value);
    }
    extras.apply_headers(&mut headers)?;
    // 思考模式下的"思维链回传"被拒时，先补回传、再考虑降档（顺序见下面的 `loop`）。
    let mut thinking_forced_off = false;
    // 这一轮是不是**补了空 `reasoning` 项**才成功的（执行器据此让余下的轮次也补）。
    let mut echo_forced = false;
    let response = loop {
        let builder = client.post(&url).headers(headers.clone()).json(&body);

        // 只记 provider / 端点 / 协议 / 模型，**绝不记 header 与请求体**
        eprintln!(
            "Run 请求 endpoint={} adapter={} model={} tools={} route={}",
            safe_endpoint(&url),
            request.api.wire(),
            request.model,
            request.tools.len(),
            crate::infra::outbound::route(&host).describe()
        );

        // ⚠ 等响应头这一步**也必须能被取消叫醒**：上游迟迟不开口时这里能挂几百秒，
        // 只等 `send()` 的话"停止"完全没反应（现场就是这么卡住的）。
        let response = tokio::select! {
            biased;
            _ = wait_cancelled(&mut cancel) => return Err(cancelled_error()),
            response = builder.send() => response.map_err(|e| {
                ApiError::ai(
                    "PROVIDER_UNREACHABLE",
                    format!(
                        "连不上上游（{}）：{}｜本次路径：{}",
                        safe_endpoint(&url),
                        trim_error(&e.to_string()),
                        crate::infra::outbound::route(&host).describe()
                    ),
                )
            })?,
        };

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
        // 那一轮没有任何思维链可回传，再按思考模式发下去就是**恒定 400**。
        //
        // 补救按两档走，**先修回传、再降档**：
        //   ① 补一个空 `content` 的 reasoning 项重发 —— 思考照旧开着。
        //      这是根因的修法：缺的是"这一轮的思维链条目"，不是"思考这个能力"。
        //      ⚠ `deepseek` 方言下第一次构建就已经补过了，重发只会得到**同一个请求**
        //      （400 之后那次"重发"的请求体逐字节相同，属一次无效消耗）——
        //      所以先比对请求体，没变化就直接进第 ② 档。
        //   ② 只有补了还被拒，才把强度降成 `effort: none`（那一档上游不校验回传）。
        //      降档是**最后手段**：它会把这个 Run 余下所有轮次的思考一起关掉，
        //      代价是模型变笨，不能拿来当默认反应。
        if !thinking_forced_off && is_reasoning_passback_error(&detail) {
            if !echo_forced {
                if let Some((echoed, next)) = build_echo_retry(
                    adapter.as_ref(),
                    request.model,
                    request.turns,
                    request.tools,
                    &reasoning,
                    request.max_output_tokens,
                ) {
                    echo_forced = true;
                    reasoning = echoed;
                    body = next;
                    eprintln!(
                        "上游要求回传思维链，但这次请求里有轮次没有思维链：\
                         先补空 reasoning 项重发（**思考保持开启**）"
                    );
                    continue;
                }
                eprintln!(
                    "上游要求回传思维链，但这个请求体里每一轮已经都带上了（补空条目不会\
                     改变请求）：不再重发同一个请求，直接降档"
                );
            }
            thinking_forced_off = true;
            reasoning = ReasoningRequest {
                effort: Some("none".into()),
                wire_value: Some("none".into()),
                format: None,
                echo_all: true,
            };
            body = adapter.build_body(
                request.model,
                request.turns,
                request.tools,
                &reasoning,
                request.max_output_tokens,
            );
            eprintln!(
                "补了空 reasoning 项仍被拒：改用 effort=none 重发一次（本次 Run 余下的请求也沿用）\
                 ｜上游原文：{detail}"
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
    let mut outcome = Outcome {
        thinking_forced_off,
        echo_forced,
        ..Default::default()
    };
    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();
    let mut seen_done = false;

    loop {
        // 取消与"下一个分片"同时等：上游想了半分钟没吐字时，点「停止」也当场收尾，
        // 不用等它先开口（那时旗标没人读 = 界面一直停在「已请求停止…」）。
        let next = tokio::select! {
            biased;
            _ = wait_cancelled(&mut cancel) => return Err(cancelled_error()),
            next = stream.next() => next,
        };
        let Some(chunk) = next else { break };
        if cancelled() {
            return Err(cancelled_error());
        }
        let chunk = chunk.map_err(|e| {
            // reqwest 的 `Display` 只有 "error decoding response body" 这一句 —— 真正的原因
            // （对端关连接 / 读超时 / 代理 RST）在 `source()` 链上。不打出来，现场就只剩一句
            // 没有下文的英文，谁也判断不出该改哪儿（2026-11-14 的排查卡在这里）。
            let cause = error_chain(&e);
            eprintln!("上游流读取失败：{cause}");
            ApiError::ai(
                STREAM_INTERRUPTED,
                format!("上游在回复中途把连接断开了（已经收到的内容保留）｜{cause}"),
            )
        })?;
        // ⚠ 缓冲按**字节**攒：SSE 行是 UTF-8，一个汉字可能正好横跨两个分片，
        // 逐片 `String::from_utf8_lossy` 会把它拆成两个替换字符（正文里就冒出乱码）。
        buffer.extend_from_slice(&chunk);
        // 按**行**处理：一条 SSE 行可能被 TCP 切成两半，所以只处理到最后一个换行为止
        while let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
            let line = String::from_utf8_lossy(&buffer[..index]).to_string();
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
    // "它答完了"，因此库中留下一条 `complete` 的不完整回复。表现为
    // **一句话说到一半就没了，而界面上没有任何异常标记**。
    if !seen_done && outcome.finish_reason.is_none() {
        return Err(ApiError::ai(
            STREAM_INTERRUPTED,
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
            StreamEvent::ToolCallDelta {
                index,
                key,
                call_id,
                name,
                arguments_fragment,
            } => {
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

/// 一条工具调用分片：`(分片序号, call id, 函数名, 参数片段, 另一个可选 id)`。
pub type ToolCallDelta = (
    usize,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

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
    pub tool_calls: Vec<ToolCallDelta>,
    /// 这一次请求是不是**补了空 `reasoning` 项**才成功的（执行器据此让余下的轮次也补）。
    ///
    /// 与 [`Self::thinking_forced_off`] 分开：补回传**不动思考强度**，
    /// 只是把缺的那一条思维链补成"这一轮确实没思考"。
    pub echo_forced: bool,
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
/// 那类配置错误也静默降档为非思考，就再也看不到真正的原因。
pub fn is_reasoning_passback_error(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    let mentions_reasoning = lower.contains("reasoning") || lower.contains("thinking");
    let mentions_echo = lower.contains("passed back")
        || lower.contains("must be passed")
        || lower.contains("pass back");
    let complains_about_empty = lower.contains("empty") || lower.contains("blank");
    mentions_reasoning && (mentions_echo || complains_about_empty)
}

/// 再"补一次空 `reasoning` 项"**会不会真的改变请求体**。
///
/// `deepseek` 方言下第一次 [`Adapter::build_body`] 就已经把缺的条目补齐了
/// （[`crate::provider::protocol`] 的 `reasoning_echo_required` 认方言，不认 `echo_all`），
/// 因此 400 之后那次"补全后重发"的请求体**逐字节相同** —— 重发只是一次无效往返
/// 和一次输入 token（2026-11-09 现场：400 之后重发的就是同一条请求）。
///
/// 返回 `Some((新的思考参数, 新请求体))` 表示重发有意义；`None` 表示补了也白补，直接降档。
fn build_echo_retry(
    adapter: &dyn Adapter,
    model: &str,
    turns: &[ChatTurn],
    tools: &[ToolSpec],
    reasoning: &ReasoningRequest,
    max_output_tokens: Option<i64>,
) -> Option<(ReasoningRequest, Value)> {
    let mut echoed = reasoning.clone();
    echoed.echo_all = true;
    let next = adapter.build_body(model, turns, tools, &echoed, max_output_tokens);
    let current = adapter.build_body(model, turns, tools, reasoning, max_output_tokens);
    (next != current).then_some((echoed, next))
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
            raw.get(parent)
                .and_then(|child| child.get(key))
                .and_then(Value::as_i64)
                .unwrap_or(0)
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

        Self {
            input,
            output,
            cached,
            reasoning,
        }
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
    previous
        .unwrap_or_default()
        .merge(TokenUsage::from_raw(incoming))
}

/// 日志里只出现 `scheme://host:port/path`（**不带 query / fragment / userInfo**）。
pub fn safe_endpoint(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return "<无法解析的地址>".to_string();
    };
    let mut out = format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or("?"));
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

/// 把错误链摊成一句话（`外层：里层：根因`）。
///
/// # 为什么必须有它
///
/// `reqwest::Error` 的 `Display` **只有一句**：`error decoding response body` /
/// `error sending request`。真正的 `connection closed before message completed` /
/// `operation timed out` 挂在 `source()` 链上，`to_string()` 拿不到。
/// 2026-11-14 现场看到的就是那句没有下文的英文 —— 界面上只写"读上游流失败：
/// error decoding response body"，日志里也没有别的，排查时等于什么都没说。
///
/// 去重是为了容忍"外层只是在复述内层"的错误类型（避免 `a：a：b`）。
pub fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = current {
        let text = e.to_string();
        let text = text.trim();
        if !text.is_empty() && !parts.iter().any(|seen| seen == text) {
            parts.push(text.to_string());
        }
        current = e.source();
    }
    parts.join("：")
}

#[cfg(test)]
mod tests;
