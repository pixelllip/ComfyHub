//! `ai` 的单元测试（主体代码在 [`ai`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn no_env(_: &str) -> Option<String> {
    None
}

// -----------------------------------------------------------------------
//  Base URL：**对着真 Kotlin 基线量出来的真值表**（2026-09-25，8098）
// -----------------------------------------------------------------------

/// 三档错误文案逐条钉住 —— 这张表是拿真 Kotlin 后端一问一答问出来的，不是照文档猜的。
///
/// 为什么值得这么细：这三句话决定的**下一步动作**完全不同 ——
/// "不是合法 URL"要去改整串、"缺少主机名"要补主机、"只支持 http/https"要换协议。
/// 混档的话用户会在一个没错的地方反复改。
#[test]
fn base_url_的三档错误与_kotlin_逐条一致() {
    let err = |url: &str, trust: &str| normalize_base_url(Some(url), trust).unwrap_err().human().to_string();

    // ① 语法不过关 → "不是合法 URL"
    for bad in [
        "http://",
        "https://",
        "https://api.example.com/a b",
        "https://api.example.com/a\tb",
        "https://api.example.com/%zz",
        "https://api.example.com/a<b",
        "https://api.example.com/a>b",
        "https://api.example.com/a\"b",
        "https://api.example.com/a{b}",
        "https://api.example.com/a|b",
        "https://api.example.com/a\\b",
        "https://api.example.com/a^b",
        "https://api.example.com/a`b",
        "https://api.example.com/a[b]",
        "https://[zzz]:9",
    ] {
        assert_eq!(err(bad, "unsafe-any"), "Base URL 不是合法 URL", "{bad}");
    }

    // ② 解析得出来但 host 为 null → "缺少主机名"
    for missing in [
        "http:/foo",
        "http:foo",
        "https:///x",
        "https://api.example.com:-1/x",
        "https://:9/x",
    ] {
        assert_eq!(err(missing, "unsafe-any"), "Base URL 缺少主机名", "{missing}");
    }

    // ③ 没有 scheme / 不是 http(s) → "只支持 http/https"
    for scheme in ["//api.example.com", "api.example.com", "ftp://api.example.com"] {
        assert_eq!(err(scheme, "unsafe-any"), "Base URL 只支持 http/https", "{scheme}");
    }

    // ④ 其它四档
    assert_eq!(err("https://api.example.com?a=1", "unsafe-any"), "Base URL 不能带查询参数");
    assert_eq!(err("https://api.example.com?", "unsafe-any"), "Base URL 不能带查询参数");
    assert_eq!(err("https://api.example.com#x", "unsafe-any"), "Base URL 不能带 fragment");
    assert_eq!(err("https://api.example.com#", "unsafe-any"), "Base URL 不能带 fragment");
    assert_eq!(
        err("https://u:p@api.example.com", "unsafe-any"),
        "Base URL 不能包含用户名密码，请使用凭据配置"
    );
    assert_eq!(err("http://api.example.com", "public"), "公网端点必须使用 https");
    assert_eq!(err("", "unsafe-any"), "Base URL 不能为空");
    assert_eq!(err("https://0.0.0.0", "unsafe-any"), "该地址属于云元数据/保留地址，已拒绝");
}

/// 通过的那些：**返回值只去掉末尾的 `/`**，路径一个字都不许改（Java 也不改）。
///
/// `url` crate 会补 `/`、小写化 host、百分号编码 —— 拿它当返回值就会悄悄改写用户的路径，
/// 而"路径被改了"在界面上完全看不出来（下次请求发去另一个地方）。
#[test]
fn base_url_通过时只去掉末尾斜杠() {
    let ok = |url: &str| normalize_base_url(Some(url), "unsafe-any").unwrap();
    assert_eq!(ok("https://api.example.com"), "https://api.example.com");
    assert_eq!(ok("https://api.example.com/"), "https://api.example.com");
    assert_eq!(ok("https://api.example.com/v1/"), "https://api.example.com/v1");
    assert_eq!(ok("https://api.example.com//v1//"), "https://api.example.com//v1");
    // 大小写 / 非 ASCII 路径 / 合法转义：**原样保留**
    assert_eq!(ok("HTTPS://API.EXAMPLE.COM/V1"), "HTTPS://API.EXAMPLE.COM/V1");
    assert_eq!(ok("https://API.example.com/v1"), "https://API.example.com/v1");
    assert_eq!(ok("https://api.example.com/路径"), "https://api.example.com/路径");
    assert_eq!(ok("https://api.example.com/%E8%B7%AF"), "https://api.example.com/%E8%B7%AF");
    // 端口**不校验范围**（Java 也不校验）
    assert_eq!(ok("https://api.example.com:99999/x"), "https://api.example.com:99999/x");
    assert_eq!(ok("https://[::1]:9"), "https://[::1]:9");
    assert_eq!(ok("http://localhost:1"), "http://localhost:1");
    // 两端空白先 trim（界面粘贴常带）
    assert_eq!(ok("  https://api.example.com/v1  "), "https://api.example.com/v1");
}

#[test]
fn 凭据状态的三个分支一条都不能错() {        // ① ref 为空：不配置、不可写（连"能不能写"都是 false：没地方写）
    let s = describe_credential(None, no_env, |_| true);
    assert_eq!(s, CredentialStatus { configured: false, source: "none".into(), writable: false });
    let s = describe_credential(Some("   "), no_env, |_| true);
    assert_eq!(s.source, "none", "空白 ref 与没有 ref 同义");

    // ② 环境变量命中 → source=env 且 **writable=false**（AIH-013：界面不许改环境变量给的 key）
    let s = describe_credential(Some("OPENAI_KEY"), |n| (n == "OPENAI_KEY").then(|| "sk-x".into()), |_| false);
    assert_eq!(s, CredentialStatus { configured: true, source: "env".into(), writable: false });
    // 空串的环境变量 = 配了但没值
    let s = describe_credential(Some("OPENAI_KEY"), |_| Some(String::new()), |_| false);
    assert_eq!(s, CredentialStatus { configured: false, source: "env".into(), writable: false });

    // ③ 前缀那个名字也要认（`COMFYHUB_CRED_<ref>`）——认错了就等于"永远读不到那把 key"
    let s = describe_credential(Some("OPENAI_KEY"), |n| (n == "COMFYHUB_CRED_OPENAI_KEY").then(|| "sk-y".into()), |_| false);
    assert_eq!(s.source, "env");
    assert!(s.configured);

    // ④ 托管文件里有没有 → managed / none；**两种都可写**（界面要能改、也要能清一把配错的）
    let s = describe_credential(Some("OPENAI_KEY"), no_env, |_| true);
    assert_eq!(s, CredentialStatus { configured: true, source: "managed".into(), writable: true });
    let s = describe_credential(Some("OPENAI_KEY"), no_env, |_| false);
    assert_eq!(s, CredentialStatus { configured: false, source: "none".into(), writable: true });

    // ⑤ 环境变量优先于托管文件（与 Kotlin 的判定顺序一致）
    let s = describe_credential(Some("K"), |_| Some("v".into()), |_| true);
    assert_eq!(s.source, "env", "环境变量先判");
}

// -----------------------------------------------------------------------
//  模型目录：JSON 列的宽松解析（坏数据不许让整页打不开）
// -----------------------------------------------------------------------

#[test]
fn 模型目录的_json_列解析不炸也不悄悄改口径() {
    // 正常值
    assert_eq!(parse_string_array(Some(r#"["text","image"]"#)), vec!["text", "image"]);
    // 空 / 缺 / 坏 JSON / 类型不对 → 一律空表（Kotlin 那边是 runCatching + getOrDefault）
    for bad in [None, Some(""), Some("   "), Some("not json"), Some("{}"), Some("null")] {
        assert!(parse_string_array(bad).is_empty(), "{bad:?} 应当解析成空表");
    }
    // 数字 / 布尔按**字面文本**取（Kotlin `contentOrNull` 就是这么给的）
    assert_eq!(parse_string_array(Some(r#"[1,true]"#)), vec!["1", "true"]);
    // JSON null 元素丢掉
    assert_eq!(parse_string_array(Some(r#"["a",null]"#)), vec!["a"]);

    // 传输方式表：键是模态、值是数组；值不是数组 → 空数组（**不是**丢掉整个键）
    let map = parse_string_list_map(Some(r#"{"image":["inline_base64"],"audio":"x"}"#));
    assert_eq!(map.get("image").unwrap(), &vec!["inline_base64".to_string()]);
    assert_eq!(map.get("audio").unwrap(), &Vec::<String>::new());
    assert!(parse_string_list_map(Some("[1,2]")).is_empty());

    // 思考等级表：值可能是数字（Anthropic 的预算）→ 按字面文本；null 的键丢掉
    let efforts = parse_string_map(Some(r#"{"max":4096,"low":"low","off":null}"#));
    assert_eq!(efforts.get("max").unwrap(), "4096");
    assert_eq!(efforts.get("low").unwrap(), "low");
    assert!(!efforts.contains_key("off"), "JSON null 不该留下一个空值的等级");
}

/// `display_name` / `capability_source` 为 NULL 时的回落 —— 直接对着**计算出的行**测，
/// 因为真 schema 上这两列都是 NOT NULL，夹具里构造不出来（只有老 MySQL 脏行才会撞上）。
#[test]
fn 可空的显示名与能力来源都有回落值() {
    let conn = crate::infra::db::open_memory().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT 'p' AS provider_id, 'm-x' AS model_id, NULL AS display_name,
                    NULL AS input_modalities, NULL AS attachment_transports,
                    NULL AS mime_allowlist, NULL AS tools, NULL AS parallel_tools,
                    NULL AS reasoning, NULL AS thinking_efforts, '  ' AS thinking_format,
                    NULL AS context_window, NULL AS max_output_tokens,
                    NULL AS max_attachment_bytes, NULL AS max_attachment_count,
                    NULL AS capability_source, NULL AS capability_verified_at, NULL AS enabled",
        )
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    let row = rows.next().unwrap().unwrap();
    let model = model_from_row(row).unwrap();

    assert_eq!(model.display_name, "m-x", "显示名空着就回落成 model_id");
    assert_eq!(model.capability_source, "manual");
    assert!(model.thinking_format.is_none(), "空白串等于没声明");
    assert!(model.context_window.is_none());
    // 布尔列 NULL → false（`getBoolean` + `wasNull` 在 Kotlin 那边给的是默认值）
    assert!(!model.tools && !model.enabled);
    // 序列化出来的形状：可空键一个都不出现
    let value = serde_json::to_value(&model).unwrap();
    let mut got: Vec<&str> = value.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            "attachmentTransports", "capabilitySource", "displayName", "enabled", "id",
            "inputModalities", "mimeAllowlist", "parallelTools", "providerId", "reasoning",
            "thinkingEfforts", "tools",
        ]
    );
}
