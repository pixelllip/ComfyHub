//! `capability_refresh` 的单元测试（主体代码在 [`capability_refresh`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn obj(json: &str) -> Map<String, Value> {
    serde_json::from_str::<Value>(json).unwrap().as_object().unwrap().clone()
}

#[test]
fn 跳过判据三档() {
    // NO_DOWNLOAD 一刀关（优先级最高）
    assert!(skip_reason(true, true, true).unwrap().contains("COMFYHUB_NO_DOWNLOAD=1"));
    // 没强制 + 没过期 → 跳过（后台巡检那条路）
    assert!(skip_reason(false, false, false).unwrap().contains("还没过期"));
    // 强制 → 不跳
    assert!(skip_reason(true, false, false).is_none());
    // 过期 → 不跳（即使没强制）
    assert!(skip_reason(false, false, true).is_none());
}

#[test]
fn normalize_模态映射与_pdf_归到_document() {
    let e = normalize(
        "claude-x",
        &obj(r#"{"id":"Claude-X","modalities":{"input":["text","image","pdf","weird"]},"tool_call":true}"#),
    )
    .unwrap();
    assert_eq!(e.id, "claude-x", "id 统一小写");
    assert_eq!(e.modalities.unwrap(), vec!["text", "image", "document"]);
    assert_eq!(e.tools, Some(true));
    assert_eq!(e.reasoning, None, "数据集没给就是没给（不发明）");
}

#[test]
fn normalize_档位_none_归到_off_且_reasoning_缺省时按有档位推为_true() {
    let e = normalize(
        "m",
        &obj(r#"{"modalities":{"input":["text"]},"reasoning_options":[{"type":"effort","values":["none","low","xhigh","bogus"]}]}"#),
    )
    .unwrap();
    let efforts = e.efforts.unwrap();
    assert_eq!(efforts.get("off").map(String::as_str), Some("none"));
    assert_eq!(efforts.get("low").map(String::as_str), Some("low"));
    assert_eq!(efforts.get("xhigh").map(String::as_str), Some("xhigh"));
    assert!(!efforts.contains_key("bogus"), "不认识的值必须跳过：{efforts:?}");
    assert_eq!(e.reasoning, Some(true), "给了档位 = 支持推理（诚实推断）");
}

#[test]
fn normalize_reasoning_明确_false_的条目不许带档位() {
    let e = normalize(
        "m",
        &obj(r#"{"reasoning":false,"reasoning_options":[{"type":"effort","values":["low"]}]}"#),
    )
    .unwrap();
    assert_eq!(e.reasoning, Some(false));
    assert!(e.efforts.is_none(), "false 与档位互斥");
}

#[test]
fn normalize_一个维度都没有的条目不占地方() {
    assert!(normalize("m", &obj(r#"{"name":"啥也没声明"}"#)).is_none());
    assert!(normalize("", &obj(r#"{"id":"  "}"#)).is_none());
}

#[test]
fn 合并_新的优先旧的补位_且合并后_false_不留档位() {
    let older = Entry {
        id: "m".into(),
        modalities: Some(vec!["text".into(), "image".into()]),
        tools: Some(true),
        reasoning: Some(true),
        efforts: None,
        last_updated: Some("2026-01-01".into()),
    };
    let newer = Entry {
        id: "m".into(),
        modalities: None, // 新条目没给模态 → 旧的要留下
        tools: None,
        reasoning: Some(false),
        efforts: Some(BTreeMap::from([("low".to_string(), "low".to_string())])),
        last_updated: Some("2026-09-01".into()),
    };
    let merged = merge_entries(newer, older);
    assert_eq!(merged.modalities.unwrap(), vec!["text", "image"]);
    assert_eq!(merged.tools, Some(true), "旧声明的工具能力不该被挤掉");
    assert_eq!(merged.reasoning, Some(false));
    assert!(merged.efforts.is_none(), "reasoning=false 时不留档位");
    assert_eq!(merged.last_updated.as_deref(), Some("2026-09-01"));
}

#[test]
fn 解析落盘_两种顶层形状都认_并且不填方言() {
    let dir = crate::testpaths::scratch("capability");
    // 形状一：顶层就是 provider map
    let flat = r#"{"openai":{"models":{"gpt-5":{"id":"gpt-5","modalities":{"input":["text","image"]},"tool_call":true,"reasoning":true,"last_updated":"2026-09-01"}}}}"#;
    assert_eq!(parse_and_write(flat, &dir, "2026-09-27T00:00:00Z").unwrap(), 1);
    let written = std::fs::read_to_string(dir.join(REMOTE_FILE)).unwrap();
    let parsed: OverlayFile = serde_json::from_str(&written).unwrap();
    assert_eq!(parsed.source, SOURCE);
    assert_eq!(parsed.fetched_at.as_deref(), Some("2026-09-27T00:00:00Z"));
    let entry = parsed.entries.get("gpt-5").unwrap();
    assert_eq!(entry.thinking_format, None, "方言永远不填（猜错比不填更糟）");
    assert_eq!(entry.tools, Some(true));

    // 形状二：包一层 `models`
    let nested = r#"{"models":{"anthropic":{"models":{"claude":{"modalities":{"input":["text"]},"tool_call":false}}}}}"#;
    assert_eq!(parse_and_write(nested, &dir, "2026-09-27T01:00:00Z").unwrap(), 1);

    // 解析不出任何模型 → 报错，而且**旧文件还在**（上面那份没被弄坏）
    let before = std::fs::read_to_string(dir.join(REMOTE_FILE)).unwrap();
    let err = parse_and_write(r#"{"hello":"world"}"#, &dir, "x").unwrap_err();
    assert!(err.contains("没有解析出任何模型"), "{err}");
    assert_eq!(std::fs::read_to_string(dir.join(REMOTE_FILE)).unwrap(), before);

    let _ = std::fs::remove_dir_all(&dir);
}
