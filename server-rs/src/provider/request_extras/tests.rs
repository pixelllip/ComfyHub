use super::*;

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn empty_columns_are_empty_extras() {
    let extras = RequestExtras::from_columns(None, None);
    assert!(extras.is_empty());
    assert!(extras.headers_json().is_none());
    assert!(extras.compat_json().is_none());
    assert_eq!(extras.describe(), "无");
}

/// 两列的形状就是 DTO 的形状：`headers_json` 是对象，查询参数在 `compat_json.query`。
#[test]
fn columns_round_trip() {
    let extras = RequestExtras::validated(
        map(&[("x-opencode-session", "ses_abc"), ("X-Trace", "1")]),
        map(&[("api-version", "2024-10-21")]),
    )
    .unwrap();

    let headers = extras.headers_json().unwrap();
    let compat = extras.compat_json().unwrap();
    assert!(headers.contains("\"x-opencode-session\":\"ses_abc\""));
    assert_eq!(
        compat, r#"{"query":{"api-version":"2024-10-21"}}"#,
        "查询参数只占 compat_json 的 query 键"
    );

    let back = RequestExtras::from_columns(Some(&headers), Some(&compat));
    assert_eq!(back, extras);
}

/// 坏 JSON 不能让整页 Provider 打不开：读侧当空，写侧才报错。
#[test]
fn broken_columns_read_as_empty() {
    let extras = RequestExtras::from_columns(Some("{"), Some("[]"));
    assert!(extras.is_empty());
    // 对象里混进非标量：那一项丢掉，其余保留
    let extras = RequestExtras::from_columns(Some(r#"{"X-A":"1","X-B":{"n":1}}"#), None);
    assert_eq!(extras.headers, map(&[("X-A", "1")]));
}

#[test]
fn validation_reports_the_offending_name() {
    let too_many = (0..=MAX_ENTRIES)
        .map(|i| (format!("X-{i}"), "1".to_string()))
        .collect();
    let error = RequestExtras::validated(too_many, BTreeMap::new()).unwrap_err();
    assert!(error.human().contains("最多"), "{}", error.human());

    let forbidden = RequestExtras::validated(map(&[("Host", "x")]), BTreeMap::new());
    assert!(forbidden.unwrap_err().human().contains("由程序生成"));

    let duplicate =
        RequestExtras::validated(map(&[("X-Trace", "1"), ("x-trace", "2")]), BTreeMap::new());
    assert!(
        duplicate.unwrap_err().human().contains("重复"),
        "名称不区分大小写，同名只能留一条"
    );

    let chinese_value = RequestExtras::validated(map(&[("X-Note", "你好")]), BTreeMap::new());
    assert!(
        chinese_value.unwrap_err().human().contains("可见 ASCII"),
        "请求头值不可发送时要指向具体那一条"
    );

    let newline = RequestExtras::validated(BTreeMap::new(), map(&[("q", "line\nbreak")]));
    assert!(newline.unwrap_err().human().contains("换行"));
}

/// 自定义头覆盖同名默认头（含大小写不同的写法），其余默认头保持不变。
#[test]
fn custom_headers_override_defaults() {
    let extras = RequestExtras::validated(
        map(&[
            ("x-opencode-session", "ses_1"),
            ("Authorization", "Token abc"),
        ]),
        BTreeMap::new(),
    )
    .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("accept", HeaderValue::from_static("text/event-stream"));
    headers.insert("authorization", HeaderValue::from_static("Bearer old"));
    extras.apply_headers(&mut headers).unwrap();

    assert_eq!(headers.get("accept").unwrap(), "text/event-stream");
    assert_eq!(
        headers.get("authorization").unwrap(),
        "Token abc",
        "同名头必须被覆盖，不能出现两条 Authorization"
    );
    assert_eq!(headers.get("x-opencode-session").unwrap(), "ses_1");
}

#[test]
fn query_is_appended_without_dropping_existing_pairs() {
    let extras =
        RequestExtras::validated(BTreeMap::new(), map(&[("api-version", "2024-10-21")])).unwrap();
    let url = extras
        .with_query("https://example.com/openai/v1/chat/completions?x=1")
        .unwrap();
    assert_eq!(
        url,
        "https://example.com/openai/v1/chat/completions?x=1&api-version=2024-10-21"
    );
    // 空表完全不动原地址
    assert_eq!(
        RequestExtras::default()
            .with_query("https://example.com/a")
            .unwrap(),
        "https://example.com/a"
    );
}

/// `{{session}}` 只由对话 ID 决定：同一对话固定、不同对话不同、形态可预测。
#[test]
fn session_token_is_stable_per_conversation_and_distinct_across_conversations() {
    let a = session_token("c-1");
    let b = session_token("c-1");
    let other = session_token("c-2");
    assert_eq!(a, b, "同一对话（含重试 / 工具轮）必须稳定");
    assert_ne!(a, other, "不同对话必须不同");
    assert_eq!(a.len(), 26);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert_eq!(
        a, "a6f7ef47ee8dc84af9056a3051",
        "算法与手机 harness 必须逐字节一致（同一对话在两端的标识相同）"
    );

    // 没有对话的请求（探测 / 校准 / 布局翻译）用固定作用域
    assert_eq!(session_token(STANDALONE_SCOPE), session_token(STANDALONE_SCOPE));
}

/// 占位符在发请求前展开，存库的原文不变。
#[test]
fn templates_expand_per_conversation_in_both_headers_and_query() {
    let extras = RequestExtras::validated(
        map(&[
            ("x-opencode-session", "ses_{{session}}"),
            ("x-conversation", "{{conversation}}"),
        ]),
        map(&[("q", "{{session}}")]),
    )
    .unwrap();
    // 写回库的是模板原文（手机同步过去才不会带上桌面那条对话的标识）
    assert_eq!(
        extras.headers["x-opencode-session"],
        "ses_{{session}}",
        "落库必须是模板，不是展开后的值"
    );

    let first = extras.resolve(Some("c-1"));
    let again = extras.resolve(Some("c-1"));
    let other = extras.resolve(Some("c-2"));
    assert_eq!(first, again);
    assert_ne!(first.headers["x-opencode-session"], other.headers["x-opencode-session"]);
    assert!(
        first.headers["x-opencode-session"]
            .starts_with("ses_"),
        "前缀由用户填，程序只替换变量"
    );
    assert_eq!(
        first.headers["x-opencode-session"],
        format!("ses_{}", session_token("c-1"))
    );
    assert_eq!(first.headers["x-conversation"], "c-1");
    assert_eq!(first.query["q"], session_token("c-1"));

    // 没有对话：{{conversation}} 换成 standalone，{{session}} 也仍然可用
    let standalone = extras.resolve(None);
    assert_eq!(standalone.headers["x-conversation"], STANDALONE_SCOPE);
    assert_eq!(standalone.query["q"], session_token(STANDALONE_SCOPE));
}

/// 不认识 / 没闭合的变量要当场报错，不能原样发出去换一个看不出原因的 400。
#[test]
fn unknown_or_unclosed_template_is_rejected() {
    let unknown = RequestExtras::validated(
        map(&[("x-session", "{{sesion}}")]),
        BTreeMap::new(),
    );
    assert!(
        unknown.unwrap_err().human().contains("不认识"),
        "拼错变量名时必须指出"
    );

    let unclosed =
        RequestExtras::validated(BTreeMap::new(), map(&[("q", "{{session")]));
    assert!(unclosed.unwrap_err().human().contains("没闭合"));

    // 两个变量都认识、可重复出现
    RequestExtras::validated(
        map(&[("x-a", "{{session}}-{{conversation}}-{{session}}")]),
        BTreeMap::new(),
    )
    .unwrap();
}
