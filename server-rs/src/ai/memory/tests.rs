//! `memory` 的单元测试（主体代码在 [`memory`](super) —— 2026-09-30 按用户要求与主体分离）。
//!
//! `use super::*;` 照旧：子模块能看见父模块的私有项，所以用例一行都不用改。

use super::*;

#[test]
fn 条数只数非空行() {
    assert_eq!(count_entries(""), 0);
    assert_eq!(count_entries("- a\n- b\n"), 2);
    assert_eq!(count_entries("- a\n\n   \n- b"), 2);
    assert_eq!(count_entries("\n\n"), 0);
}

#[test]
fn utf16_截断与_kotlin_take_一致() {
    // 中文按 1 个码元
    assert_eq!(take_utf16("中文abc", 3), "中文a");
    // emoji 占 2 个码元：limit 停在 emoji 中间时**整个丢掉**（Kotlin 会截出半个代理对，
    // 那种字符串根本不能进 JSON —— 这里刻意比 Kotlin 更严，代价是极端输入下少一个字符）
    let text = "a😀b";
    assert_eq!(take_utf16(text, 1), "a");
    assert_eq!(
        take_utf16(text, 3),
        "a😀",
        "limit 正好够放下整个 emoji 时要包含它"
    );
    assert_eq!(take_utf16(text, 4), "a😀b");
}

#[test]
fn 读不到文件就是空而不是报错() {
    // ⚠ 这一支要的就是"**读不到**"：只拼路径、不许创建
    let dir = crate::infra::testpaths::root().join("memory-没有这个目录");
    assert_eq!(read(&dir), "");
    assert_eq!(prompt_text(&dir), "");
}

/// 界面那一套：整篇替换 / 追加一条 / 按下标删 / 清空的往返。
///
/// ⚠ 这条用例盯的是"**读 → 改 → 写是一次幂等的往返**"：Kotlin 落盘时统一加结尾换行、
/// 读回来去掉首尾空白。少这一下，界面里每按一次保存就多攒一个空行。
#[test]
fn 界面写入往返是幂等的() {
    let dir = crate::infra::testpaths::scratch("memory-ui");

    // 空 → 写两条 → 条数与内容
    let dto = write(&dir, "- 第一条\n- 第二条\n").unwrap();
    assert_eq!(dto.content, "- 第一条\n- 第二条");
    assert_eq!(dto.entry_count, 2);
    assert_eq!(dto.max_chars, MAX_CHARS as i64);
    assert_eq!(dto.max_entries, MAX_ENTRIES);
    assert!(dto.updated_at.is_some(), "文件在 → updatedAt 一定有值");
    assert!(dto.path.ends_with("memory.md"));
    // 落盘带结尾换行（人看才像正常文件）
    assert_eq!(
        std::fs::read_to_string(dir.join(FILE_NAME)).unwrap(),
        "- 第一条\n- 第二条\n"
    );

    // 再"读 → 原样写"一次：内容一个字节都不许变（幂等）
    let again = write(&dir, &read_dto(&dir).content).unwrap();
    assert_eq!(again.content, "- 第一条\n- 第二条");
    assert_eq!(
        std::fs::read_to_string(dir.join(FILE_NAME)).unwrap(),
        "- 第一条\n- 第二条\n"
    );

    // 追加：用户手敲的**不带日期前缀**
    let dto = append_user(&dir, "第三条").unwrap();
    assert_eq!(dto.content, "- 第一条\n- 第二条\n- 第三条");
    // 判重（同一句再说一遍不算新增、也不加行）
    let dto = append_user(&dir, "第三条").unwrap();
    assert_eq!(dto.entry_count, 3);

    // 按下标删中间那条；越界下标忽略掉（界面手里的列表可能只差一瞬间）
    let dto = delete_entries(&dir, &[1, 99]).unwrap();
    assert_eq!(dto.content, "- 第一条\n- 第三条");
    // 空下标 = 原样返回
    assert_eq!(
        delete_entries(&dir, &[]).unwrap().content,
        "- 第一条\n- 第三条"
    );

    // 清空
    let dto = clear(&dir).unwrap();
    assert_eq!(dto.content, "");
    assert_eq!(dto.entry_count, 0);
    assert_eq!(std::fs::read_to_string(dir.join(FILE_NAME)).unwrap(), "");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 三种超限各自回**哪个稳定码** —— 这是界面上"红色提示说什么"的全部依据。
///
/// 为什么不用"拿人话 `contains` 猜码"：`记忆太长（N > 8000 字符）` 里也有"太长"，
/// 而它该是 `MEMORY_TOO_LARGE`（单条 501 字符那条才是 `INVALID_ARGUMENT`）——
/// 靠字符串猜必然把这两档混掉，而"该去清理记忆"和"该把这条拆短"是两种处置。
#[test]
fn 三类超限的稳定码各不相同() {
    let dir = crate::infra::testpaths::scratch("memory-codes");

    // 单条太长（501 > 500）→ INVALID_ARGUMENT
    let long_entry = "字".repeat(MAX_ENTRY_CHARS + 1);
    let e = append_user(&dir, &long_entry).unwrap_err();
    assert_eq!(e.code, "INVALID_ARGUMENT");
    assert!(e.message.contains("拆成几条"), "{}", e.message);

    // 空内容 → INVALID_ARGUMENT
    assert_eq!(
        append_user(&dir, "   ").unwrap_err().code,
        "INVALID_ARGUMENT"
    );

    // 整篇太长（> 8000）→ MEMORY_TOO_LARGE（**不是** INVALID_ARGUMENT）
    let e = write(&dir, &"字".repeat(MAX_CHARS + 1)).unwrap_err();
    assert_eq!(e.code, "MEMORY_TOO_LARGE");
    assert!(
        e.message.starts_with("记忆太长（8001 > 8000 字符）"),
        "{}",
        e.message
    );

    // 条数满了 → MEMORY_FULL（不静默丢、也不挤掉最旧的一条）
    let full: String = (0..MAX_ENTRIES)
        .map(|i| format!("- 第{i}条"))
        .collect::<Vec<_>>()
        .join("\n");
    write(&dir, &full).unwrap();
    let e = append_user(&dir, "再来一条").unwrap_err();
    assert_eq!(e.code, "MEMORY_FULL");
    assert!(e.message.contains("上限 100 条"), "{}", e.message);

    // AI 那条路：拿不到本机日期就不写（宁可报错，也不要写一条 1970 年的记忆）
    let e = append(&dir, "x", "  ").unwrap_err();
    assert_eq!(e.code, "MEMORY_WRITE_FAILED");

    let _ = std::fs::remove_dir_all(&dir);
}
