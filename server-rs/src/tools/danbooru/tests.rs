//! `danbooru` 的单元测试（主体代码在 [`danbooru`](super) —— 与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn 归一化的四种写法() {
    assert_eq!(normalize_tag("shinku"), "shinku");
    assert_eq!(normalize_tag("  Shinku (NTE)  "), "shinku_(nte)");
    // 全角括号 + 已有下划线：正好一个 `_(`
    assert_eq!(normalize_tag("真红（NTE）"), "真红_(nte)");
    assert_eq!(normalize_tag("shinku_（nte）"), "shinku_(nte)");
    // 多个空白连成一串 → 一个下划线
    assert_eq!(normalize_tag("shinku   nte"), "shinku_nte");
    // 两头多余的下划线去掉，但**中间的不动**
    assert_eq!(normalize_tag("_shinku_(nte)_"), "shinku_(nte)");
}

#[test]
fn 分类码_2_是未定义() {
    assert_eq!(category_name(Some(4)), "character");
    assert_eq!(category_name(Some(1)), "artist");
    assert_eq!(
        category_name(Some(2)),
        "unknown",
        "2 在 danbooru 上没有定义，不许说成 copyright"
    );
    assert_eq!(category_name(None), "unknown");
}

#[test]
fn 编码保留通配符与下划线() {
    assert_eq!(encode("shinku_(nte)"), "shinku_%28nte%29");
    assert_eq!(
        encode("*shinku*"),
        "*shinku*",
        "通配符被编码成 %2A 就不是模糊搜索了"
    );
    assert_eq!(encode("真红"), "%E7%9C%9F%E7%BA%A2");
    assert_eq!(encode("a b"), "a%20b");
}

#[test]
fn 客户端不许收空地址() {
    assert!(DanbooruClient::new("   ", USER_AGENT, 5).is_err());
    let client = DanbooruClient::new("https://example.com/", USER_AGENT, 999).unwrap();
    assert_eq!(client.base_url(), "https://example.com", "结尾斜杠要去掉");
    assert_eq!(
        client.timeout.as_secs(),
        MAX_TIMEOUT_SECONDS,
        "超时要夹到上限"
    );
}

#[test]
fn 挑战页判据认三种标记() {
    assert!(looks_like_cloudflare_challenge(
        "<html><title>Just a moment...</title>"
    ));
    assert!(looks_like_cloudflare_challenge(
        "<script src='/cdn-cgi/challenge-platform/x'></script>"
    ));
    assert!(looks_like_cloudflare_challenge(
        "Enable JavaScript and cookies to continue"
    ));
    assert!(!looks_like_cloudflare_challenge(
        r#"[{"name":"shinku_(nte)"}]"#
    ));
}

#[test]
fn wiki_截断是逐字符的() {
    let short = "很短".to_string();
    assert_eq!(body_text(&short), "很短");
    let long = "字".repeat(MAX_WIKI_CHARS + 50);
    let cut = body_text(&long);
    assert!(cut.ends_with("（wiki 过长已截断）"));
    assert!(cut.chars().count() < long.chars().count());
}

#[test]
fn 选角色是并列取第一个() {
    // 与 Kotlin `maxByOrNull` 同义：postCount 并列时**取先来的那个**
    let tags = [
        Tag {
            name: "a_(x)".into(),
            category: Some(4),
            post_count: 100,
            deprecated: false,
        },
        Tag {
            name: "b_(x)".into(),
            category: Some(4),
            post_count: 100,
            deprecated: false,
        },
    ];
    let picked = tags
        .iter()
        .filter(|t| t.category == Some(4))
        .fold(None::<&Tag>, |best, tag| match best {
            Some(current) if current.post_count >= tag.post_count => Some(current),
            _ => Some(tag),
        })
        .unwrap();
    assert_eq!(picked.name, "a_(x)");
}
