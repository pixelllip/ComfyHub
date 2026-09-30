//! `upstream` 的单元测试（主体代码在 [`upstream`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

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
