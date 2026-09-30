//! `capability_catalog` 的单元测试（主体代码在 [`capability_catalog`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

/// ⚠ 每个用例**自己一个目录**：共用 `comfyhub-cap-<pid>` 会让并行用例互相覆盖
/// `custom.json`（这个坑在本仓库踩过不止一次，见 docs/pitfalls.md 那条"偶发红"）。
fn dir(tag: &str) -> PathBuf {
    crate::infra::testpaths::scratch(&format!("cap-{tag}"))
}

/// ★ 内置规则的**条数与 Kotlin 一致**（118 条）—— 这是"生成器漏规则"的绊线。
///
/// 真机验收靠"两端 `/api/ai/capabilities/status` 的 `builtinRules` 相等"，
/// 但那条要起两个后端；这条能在 `cargo test` 里就把它钉住。
#[test]
fn 内置规则条数与生成源一致() {
    assert_eq!(rules().len(), 118, "规则表变了？重跑 tools/gen_capability_rules.py");
    // 前几条的顺序**就是语义**（宽前缀在后），抽查开头与结尾
    assert_eq!(rules()[0].0, "claude-sonnet-5");
    assert_eq!(rules().last().unwrap().0, "grok");
    assert!(rules().iter().any(|(p, _)| *p == "deepseek-v4.1"));
    // 编码是对的：`claude` 那条不能抢在 `claude-sonnet-5` 前面
    let idx = |name: &str| rules().iter().position(|(p, _)| *p == name).unwrap();
    assert!(idx("claude-sonnet-5") < idx("claude"), "宽前缀必须排在后面");
    assert!(idx("gpt-5.6") < idx("gpt-5"));
    assert!(idx("deepseek-v4.1") < idx("deepseek-v4"));
}

/// 包含匹配 + 先命中先用：图片 / 工具 / 推理 / 档位 / 方言都照表来。
#[test]
fn 内置匹配按包含且先命中先用() {
    let overlays = Overlays::default();
    let hit = lookup(&overlays, "anthropic/claude-sonnet-5").unwrap();
    assert_eq!(hit.modalities, vec!["text", "image"]);
    assert!(hit.tools && hit.reasoning);
    assert_eq!(hit.thinking_format, None, "claude 系没有特殊方言");
    assert_eq!(hit.matched_by, "内置规则「claude-sonnet-5」");
    assert!(hit.thinking_efforts.iter().any(|(k, v)| *k == "max" && *v == "max"));

    // deepseek-v4.1：带图片 + 思考档位 + deepseek 方言（这一条 reasoning=true，方言留着）
    let deepseek = lookup(&overlays, "deepseek-v4.1").unwrap();
    assert_eq!(deepseek.modalities, vec!["text", "image"]);
    assert!(deepseek.reasoning);
    assert_eq!(deepseek.thinking_format.as_deref(), Some("deepseek"));
    assert!(deepseek.thinking_efforts.iter().any(|(k, _)| k == "max"));

    // deepseek-chat：只吃文本、**不声明推理** → 方言也一并清空（Kotlin 同：
    // `val format = if (!reasoning) null else …`）。
    // 这条看着反直觉（"方言又不是推理能力"），但留着它就会与 `validate_model` 打架。
    let chat = lookup(&overlays, "deepseek-chat").unwrap();
    assert_eq!(chat.modalities, vec!["text"]);
    assert!(!chat.reasoning, "deepseek-chat 不声明推理");
    assert_eq!(chat.thinking_format, None, "不支持推理 → 方言与档位一律清空");
    assert!(chat.thinking_efforts.is_empty());

    // 目录里没有的模型 → `None`（调用方按 unknown() 兜底）
    assert!(lookup(&overlays, "some-unknown-model-3000").is_none());
    let fallback = unknown();
    assert_eq!(fallback.modalities, vec!["text"]);
    assert!(fallback.tools, "工具默认给上（很多网关不声明但实际支持）");
    assert!(!fallback.reasoning);
}

/// 覆盖层：**精确匹配 → 去前缀（两个方向）→ 有歧义就不命中**。
#[test]
fn 覆盖层去前缀只在唯一命中时才认() {
    let d = dir("overlay");
    std::fs::write(
        d.join(CUSTOM_FILE),
        r#"{"source":"custom","entries":{
             "z-ai/glm-5": {"tools": false},
             "a/x": {"tools": false},
             "b/x": {"tools": false},
             "plain": {"reasoning": true}
           }}"#,
    )
    .unwrap();
    let overlays = load_overlays(&d);

    // 精确命中（大小写不敏感）
    let hit = lookup(&overlays, "Z-AI/GLM-5").unwrap();
    assert!(!hit.tools);
    assert!(hit.matched_by.contains("手工目录 custom.json"), "{}", hit.matched_by);

    // 网关 id 不带前缀 → 数据集条目的后缀唯一命中
    assert!(!lookup(&overlays, "glm-5").unwrap().tools);

    // 有歧义（a/x 与 b/x 都以 /x 结尾）→ **三层都不命中**，落回调用方的保守默认
    // （猜错的代价是给模型勾错模态，而那是会发出错误请求的）
    assert!(
        lookup(&overlays, "x").is_none(),
        "两个候选时宁可不命中（`lookup` 回 None，调用方按 unknown() 兜底）"
    );

    // 只声明了一个维度：其余维度落回内置
    let plain = lookup(&overlays, "plain").unwrap();
    assert!(plain.reasoning);
    assert_eq!(plain.modalities, vec!["text"], "覆盖层没说模态 → 落回内置的 TEXT_ONLY");
    let _ = std::fs::remove_dir_all(&d);
}

/// ★ 最终 `reasoning=false` 时，档位与方言**一律清空**（不管哪层给过）。
///
/// 留着就会与 `validate_model` 打架：那一条会拒掉"声明了档位却没勾推理"，
/// 而用户看到的是"导入进来的模型存不进去"。
#[test]
fn 不支持推理时档位与方言一律清空() {
    let d = dir("reasoning");
    // 内置 `kimi-k2.5` 是 TEXT_IMAGE、不支持推理；覆盖层硬塞档位与方言
    std::fs::write(
        d.join(CUSTOM_FILE),
        r#"{"entries":{"kimi-k2.5":{"thinkingEfforts":{"high":"high"},"thinkingFormat":"zai"}}}"#,
    )
    .unwrap();
    let overlays = load_overlays(&d);
    let hit = lookup(&overlays, "kimi-k2.5").unwrap();
    assert!(!hit.reasoning);
    assert!(hit.thinking_efforts.is_empty(), "不支持推理就一个档位都不许留");
    assert_eq!(hit.thinking_format, None, "不支持推理就不给方言");
    // 而同一层的条目如果**明说**支持推理，档位就保留
    std::fs::write(
        d.join(CUSTOM_FILE),
        r#"{"entries":{"kimi-k2.5":{"reasoning":true,"thinkingEfforts":{"high":"high"},"thinkingFormat":"zai"}}}"#,
    )
    .unwrap();
    let hit = lookup(&load_overlays(&d), "kimi-k2.5").unwrap();
    assert!(hit.reasoning);
    assert_eq!(hit.thinking_format.as_deref(), Some("zai"));
    assert_eq!(hit.thinking_efforts, vec![("high".to_string(), "high".to_string())]);
    let _ = std::fs::remove_dir_all(&d);
}

/// 状态：三层各自的概况 + `builtinRules`；坏文件当"该层不存在"（**不抛**）。
#[test]
fn 状态读三层而坏文件不炸() {
    let d = dir("status");
    // 先什么都没有
    let empty = status(&d);
    assert_eq!(empty.version, VERSION);
    assert_eq!(empty.builtin_rules, 118);
    assert!(empty.custom.is_none() && empty.remote.is_none());
    assert!(is_stale(&d, 1_000_000), "没拉过就算过期");

    // 两层都在
    std::fs::write(d.join(CUSTOM_FILE), r#"{"source":"custom","entries":{"a":{},"b":{}}}"#).unwrap();
    std::fs::write(
        d.join(REMOTE_FILE),
        r#"{"source":"models.dev","fetchedAt":"2026-09-25T00:00:00Z","entries":{"a":{}}}"#,
    )
    .unwrap();
    let s = status(&d);
    assert_eq!(s.custom.as_ref().unwrap().entry_count, 2);
    assert_eq!(s.custom.as_ref().unwrap().source, "custom");
    assert_eq!(s.remote.as_ref().unwrap().source, "models.dev");
    assert_eq!(s.remote.as_ref().unwrap().fetched_at.as_deref(), Some("2026-09-25T00:00:00Z"));
    // 拉取时间 → 年龄 / 过期（7 天 = 168 小时）
    // ⚠ 用**解析出来的**那个时刻做基准，别自己手算 epoch（手算错一位就是一条假红的用例）
    let (fetched, _) = crate::infra::iso_time::parse("2026-09-25T00:00:00Z").unwrap();
    assert_eq!(remote_age_hours(&d, fetched + 3600 * 5).unwrap(), 5);
    assert!(!is_stale(&d, fetched + 3600 * 100), "100 小时不算过期");
    assert!(is_stale(&d, fetched + 3600 * 200), "200 小时 > 7 天算过期");
    // 默认 source（文件里没写）
    std::fs::write(d.join(CUSTOM_FILE), r#"{"entries":{}}"#).unwrap();
    assert_eq!(status(&d).custom.unwrap().source, "custom");

    // 坏 JSON → 该层当不存在，**不许抛**
    std::fs::write(d.join(CUSTOM_FILE), "{ 这不是 JSON").unwrap();
    let broken = status(&d);
    assert!(broken.custom.is_none(), "坏文件当没有这一层");
    assert!(broken.remote.is_some(), "另一层不受影响");
    let _ = std::fs::remove_dir_all(&d);
}
