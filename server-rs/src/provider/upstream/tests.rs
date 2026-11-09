//! `upstream` 的单元测试（主体代码在 [`upstream`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;
use crate::provider::protocol::{OpenAiResponsesAdapter, ThinkingFormat, ToolCallRef};

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
        assert!(
            explain(code, status).contains(&status.to_string()),
            "人话里要有状态码"
        );
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
    assert_eq!(
        anthropic[1],
        ("anthropic-version", "2023-06-01".to_string())
    );
    // 没凭据 → 一个头都不加（有的本机网关不需要 key）
    assert!(auth_header(Api::OpenAiCompletions, None).is_empty());
    assert!(auth_header(Api::OpenAiCompletions, Some("   ")).is_empty());
}

/// **"思考模式必须回传思维链"那一条 400 的判据**（2026-10-25 现场）。
///
/// 只认"两样都提到"的正文：既要 `reasoning` / `thinking`，又要"回传"。
/// 看到 `reasoning` 就重发，会把"这个模型根本不支持思考"那类配置错误悄悄降档 ——
/// 用户从此看不到真正的原因。
#[test]
fn reasoning_passback_error_is_recognised() {
    let real = r#"{"error":{"message":"The `reasoning_text` in the thinking mode must be passed back to the API. (request_id: abc)","type":"invalid_request_error"}}"#;
    assert!(is_reasoning_passback_error(real));
    // 网关把措辞换成 thinking + pass back 也要认
    assert!(is_reasoning_passback_error(
        "thinking mode: reasoning must be passed back"
    ));
    // "不允许空的思考内容"也是同一件事的变体（我们给没思考的工具轮补的是空 content 项）
    assert!(is_reasoning_passback_error(
        r#"{"error":{"message":"reasoning content must not be empty"}}"#
    ));
    // 只提到 reasoning、或只提到回传 —— 都不认
    assert!(!is_reasoning_passback_error(
        "this model does not support reasoning"
    ));
    assert!(!is_reasoning_passback_error(
        "tool result must be passed back"
    ));
}

/// **补空条目不能变成"重发同一个请求"**（2026-11-09 现场）。
///
/// `deepseek` 方言下 `build_body` 第一次就把缺的 `reasoning` 项补上了，所以被 400 之后
/// 再"补一次"拿到的是逐字节相同的请求 —— [`build_echo_retry`] 必须返回 `None`，
/// 让降档直接发生（少烧一次 400 的往返与输入 token）。
/// 反过来，模型没声明方言时把 `echo_all` 打开**真的**会多一个 reasoning 项 ——
/// 那种情况必须重发（这条分支是 2026-11-01 加的，别顺手删掉）。
#[test]
fn echo_retry_is_skipped_when_the_body_would_not_change() {
    let silent = ChatTurn {
        role: "assistant".into(),
        tool_calls: vec![ToolCallRef {
            id: "call_1".into(),
            name: "t".into(),
            arguments: "{}".into(),
        }],
        ..Default::default()
    };
    let deepseek = ReasoningRequest {
        effort: Some("high".into()),
        wire_value: Some("high".into()),
        format: Some(ThinkingFormat::DeepSeek),
        echo_all: false,
    };
    assert!(
        build_echo_retry(
            &OpenAiResponsesAdapter,
            "m",
            std::slice::from_ref(&silent),
            &[],
            &deepseek,
            None
        )
        .is_none(),
        "方言是 deepseek 时第一次就补过了，重发只是同一个请求"
    );

    let plain = ReasoningRequest {
        format: Some(ThinkingFormat::OpenAi),
        ..deepseek
    };
    let retry = build_echo_retry(&OpenAiResponsesAdapter, "m", &[silent], &[], &plain, None)
        .expect("没声明方言时打开 echo_all 会真的多一个 reasoning 项，必须重发");
    assert_eq!(retry.1["input"][0]["type"], "reasoning");
    assert_eq!(
        retry.1["input"][0]["content"],
        json!([{ "type": "reasoning_text", "text": crate::provider::protocol::NO_REASONING_TEXT }])
    );
}

/// usage 累加：多家形状归一，多轮相加（AIH-057）。
///
/// ⚠ 重点在**四个字段一个都不能少**：`prompt_tokens` / `completion_tokens` 保住了
/// 不代表对了 —— 2026-10 之前这一版只留了那两个数，于是 `cachedTokens` / `reasoningTokens`
/// 在桌面上永久是 0（界面那行"缓存命中"从来没出现过）。细账见下一条。
#[test]
fn usage_accumulates_across_rounds() {
    let openai = serde_json::json!({"prompt_tokens": 10, "completion_tokens": 4});
    let merged = merge_usage(None, &openai);
    assert_eq!(merged.input, 10);
    assert_eq!(merged.output, 4);
    assert_eq!(merged.to_json()["inputTokens"], 10);
    assert_eq!(merged.to_json()["outputTokens"], 4);

    // 第二轮累加
    let merged = merge_usage(
        Some(merged),
        &serde_json::json!({"prompt_tokens": 5, "completion_tokens": 1}),
    );
    assert_eq!(merged.input, 15);
    assert_eq!(merged.output, 5);

    // Anthropic 的键名也要认
    let merged = merge_usage(
        None,
        &serde_json::json!({"input_tokens": 7, "output_tokens": 2}),
    );
    assert_eq!(merged.input, 7);
    assert_eq!(merged.output, 2);

    // 缺键按 0（不 panic）
    assert_eq!(
        merge_usage(None, &serde_json::json!({})),
        TokenUsage::default()
    );
}

/// **缓存命中与思考 token 不许丢** —— 这两条是"前缀缓存到底有没有生效"的唯一度量。
///
/// 契约（`docs/run-event-contract.md` §2）要求这四个数真的送到界面；老实现只归一化输入/输出两个数，
/// 于是 `cachedTokens` 永远是 0 —— **优化缓存的人连自己有没有优化成功都看不见**。
#[test]
fn cache_and_reasoning_tokens_survive_normalization() {
    // OpenAI 兼容：命中藏在 `prompt_tokens_details` 里，而且 `prompt_tokens` **已经含**它
    let usage = merge_usage(
        None,
        &serde_json::json!({
            "prompt_tokens": 1000,
            "completion_tokens": 50,
            "prompt_tokens_details": { "cached_tokens": 896 },
            "completion_tokens_details": { "reasoning_tokens": 30 },
        }),
    );
    assert_eq!(
        usage.input, 1000,
        "OpenAI 的 prompt_tokens 本来就含命中，别再叠加一次"
    );
    assert_eq!(usage.cached, 896);
    assert_eq!(usage.reasoning, 30);

    // Anthropic：`input_tokens` **不含**命中，而且读 / 写缓存是两个字段
    let anthropic = merge_usage(
        None,
        &serde_json::json!({
            "input_tokens": 4,
            "output_tokens": 9,
            "cache_read_input_tokens": 896,
            "cache_creation_input_tokens": 100,
        }),
    );
    assert_eq!(
        anthropic.input, 1000,
        "4 + 896(读) + 100(写)：输入要含命中与写缓存"
    );
    assert_eq!(
        anthropic.cached, 896,
        "命中只算**读**；cache_creation 是写（1.25×），不算命中"
    );
    assert_eq!(anthropic.output, 9);

    // 只给 total_tokens 的网关：口径退化，全记在输入上（§5.4 记着这条）
    let total_only = merge_usage(None, &serde_json::json!({"total_tokens": 42}));
    assert_eq!(total_only.input, 42);
    assert_eq!(total_only.output, 0);

    // 归一化过的形状要能**原样读回来**（`merge_usage` 的上一轮就是它，重启回填也一样）
    assert_eq!(merge_usage(None, &anthropic.to_json()), anthropic);

    // 送到界面的键**一个不多一个不少**（契约 §2 就是这四个）
    let json = anthropic.to_json();
    let keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec![
            "inputTokens",
            "outputTokens",
            "cachedTokens",
            "reasoningTokens"
        ]
    );
    assert_eq!(json["cachedTokens"], 896);
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
