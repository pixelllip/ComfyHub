//! `memory` 的单元测试（主体代码在 [`memory`](super) —— 与主体分离）。
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
    let memory = prompt(&dir);
    assert_eq!(memory.text, "");
    assert!(memory.is_empty());
    assert_eq!(memory.total_entries, 0);
    assert_eq!(memory.omitted_entries, 0);
}

/// 两条上限各管一件事，而且**都能被钉住**：
/// * 写入上限 = 条数上限 × 单条上限（含换行）= 100 条满长条目**真的写得下**；
/// * 注入上限 8000 —— 提示词只放最新一截，其余靠 `recall_memory` 查。
///
/// 只改其中一个常量、或者把"留最新"写成"留最新一条"，这条都会红
/// （2026-11-14 用户报"8000 字符很快就撞到 100 条上限"的回归）。
#[test]
fn 写入上限装得下_100_条满长条目() {
    assert_eq!(MAX_CHARS, MAX_ENTRIES as usize * MAX_ENTRY_CHARS + 99);
    // 注入上限必须**小于**写入上限（下面那条"write 收下、prompt 只留一截"就是它的判据）

    let dir = crate::infra::testpaths::scratch("memory-two-tier");
    // 每条**正好 500 字**（`- 第NNN条` 占 7 个，补 493 个"字"），共 100 条：
    // 连换行算进去正好等于写入上限 —— 差一个字符就写不进去，这条判据很硬。
    let entries: Vec<String> = (0..MAX_ENTRIES as usize)
        .map(|i| format!("- 第{i:03}条{}", "字".repeat(MAX_ENTRY_CHARS - 7)))
        .collect();
    let content = entries.join("\n");
    assert_eq!(content.encode_utf16().count(), MAX_CHARS);
    let dto = write(&dir, &content).unwrap();
    assert_eq!(dto.entry_count, MAX_ENTRIES);

    let memory = prompt(&dir);
    assert_eq!(memory.total_entries, MAX_ENTRIES);
    assert!(memory.text.encode_utf16().count() <= PROMPT_CHARS);
    assert!(memory.text.contains("第099条"), "最新的那条必须在");
    assert!(!memory.text.contains("第000条"), "最旧的让出额度");
    // **留得最多**：不是只留最新一条（那是把"留最新"写反了）。
    let kept = memory
        .text
        .lines()
        .filter(|line| line.starts_with("- "))
        .count() as i64;
    assert_eq!(kept, MAX_ENTRIES - memory.omitted_entries);
    assert!(kept >= 10, "8000 字符至少该装下 10 条满长条目，实际 {kept}");
    assert!(
        memory
            .text
            .contains(&format!("更早的 {} 条", memory.omitted_entries)),
        "省略了多少条必须写在正文里"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 省略发生时：正文留最新、说明写清"更早的 N 条没放进来"、标题条数仍然是真的。
#[test]
fn 省略最早若干条时如实报条数() {
    let dir = crate::infra::testpaths::scratch("memory-prompt-omitted");
    let entries: Vec<String> = (0..40)
        .map(|i| format!("- [2026-11-1{}] 第{i:02}条{}", i % 10, "字".repeat(280)))
        .collect();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(FILE_NAME), entries.join("\n") + "\n").unwrap();

    let memory = prompt(&dir);
    assert!(memory.text.encode_utf16().count() <= PROMPT_CHARS);
    assert!(memory.omitted_entries > 0, "这段夹具必须真的触发省略");
    assert_eq!(memory.total_entries, 40);
    // 每条约 300 字符 → 8000 装得下二十多条；按"只留最新一条"写就会掉到 1。
    let kept = memory
        .text
        .lines()
        .filter(|line| line.starts_with("- "))
        .count() as i64;
    assert!(kept >= 20, "留得最多才对，实际只留了 {kept} 条");
    assert_eq!(kept, 40 - memory.omitted_entries);
    assert!(
        memory.text.contains("第39条"),
        "最新一条必须在（追加在末尾，截断只能动前面的）"
    );
    assert!(
        !memory.text.contains("第00条"),
        "最旧的条目应该被省略掉，而不是占着额度"
    );
    assert!(
        memory
            .text
            .contains(&format!("更早的 {} 条", memory.omitted_entries)),
        "省略了多少条必须写在正文里：\n{}",
        memory.text
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 手工改出一条比上限还长的**单条**：不许报错、不许返回空，留结尾并说清楚。
#[test]
fn 单条超长的手改文件也不丢正文() {
    let dir = crate::infra::testpaths::scratch("memory-prompt-tail");
    let long = format!("- 开头{}结尾", "长".repeat(PROMPT_CHARS + 500));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(FILE_NAME), long).unwrap();

    let memory = prompt(&dir);
    assert!(memory.text.encode_utf16().count() <= PROMPT_CHARS);
    assert!(memory.text.contains("结尾"), "留的是结尾（最新内容在末尾）");
    assert!(memory.text.contains("只放了结尾"), "{}", memory.text);
    assert_eq!(memory.total_entries, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 整篇放得下时**一个字节都不许动**（不许加说明、不许改顺序）。
#[test]
fn 放得下时正文原样返回() {
    let dir = crate::infra::testpaths::scratch("memory-prompt-plain");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(FILE_NAME), "- 第一条\n- 第二条\n").unwrap();

    let memory = prompt(&dir);
    assert_eq!(memory.text, "- 第一条\n- 第二条");
    assert_eq!(memory.total_entries, 2);
    assert_eq!(memory.omitted_entries, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 只是空行把文件撑过上限时，**没有省略就不许说省略**（说明里那句"更早的 N 条"会变成假话）。
#[test]
fn 只有空行超限时不编造省略() {
    let dir = crate::infra::testpaths::scratch("memory-prompt-blank");
    std::fs::create_dir_all(&dir).unwrap();
    let mut text = "- 甲\n- 乙".to_string();
    text.push_str(&"\n\n".repeat(PROMPT_CHARS)); // 空行把整篇撑到远超上限
    std::fs::write(dir.join(FILE_NAME), text).unwrap();

    let memory = prompt(&dir);
    assert_eq!(memory.text, "- 甲\n- 乙");
    assert_eq!(memory.omitted_entries, 0);
    assert_eq!(memory.total_entries, 2);
    let _ = std::fs::remove_dir_all(&dir);
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

    // 追加：手动输入的**不带日期前缀**
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
/// 为什么不用"拿人话 `contains` 猜码"：`记忆太长（N > 50099 字符）` 里也有"太长"，
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

    // 整篇太长（> 上限）→ MEMORY_TOO_LARGE（**不是** INVALID_ARGUMENT）
    let e = write(&dir, &"字".repeat(MAX_CHARS + 1)).unwrap_err();
    assert_eq!(e.code, "MEMORY_TOO_LARGE");
    assert!(
        e.message
            .starts_with(&format!("记忆太长（{} > {MAX_CHARS} 字符）", MAX_CHARS + 1)),
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
