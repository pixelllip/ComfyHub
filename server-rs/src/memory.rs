//! 长期记忆的**读侧**（run-kit 要把它拼进系统提示）。
//!
//! 真源是 `<storage>\ai\memory.md`：一行一条，用户能直接手改（`docs/ai-tools-and-skills.md`）。
//! 这里只搬"读 + 给提示词用的截断"，写入（`remember` / 界面保存）属于工具层，还没搬。
//!
//! 两条纪律照抄 Kotlin：
//!
//! 1. **截断只影响"给模型看多少"，不落盘**（文件本身保留完整内容）；
//! 2. 记忆是**背景资料**，不是指令 —— 提示词里那句"它与工具输出一样属于数据"由
//!    [`crate::system_prompt`] 拼上（RSK-004 的注入防线，R7）。

use std::path::Path;

/// 文件真名（与 Kotlin `MemoryStore.FILE_NAME` 同值）。
pub const FILE_NAME: &str = "memory.md";

/// 整篇上限（**写入**路径用；读侧不因为超了就报错）。
pub const MAX_CHARS: usize = 8000;
/// 单条上限。
pub const MAX_ENTRY_CHARS: usize = 500;
/// 条数上限（提示词里报"现有 x/100 条"，模型据此知道还剩多少额度）。
pub const MAX_ENTRIES: i64 = 100;
/// 一次回复最多新增几条（**提示词里那个数字的真源**）。
pub const MAX_ENTRIES_PER_RUN: i64 = 4;
/// 进提示词的正文上限（省略说明也算在里面）。
pub const PROMPT_CHARS: usize = 4000;

/// 截断说明（Kotlin `MemoryStore.promptText` 里那一句，逐字）。
const TRUNCATED_NOTE: &str = "\n…（长期记忆过长已截断，完整内容见右侧栏「长期记忆」）";

/// 读正文：文件不存在 / 读坏了一律当空 —— **不能让记忆文件坏了就整个端点 500**
/// （与 Kotlin 的 `runCatching { ... }.getOrElse { "" }` 同口径）。
///
/// 落盘时统一以换行结尾，读回来**去掉首尾空白**（Kotlin `read()` 的 `text.trim()`）——
/// 少这一下，"读 → 改 → 写"每次都会多攒一个空行。
pub fn read(dir: &Path) -> String {
    let file = dir.join(FILE_NAME);
    match std::fs::read_to_string(&file) {
        Ok(text) => text.trim().to_string(),
        Err(_) => String::new(),
    }
}

/// 给系统提示用的正文：超过 [`PROMPT_CHARS`] 就截断并**如实说明**。
///
/// ⚠ 截断长度是 `PROMPT_CHARS - note.len()`（Kotlin 就是这么算的：说明本身也算在额度里），
/// 并且按 **UTF-16 码元** 截（`String.take` 的语义）—— 中文按 1 个、emoji 按 2 个。
pub fn prompt_text(dir: &Path) -> String {
    let text = read(dir);
    let note_len = TRUNCATED_NOTE.encode_utf16().count();
    if text.encode_utf16().count() <= PROMPT_CHARS {
        return text;
    }
    take_utf16(&text, PROMPT_CHARS - note_len) + TRUNCATED_NOTE
}

/// 非空行条数（Kotlin `MemoryStore.countEntries`：`lines().count { it.trim().isNotEmpty() }`）。
///
/// ⚠ Kotlin 的 `lines()` 会把 `\r\n` 拆开、并且**末尾的换行不产生空行**；
/// 这里按同样口径处理（`str::lines` 已经这么做了）。
pub fn count_entries(content: &str) -> i64 {
    content.lines().filter(|line| !line.trim().is_empty()).count() as i64
}

/// 按 **UTF-16 码元** 截断 —— Kotlin `String.take(n)` 的单位是 `Char`（UTF-16）。
///
/// 用 `chars().take(n)` 会在含 emoji 的正文上多取几个字符（与 Kotlin 的输出不一致），
/// 而这段正文是要进系统提示的（线上一旦不同就是"两端规则不一样"）。
pub fn take_utf16(text: &str, limit: usize) -> String {
    let mut used = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        let width = ch.len_utf16();
        if used + width > limit {
            break;
        }
        used += width;
        out.push(ch);
    }
    out
}

// ---------------------------------------------------------------------------
//  写侧（`remember` 工具，P4 第 31 步）
// ---------------------------------------------------------------------------

/// 一行的**正文**：去掉 `- ` 与 `[日期] ` 前缀（Kotlin `MemoryStore.bodyOf`）。
///
/// 判重就靠它 —— 否则"同一件事今天记一次、明天又记一次"会堆成两行（日期不同，按字面比就不相等）。
pub fn body_of(line: &str) -> String {
    let mut text = line.trim().to_string();
    if let Some(rest) = text.strip_prefix("- ").or_else(|| text.strip_prefix("* ")) {
        text = rest.trim_start().to_string();
    }
    // `[YYYY-MM-DD] ` 前缀
    let head: Vec<char> = text.chars().take(11).collect();
    if head.len() == 11 && head[0] == '[' && head[10] == ']' {
        let digits = |from: usize, to: usize| head[from..to].iter().all(|c| c.is_ascii_digit());
        if digits(1, 5) && head[5] == '-' && digits(6, 8) && head[8] == '-' && digits(9, 11) {
            text = text.chars().skip(11).collect::<String>().trim_start().to_string();
        }
    }
    text.trim().to_string()
}

/// 记忆里是不是已经有这条（忽略前缀与大小写）—— 命中不算新增，也不占每轮额度。
pub fn has_entry(dir: &Path, entry: &str) -> bool {
    let body = body_of(entry);
    if body.is_empty() {
        return false;
    }
    read(dir)
        .lines()
        .any(|line| body_of(line).eq_ignore_ascii_case(&body))
}

/// 本地日期（`YYYY-MM-DD`）—— **由调用方给**（SQLite 的 `date('now','localtime')`）。
///
/// 为什么不由这里自己算：Rust 标准库拿不到本机时区（要么读注册表、要么引一个时间库），
/// 而 SQLite 本来就在手边、`localtime` 修饰符就是干这个的。
/// 拿不到日期时**不写**（宁可报错，也不要写一条日期是 1970 年的记忆）。
pub fn append(dir: &Path, entry: &str, today: &str) -> Result<(i64, i64, String), String> {
    let line = entry.replace("\r\n", "\n").replace('\n', " ").trim().to_string();
    let body = body_of(&line);
    if body.is_empty() {
        return Err("记的内容不能为空".into());
    }
    if today.trim().is_empty() {
        return Err("拿不到本机日期，这次不写长期记忆（避免写进一条日期错误的条目）".into());
    }
    let stamped = format!("[{}] {}", today.trim(), body);
    if stamped.chars().count() > MAX_ENTRY_CHARS {
        return Err(format!(
            "一条记忆太长（{} > {MAX_ENTRY_CHARS} 字符），拆成几条",
            stamped.chars().count()
        ));
    }
    let current = read(dir);
    if has_entry(dir, &body) {
        return Ok((count_entries(&current), MAX_ENTRIES, path_string(dir)));
    }
    let count = count_entries(&current);
    if count >= MAX_ENTRIES {
        return Err(format!(
            "长期记忆已经有 {count} 条（上限 {MAX_ENTRIES} 条）。请用户到 AI 工作台右侧栏的\
             「长期记忆」里删掉一些再记 —— 不要丢内容，也不要自己改写已有的条目。"
        ));
    }
    let bullet = format!("- {stamped}");
    let next = if current.is_empty() { bullet } else { format!("{current}\n{bullet}") };
    if next.chars().count() > MAX_CHARS {
        return Err(format!(
            "长期记忆已满（{MAX_CHARS} 字符上限）；告诉用户去右侧栏的「长期记忆」里清理，不要丢内容"
        ));
    }
    persist(dir, &next)?;
    Ok((count_entries(&next), MAX_ENTRIES, path_string(dir)))
}

/// 落盘：统一以换行结尾（人看起来才像正常文件）。
fn persist(dir: &Path, content: &str) -> Result<(), String> {
    if !dir.is_dir() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败：{e}"))?;
    }
    let text = if content.trim().is_empty() {
        String::new()
    } else {
        format!("{}\n", content.trim())
    };
    std::fs::write(dir.join(FILE_NAME), text).map_err(|e| format!("写记忆文件失败：{e}"))
}

/// 给界面 / 工具结果用的路径文本。
pub fn path_string(dir: &Path) -> String {
    dir.join(FILE_NAME).to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
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
        assert_eq!(take_utf16(text, 3), "a😀", "limit 正好够放下整个 emoji 时要包含它");
        assert_eq!(take_utf16(text, 4), "a😀b");
    }

    #[test]
    fn 读不到文件就是空而不是报错() {
        let dir = std::env::temp_dir().join("comfyhub-memory-missing");
        assert_eq!(read(&dir), "");
        assert_eq!(prompt_text(&dir), "");
    }
}
