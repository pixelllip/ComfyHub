//! `provider_probe` 的单元测试（主体代码在 [`provider_probe`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn 模型列表地址的两个候选() {
    // OpenAI 兼容：首选直接在 Base URL 后面拼 /models（带不带 /v1 由用户填的算数）
    assert_eq!(
        models_url_candidates("https://api.openai.com", "openai-completions"),
        vec![
            "https://api.openai.com/models".to_string(),
            "https://api.openai.com/v1/models".to_string(),
        ]
    );
    // 带了 /v1：首选就是它，次选去掉 /v1
    assert_eq!(
        models_url_candidates("https://api.openai.com/v1", "openai-completions"),
        vec![
            "https://api.openai.com/v1/models".to_string(),
            "https://api.openai.com/models".to_string(),
        ]
    );
    // Anthropic：没写 /v1 时首选与次选归一成同一个地址 → distinct 之后只剩一个
    assert_eq!(
        models_url_candidates("https://api.anthropic.com", "anthropic-messages"),
        vec!["https://api.anthropic.com/v1/models".to_string()]
    );
    // 写了 /v1：首选它，次选去掉 /v1 再拼
    assert_eq!(
        models_url_candidates("https://api.anthropic.com/v1", "anthropic-messages"),
        vec![
            "https://api.anthropic.com/v1/models".to_string(),
            "https://api.anthropic.com/models".to_string(),
        ]
    );
    // 尾斜杠要收拾干净
    assert_eq!(
        models_url_candidates("https://api.openai.com/v1/", "openai-responses"),
        vec![
            "https://api.openai.com/v1/models".to_string(),
            "https://api.openai.com/models".to_string(),
        ]
    );
}

#[test]
fn 数模型个数认两种信封() {
    assert_eq!(count_models(r#"{"data":[{"id":"a"},{"id":"b"}]}"#), Some(2));
    assert_eq!(count_models(r#"{"models":[{"id":"a"}]}"#), Some(1));
    assert_eq!(count_models(r#"[{"id":"a"}]"#), Some(1));
    assert_eq!(count_models("not json"), None);
}

#[test]
fn 从正文细化错误码() {
    assert_eq!(
        crate::provider::upstream::refine_from_body(r#"{"error":{"code":"insufficient_quota"}}"#),
        Some("QUOTA_EXCEEDED")
    );
    assert_eq!(
        crate::provider::upstream::refine_from_body("Invalid API key"),
        Some("MISSING_CREDENTIAL")
    );
    assert_eq!(
        crate::provider::upstream::refine_from_body("rate limit exceeded"),
        Some("RATE_LIMIT")
    );
    assert_eq!(
        crate::provider::upstream::refine_from_body("The model is not supported on this endpoint"),
        Some("PROTOCOL_ERROR")
    );
    assert_eq!(
        crate::provider::upstream::refine_from_body("something else"),
        None
    );
}

#[test]
fn 接口明确声明的能力逐维度读出来() {
    // 只有 id：什么都没声明
    let obj = serde_json::from_str::<Value>(r#"{"id":"m"}"#).unwrap();
    assert!(parse_declared_capabilities(obj.as_object().unwrap()).is_none());

    // 数组形态 + 混装 tools
    let obj: Value = serde_json::from_str(
        r#"{"id":"m","input_modalities":["text","image"],"capabilities":["vision","tools"]}"#,
    )
    .unwrap();
    let declared = parse_declared_capabilities(obj.as_object().unwrap()).unwrap();
    assert_eq!(
        declared.modalities.as_deref(),
        Some(&["text".to_string(), "image".to_string()][..])
    );
    assert_eq!(declared.tools, Some(true));
    assert_eq!(declared.reasoning, None, "没声明的维度必须是 None");

    // OpenRouter 的 architecture.modality
    let obj: Value =
        serde_json::from_str(r#"{"id":"m","architecture":{"modality":"text+image->text"}}"#)
            .unwrap();
    let declared = parse_declared_capabilities(obj.as_object().unwrap()).unwrap();
    assert_eq!(
        declared.modalities.as_deref(),
        Some(&["text".to_string(), "image".to_string()][..])
    );

    // 布尔开关
    let obj: Value =
        serde_json::from_str(r#"{"id":"m","supports_vision":true,"supports_reasoning":true}"#)
            .unwrap();
    let declared = parse_declared_capabilities(obj.as_object().unwrap()).unwrap();
    let modalities = declared.modalities.unwrap();
    assert!(modalities.contains(&"image".to_string()), "{modalities:?}");
    assert_eq!(declared.reasoning, Some(true));

    // 嵌套 capabilities
    let obj: Value = serde_json::from_str(
        r#"{"id":"m","capabilities":{"vision":true,"function_calling":false,"thinking":true}}"#,
    )
    .unwrap();
    let declared = parse_declared_capabilities(obj.as_object().unwrap()).unwrap();
    assert_eq!(declared.tools, Some(false));
    assert_eq!(declared.reasoning, Some(true));
}

#[test]
fn 候选_逐维度合并且来源如实() {
    let overlays = crate::provider::capability_catalog::Overlays::default();
    // ① 接口只声明模态 → 其它维度用默认，来源 = 接口声明
    let body = r#"{"data":[{"id":"some-random-model","input_modalities":["text","image"]}]}"#;
    let candidates = parse_candidates(body, &overlays);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["modalities"], json!(["text", "image"]));
    assert_eq!(candidates[0]["tools"], json!(true), "工具默认给上");
    assert_eq!(candidates[0]["reasoning"], json!(false));
    assert_eq!(candidates[0]["capabilitySource"], json!("discovered"));
    assert!(
        candidates[0]["capabilityNote"]
            .as_str()
            .unwrap()
            .contains("接口"),
        "{:?}",
        candidates[0]
    );

    // ② 接口什么都没说、内置目录也没有 → 仅文本 + 默认给工具 + unknown
    let body = r#"{"data":[{"id":"完全不认识的模型-xyz"}]}"#;
    let candidates = parse_candidates(body, &overlays);
    assert_eq!(candidates[0]["modalities"], json!(["text"]));
    assert_eq!(candidates[0]["capabilitySource"], json!("unknown"));
    assert!(
        candidates[0]["capabilityNote"]
            .as_str()
            .unwrap()
            .contains("仅文本"),
        "{:?}",
        candidates[0]
    );

    // ③ 重复 id 只留一条（`distinctBy { it.id }`）
    let body = r#"{"data":[{"id":"dup"},{"id":"dup"}]}"#;
    assert_eq!(parse_candidates(body, &overlays).len(), 1);

    // ④ 没有 id 的条目丢掉；坏 JSON 回空表
    assert!(parse_candidates(r#"{"data":[{"name":"没有id时 name 也算"}]}"#, &overlays).len() == 1);
    assert!(parse_candidates(r#"{"data":[{"object":"model"}]}"#, &overlays).is_empty());
    assert!(parse_candidates("不是 JSON", &overlays).is_empty());
}

#[test]
fn 内置目录命中时来源标_builtin_并带上规则名() {
    // 从真表里挑一条一定存在的规则（`capability_rules.rs` 是生成的，deepseek 一定在）
    let overlays = crate::provider::capability_catalog::Overlays::default();
    let Some(hit) = crate::provider::capability_catalog::lookup(&overlays, "deepseek-chat") else {
        // 规则表变了也不该让用例假绿：如实报出来
        panic!("内置目录里应当有 deepseek 的规则");
    };
    let body = r#"{"data":[{"id":"deepseek-chat"}]}"#;
    let candidates = parse_candidates(body, &overlays);
    assert_eq!(candidates[0]["capabilitySource"], json!("builtin"));
    assert_eq!(candidates[0]["reasoning"], json!(hit.reasoning));
    assert!(
        candidates[0]["capabilityNote"]
            .as_str()
            .unwrap()
            .contains(&hit.matched_by),
        "说明里要点出命中的规则：{:?}",
        candidates[0]
    );
    // 档位只在内置目录说"支持推理"时才预填（否则保存会被拒）
    if !hit.reasoning {
        assert!(candidates[0].get("thinkingEfforts").is_none());
    }
}
