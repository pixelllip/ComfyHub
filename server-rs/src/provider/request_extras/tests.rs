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
