//! `strict_intake` 的单元测试（主体代码在 [`strict_intake`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

fn model_of(json: &str) -> AiModelDto {
    serde_json::from_str(json).unwrap()
}

fn fact(name: &str, modality: Option<&str>, mime: &str, size: i64) -> AttachmentFact {
    AttachmentFact {
        name: name.to_string(),
        modality: modality.map(|m| m.to_string()),
        mime_type: mime.to_string(),
        size_bytes: size,
        pixels: None,
    }
}

#[test]
fn 传输方式三家都只有图片的内联() {
    for api in [
        "openai-completions",
        "openai-responses",
        "anthropic-messages",
    ] {
        assert_eq!(
            adapter_transports(api, "image"),
            vec!["inline_base64"],
            "{api}"
        );
        assert!(
            adapter_transports(api, "video").is_empty(),
            "{api} 视频还没实现"
        );
        assert!(adapter_transports(api, "document").is_empty());
    }
    assert!(adapter_transports("unknown-api", "image").is_empty());
}

#[test]
fn 模型没声明传输方式时回落到适配器实现的那种() {
    // 内置目录里那些模型只声明模态、不声明传输 —— 不回落的后果是"图片永远发不出去"
    let model = model_of(
        r#"{"providerId":"p","id":"m","displayName":"内置模型","inputModalities":["image"]}"#,
    );
    let result = evaluate(
        &model,
        &fact("a.png", Some("image"), "image/png", 1024),
        &["inline_base64"],
        0,
    );
    assert!(result.allowed, "{:?}", result.blockers);
}

#[test]
fn 模型声明了传输方式时取与适配器的交集() {
    let model = model_of(
        r#"{"providerId":"p","id":"m","displayName":"只声明 remote_url 的模型","inputModalities":["image"],
            "attachmentTransports":{"image":["remote_url"]}}"#,
    );
    let result = evaluate(
        &model,
        &fact("a.png", Some("image"), "image/png", 1024),
        &["inline_base64"],
        0,
    );
    assert!(!result.allowed);
    assert!(
        result.blockers[0].contains("remote_url"),
        "{:?}",
        result.blockers
    );
    assert!(
        result.blockers[0].contains("尚未实现"),
        "{:?}",
        result.blockers
    );
}

#[test]
fn 六种阻断逐条说人话() {
    // ① 模型不支持这种模态
    let text_only = model_of(
        r#"{"providerId":"p","id":"m","displayName":"纯文本模型","inputModalities":["text"]}"#,
    );
    let result = evaluate(
        &text_only,
        &fact("a.png", Some("image"), "image/png", 10),
        &["inline_base64"],
        0,
    );
    assert!(!result.allowed);
    assert!(
        result
            .blockers
            .iter()
            .any(|b| b.contains("未声明支持图片输入")),
        "{:?}",
        result.blockers
    );

    // ② 超过内联上限
    let model = model_of(
        r#"{"providerId":"p","id":"m","displayName":"看图模型","inputModalities":["image"]}"#,
    );
    let result = evaluate(
        &model,
        &fact("big.png", Some("image"), "image/png", MAX_INLINE_BYTES + 1),
        &["inline_base64"],
        0,
    );
    assert!(!result.allowed);
    assert!(
        result
            .blockers
            .iter()
            .any(|b| b.contains("超过内联发送上限")),
        "{:?}",
        result.blockers
    );

    // ③ 超过模型自己的大小上限
    let limited = model_of(
        r#"{"providerId":"p","id":"m","displayName":"小图模型","inputModalities":["image"],
            "maxAttachmentBytes":2097152}"#,
    );
    let result = evaluate(
        &limited,
        &fact("a.png", Some("image"), "image/png", 3 * 1024 * 1024),
        &["inline_base64"],
        0,
    );
    assert!(
        result
            .blockers
            .iter()
            .any(|b| b.contains("超过大小上限（2 MB）")),
        "{:?}",
        result.blockers
    );

    // ④ 超过数量上限
    let counting = model_of(
        r#"{"providerId":"p","id":"m","displayName":"两张上限","inputModalities":["image"],
            "maxAttachmentCount":2}"#,
    );
    let result = evaluate(
        &counting,
        &fact("c.png", Some("image"), "image/png", 10),
        &["inline_base64"],
        2,
    );
    assert!(
        result
            .blockers
            .iter()
            .any(|b| b.contains("附件数量超过上限（2）")),
        "{:?}",
        result.blockers
    );

    // ⑤ MIME 白名单（支持 `image/*` 通配）
    let allow = model_of(
        r#"{"providerId":"p","id":"m","displayName":"白名单","inputModalities":["image"],
            "mimeAllowlist":["image/png"]}"#,
    );
    let result = evaluate(
        &allow,
        &fact("a.webp", Some("image"), "image/webp", 10),
        &["inline_base64"],
        0,
    );
    assert!(
        result
            .blockers
            .iter()
            .any(|b| b.contains("不接受 image/webp 类型")),
        "{:?}",
        result.blockers
    );
    let wildcard = model_of(
        r#"{"providerId":"p","id":"m","displayName":"通配","inputModalities":["image"],
            "mimeAllowlist":["image/*"]}"#,
    );
    assert!(
        evaluate(
            &wildcard,
            &fact("a.webp", Some("image"), "image/webp", 10),
            &["inline_base64"],
            0
        )
        .allowed
    );

    // ⑥ 类型未知
    let result = evaluate(
        &model,
        &fact("x.bin", None, "application/octet-stream", 10),
        &[],
        0,
    );
    assert!(!result.allowed);
    assert!(
        result.blockers[0].contains("无法识别文件类型"),
        "{:?}",
        result.blockers
    );
}

#[test]
fn 文本附件不算多模态_直接放行() {
    let text_only = model_of(
        r#"{"providerId":"p","id":"m","displayName":"纯文本","inputModalities":["text"]}"#,
    );
    let result = evaluate(
        &text_only,
        &fact("notes.md", Some("text"), "text/markdown", 100),
        &[],
        0,
    );
    assert!(result.allowed, "{:?}", result.blockers);
}

#[test]
fn 严格判定按签名_前端谎报骗不过() {
    // 声称 image/png、内容却是 PDF
    let resolved = resolve(
        "evil.png",
        Some("image"),
        "image/png",
        10,
        None,
        Some(b"%PDF-1.7"),
    );
    assert_eq!(
        resolved.fact.modality.as_deref(),
        Some("document"),
        "按签名判成文档"
    );
    assert!(
        resolved.blockers.is_empty(),
        "认得出来就不阻断（只是模态按事实走）"
    );

    // 随机二进制 → 阻断
    let resolved = resolve(
        "evil.png",
        Some("image"),
        "image/png",
        10,
        None,
        Some(&[0, 1, 2, 255]),
    );
    assert!(resolved.fact.modality.is_none());
    assert_eq!(resolved.blockers.len(), 1);
    assert!(
        resolved.blockers[0].contains("无法识别文件 evil.png"),
        "{:?}",
        resolved.blockers
    );

    // 没有文件头：只能采信声明（不假装确认过）
    let resolved = resolve("a.png", Some("image"), "image/png", 10, None, None);
    assert_eq!(resolved.fact.modality.as_deref(), Some("image"));
    assert!(resolved.blockers.is_empty());
}

#[test]
fn 内联预算_贪心保序且单张与总量都要卡() {
    let plan = inline_budget_plan(&[1_000, MAX_INLINE_BYTES + 1, 2_000]);
    assert_eq!(
        plan,
        vec![true, false, true],
        "超单张的跳过，后面的小图还能进"
    );

    let plan = inline_budget_plan_with(&[10, 10, 10, 10], 25, 100);
    assert_eq!(plan, vec![true, true, false, false], "总量卡住之后就不再加");

    // 0 / 负数不算数（空文件不该占预算）
    assert_eq!(inline_budget_plan(&[0, -5, 1]), vec![false, false, true]);
}
