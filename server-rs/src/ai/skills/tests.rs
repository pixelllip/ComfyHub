//! `skills` 的单元测试（主体代码在 [`skills`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn frontmatter_认块标量与引号() {
    let raw =
        "---\nname: demo\ndescription: |\n  第一行\n  第二行\nwhenToUse: '场景 X'\n---\n正文\n";
    let (meta, body) = parse_front_matter(raw);
    assert_eq!(meta.get("name").unwrap(), "demo");
    assert_eq!(meta.get("description").unwrap(), "第一行\n第二行");
    assert_eq!(meta.get("whentouse").unwrap(), "场景 X");
    // Kotlin 的 `lines()` 末尾换行不产生空行 → body 就是 "正文"（不带结尾换行）
    assert_eq!(body, "正文");

    // 折行块标量 `>` → 空格连接
    let folded = "---\ndescription: >-\n  甲\n  乙\n---\nbody";
    let (meta, _) = parse_front_matter(folded);
    assert_eq!(meta.get("description").unwrap(), "甲 乙");

    // 没有 frontmatter：meta 空、body 是原文（不许把正文吃掉）
    let (meta, body) = parse_front_matter("# 标题\n正文");
    assert!(meta.is_empty());
    assert_eq!(body, "# 标题\n正文");

    // 未闭合的 frontmatter：**整个当正文**（宁可多一段，也别把内容丢了）
    let (meta, body) = parse_front_matter("---\nname: x\n正文");
    assert!(meta.is_empty());
    assert!(body.starts_with("---"));
}

#[test]
fn 名称规则与_kotlin_一致() {
    assert!(valid_name("a"));
    assert!(valid_name("a-b-c"));
    assert!(valid_name("a1"));
    assert!(!valid_name("A"));
    assert!(!valid_name("-a"));
    assert!(!valid_name("a_b"));
    assert!(!valid_name(""));
    assert!(!valid_name(&"a".repeat(65)));
    assert!(valid_name(&"a".repeat(64)));
}

#[test]
fn 空白压平只吃_ascii_空白() {
    assert_eq!(collapse_whitespace("a\n\n b\tc"), "a b c");
    assert_eq!(collapse_whitespace("  a  ").trim(), "a");
    // 全角空格**不是** Java 的 \s：不能被吃掉（否则两端提示词不一样）
    assert_eq!(collapse_whitespace("a\u{3000}b"), "a\u{3000}b");
}

#[test]
fn 扫描认_bundle_与平铺两种形态并且非法项要标记出来() {
    let dir = crate::infra::testpaths::scratch("skills");
    let user = dir.join("user");
    std::fs::create_dir_all(user.join("good-skill")).unwrap();
    std::fs::write(
        user.join("good-skill").join("SKILL.md"),
        "---\nname: good-skill\ndescription: 好用\n---\n正文",
    )
    .unwrap();
    std::fs::write(
        user.join("flat.md"),
        "---\nname: flat\ndescription: 平铺\n---\n正文",
    )
    .unwrap();
    // 非法项：目录名与 frontmatter 的 name 不一致（不许静默忽略，也不许进 catalog）
    std::fs::create_dir_all(user.join("bad-name")).unwrap();
    std::fs::write(
        user.join("bad-name").join("SKILL.md"),
        "---\nname: other-name\ndescription: x\n---\n正文",
    )
    .unwrap();
    // 缺 description
    std::fs::write(user.join("no-desc.md"), "---\nname: no-desc\n---\n正文").unwrap();

    let entries = catalog(&dir.join("builtin"), &user);
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["flat", "good-skill"],
        "非法项不进 catalog，合法的按 name 升序"
    );

    let all = scan(&dir.join("builtin"), &user);
    assert_eq!(all.len(), 4, "scan() 照样列出非法项（界面要显示诊断）");
    assert_eq!(all.iter().filter(|e| !e.valid).count(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 同名时用户版胜出() {
    let dir = crate::infra::testpaths::scratch("skills-override");
    let builtin = dir.join("builtin");
    let user = dir.join("user");
    std::fs::create_dir_all(&builtin).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        builtin.join("same.md"),
        "---\nname: same\ndescription: 内置\n---\n正文",
    )
    .unwrap();
    std::fs::write(
        user.join("same.md"),
        "---\nname: same\ndescription: 用户\n---\n正文",
    )
    .unwrap();
    let found = catalog(&builtin, &user);
    assert_eq!(found.len(), 1, "同名只留一条");
    assert_eq!(found[0].description, "用户");
    let _ = std::fs::remove_dir_all(&dir);
}
