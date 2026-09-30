//! `credentials` 的单元测试（主体代码在 [`credentials`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn sanitize_照_kotlin_的四条判据() {
    // 正常值：**trim 后**存
    assert_eq!(sanitize("  sk-abc  ").unwrap(), "sk-abc");
    // `=` 前的段不是合法标识符（带 `-`）→ 不当成 NAME=value，原样收下
    assert_eq!(sanitize("sk-a=b").unwrap(), "sk-a=b");

    // 空白
    let e = sanitize("   ").unwrap_err();
    assert_eq!(e.error, "CONFIG_ERROR");
    assert_eq!(e.detail.as_deref(), Some("密钥不能为空；留空表示不修改已有密钥"));

    // 带引号（含"单个引号字符"这种边界 —— Kotlin 的 startsWith/endsWith 也拒它）
    for raw in ["\"sk-abc\"", "'sk-abc'", "\"", "'"] {
        let e = sanitize(raw).unwrap_err();
        assert_eq!(e.detail.as_deref(), Some("请只粘贴密钥本身，不要带引号"), "{raw}");
    }
    // 一边一个引号不算"带引号"（Kotlin 只认首尾同款）
    assert_eq!(sanitize("\"sk-abc'").unwrap(), "\"sk-abc'");

    // NAME=value 整行
    let e = sanitize("OPENAI_API_KEY=sk-abc").unwrap_err();
    assert_eq!(e.detail.as_deref(), Some("请只粘贴密钥的值，不要粘贴 NAME=value 这种整行"));
    // `^[A-Za-z_][A-Za-z0-9_]*$` 也认得下划线开头的名字
    assert_eq!(
        sanitize("_A1=x").unwrap_err().detail.as_deref(),
        Some("请只粘贴密钥的值，不要粘贴 NAME=value 这种整行")
    );
    // 但数字开头的**不是**合法标识符 → 当普通值放行（Kotlin 同）
    assert_eq!(sanitize("9A=x").unwrap(), "9A=x");

    // 不可打印 / 含空格 / 非 ASCII
    for raw in ["sk abc", "sk\tabc", "sk abc\ndef", "密钥"] {
        let e = sanitize(raw).unwrap_err();
        // 含换行的值会被 trim 成 "sk abc" 之类，但仍应被拒（空格）
        assert_eq!(
            e.detail.as_deref(),
            Some("密钥只能包含可打印字符，且不能有空格或换行"),
            "{raw:?}"
        );
    }
}

#[test]
fn 读坏文件当空而不是报错() {
    // ⚠ 这一支要的就是"**读不到**"：只拼路径、**不许创建**（用 root() 而不是 scratch()）
    let dir = crate::infra::testpaths::root().join("cred-没有这个目录");
    assert!(read_store(&dir.join("nope.json")).is_empty());

    let bad = crate::infra::testpaths::scratch("cred").join("bad.json");
    std::fs::write(&bad, "{ 这不是 JSON").unwrap();
    assert!(read_store(&bad).is_empty());
    // 值的类型不是字符串时按 Kotlin 的 JsonPrimitive.content 取字面量；对象/数组跳过
    std::fs::write(&bad, r#"{"a":"x","n":5,"b":true,"o":{},"arr":[]}"#).unwrap();
    let store = read_store(&bad);
    assert_eq!(store.get("a").map(String::as_str), Some("x"));
    assert_eq!(store.get("n").map(String::as_str), Some("5"));
    assert_eq!(store.get("b").map(String::as_str), Some("true"));
    assert!(!store.contains_key("o"));
    assert!(!store.contains_key("arr"));
    let _ = std::fs::remove_file(&bad);
}
