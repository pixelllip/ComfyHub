//! `skills` 的单元测试（主体代码在 [`skills`](super) —— 与主体分离）。
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
fn 平铺技能通过_read_body_能加载() {
    let dir = crate::infra::testpaths::scratch("skills-flat-load");
    let user = dir.join("user");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("flat.md"),
        "---\nname: flat\ndescription: 平铺\n---\n正文",
    )
    .unwrap();
    let found = read_body(&dir.join("builtin"), &user, "flat").expect("找得到平铺技能");
    assert!(found.valid, "平铺技能必须能通过校验：{:?}", found.valid);
    assert_eq!(found.body, "正文");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 平铺文件的名字可以与_frontmatter_不同() {
    let dir = crate::infra::testpaths::scratch("skills-flat-rename");
    let user = dir.join("user");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("Anima提示词.md"),
        "---\nname: anima-prompt\ndescription: 平铺改名\n---\n正文",
    )
    .unwrap();
    let found = read_body(&dir.join("builtin"), &user, "anima-prompt").expect("按 name 找得到");
    assert!(found.valid);
    assert_eq!(found.entry.name, "anima-prompt");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 目录注入只收_model_invocable_且尊重停用名单() {
    let dir = crate::infra::testpaths::scratch("skills-invocable");
    let user = dir.join("user");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("manual.md"),
        "---\nname: manual\ndescription: 只让用户调\ndisable-model-invocation: true\n---\n正文",
    )
    .unwrap();
    std::fs::write(
        user.join("auto.md"),
        "---\nname: auto\ndescription: 模型可调\n---\n正文",
    )
    .unwrap();

    let names: Vec<String> = catalog(&dir.join("builtin"), &user)
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, vec!["auto"], "disable-model-invocation 的不进目录");

    let disabled: HashSet<String> = HashSet::from(["auto".to_string()]);
    let none = catalog_with(&dir.join("builtin"), &user, &disabled);
    assert!(none.is_empty(), "停用的不进目录");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 用户版不合法时目录回落到内置版() {
    let dir = crate::infra::testpaths::scratch("skills-override-invalid");
    let builtin = dir.join("builtin");
    let user = dir.join("user");
    std::fs::create_dir_all(&builtin).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        builtin.join("same.md"),
        "---\nname: same\ndescription: 内置\n---\n正文",
    )
    .unwrap();
    // 缺 description → 非法
    std::fs::write(user.join("same.md"), "---\nname: same\n---\n用户正文").unwrap();

    let found = catalog(&builtin, &user);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].description, "内置", "用户版非法时应回落到内置版");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bundle_附带文件能列出来() {
    let dir = crate::infra::testpaths::scratch("skills-bundle-files");
    let user = dir.join("user");
    let bundle = user.join("with-files");
    std::fs::create_dir_all(bundle.join("references")).unwrap();
    std::fs::write(
        bundle.join("SKILL.md"),
        "---\nname: with-files\ndescription: 带附件\n---\n正文",
    )
    .unwrap();
    std::fs::write(bundle.join("references").join("a.md"), "参考").unwrap();
    let found = read_body(&dir.join("builtin"), &user, "with-files").expect("读取技能");
    let files = bundle_files(&found);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].0, "references/a.md");
    // 平铺技能没有 bundle，列不出附带文件
    std::fs::write(
        user.join("flat.md"),
        "---\nname: flat\ndescription: 平铺\n---\n正文",
    )
    .unwrap();
    let flat = read_body(&dir.join("builtin"), &user, "flat").expect("读取平铺");
    assert!(bundle_files(&flat).is_empty());
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
