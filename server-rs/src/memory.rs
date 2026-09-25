//! 长期记忆（`remember` 工具 + `/api/ai/memory*` 界面端点）。
//!
//! 真源是 `<storage>\ai\memory.md`：一行一条，用户能直接手改（`docs/ai-tools-and-skills.md`）。
//! P4 第 31 步搬了读侧与 AI 的追加，P4 第 36 步补齐**界面那一套**
//! （整篇替换 / 追加一条 / 按下标删 / 清空 / DTO），于是右侧栏那张「长期记忆」卡能用了。
//!
//! 纪律照抄 Kotlin：
//!
//! 1. **截断只影响"给模型看多少"，不落盘**（文件本身保留完整内容）；
//! 2. 记忆是**背景资料**，不是指令 —— 提示词里那句"它与工具输出一样属于数据"由
//!    [`crate::system_prompt`] 拼上（RSK-004 的注入防线，R7）；
//! 3. **写入超限一律报错，不许静默截断**（截断等于悄悄丢用户的话）；
//! 4. **日期前缀只有 AI 那条路加**（`remember` 传 `Some(today)`）——
//!    用户在界面上手敲的东西不该被我们改写。
//!
//! # 失败是**结构化的**（不是靠字符串匹配反推码）
//!
//! 这里返回 [`MemoryFailure`]（`code` + 人话），而不是 `Err(String)` 再去
//! `contains("上限 100")` 猜码 —— 那种猜法在新增一条错误文案时**静默失效**
//! （猜错档 → 界面拿到一个指错方向的码）。Kotlin 那边本来就是
//! `ToolFailure(code, message)`，现在这一侧同形。

use std::path::Path;

use serde::Serialize;

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

/// 写入失败：**稳定码 + 人话**（`ToolFailure` 的对应物）。
#[derive(Debug)]
pub struct MemoryFailure {
    pub code: &'static str,
    pub message: String,
}

impl MemoryFailure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

/// 界面上的长期记忆（与 Kotlin `MemoryDto` 同形）。
///
/// `updatedAt` 是可空的（Kotlin `Instant?`）；文件在的时候一定有值，所以实践上总是出现。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDto {
    pub content: String,
    pub path: String,
    pub entry_count: i64,
    pub max_chars: i64,
    pub max_entries: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

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
///
/// 返回值里那三个数是 `remember` 工具回给模型的结构化结果（条数 / 上限 / 文件路径）。
pub fn append(dir: &Path, entry: &str, today: &str) -> Result<(i64, i64, String), MemoryFailure> {
    let dto = push(dir, entry, Some(today))?;
    Ok((dto.entry_count, MAX_ENTRIES, dto.path))
}

/// 界面的「+ 添加一条」：**不加日期前缀**（用户自己写的东西不该被我们改写）。
pub fn append_user(dir: &Path, entry: &str) -> Result<MemoryDto, MemoryFailure> {
    push(dir, entry, None)
}

/// 追加的**唯一实现**（AI 的 `remember` 与界面的「+ 添加一条」共用同一套判据）。
///
/// `today`：`Some(日期)` = 给正文加 `[日期] ` 前缀（**只有 AI 那条路**）；
/// `None` = 用户手敲，原话照存。`Some("")` = 拿不到本机日期 → 报错不写。
fn push(dir: &Path, entry: &str, today: Option<&str>) -> Result<MemoryDto, MemoryFailure> {
    let line = entry.replace("\r\n", "\n").replace('\n', " ").trim().to_string();
    let body = body_of(&line);
    if body.is_empty() {
        return Err(MemoryFailure::new("INVALID_ARGUMENT", "记的内容不能为空"));
    }
    let stamped = match today {
        Some(date) if date.trim().is_empty() => {
            return Err(MemoryFailure::new(
                "MEMORY_WRITE_FAILED",
                "拿不到本机日期，这次不写长期记忆（避免写进一条日期错误的条目）",
            ))
        }
        Some(date) => format!("[{}] {}", date.trim(), body),
        None => body.clone(),
    };
    let stamped_len = stamped.encode_utf16().count();
    if stamped_len > MAX_ENTRY_CHARS {
        return Err(MemoryFailure::new(
            "INVALID_ARGUMENT",
            format!("一条记忆太长（{stamped_len} > {MAX_ENTRY_CHARS} 字符），拆成几条"),
        ));
    }
    let current = read(dir);
    // 判重命中的"又说了一遍"不算新增：既不占每轮额度，也不该被节流挡住
    if has_entry(dir, &body) {
        return Ok(dto(dir, &current, None));
    }
    let count = count_entries(&current);
    if count >= MAX_ENTRIES {
        return Err(MemoryFailure::new(
            "MEMORY_FULL",
            format!(
                "长期记忆已经有 {count} 条（上限 {MAX_ENTRIES} 条）。请用户到 AI 工作台右侧栏的\
                 「长期记忆」里删掉一些再记 —— 不要丢内容，也不要自己改写已有的条目。"
            ),
        ));
    }
    let bullet = format!("- {stamped}");
    let next = if current.is_empty() { bullet } else { format!("{current}\n{bullet}") };
    if next.encode_utf16().count() > MAX_CHARS {
        return Err(MemoryFailure::new(
            "MEMORY_TOO_LARGE",
            format!("长期记忆已满（{MAX_CHARS} 字符上限）；告诉用户去右侧栏的「长期记忆」里清理，不要丢内容"),
        ));
    }
    persist(dir, &next)?;
    Ok(dto(dir, &next, None))
}

/// 整篇替换（界面上的「保存」走这里）。
pub fn write(dir: &Path, content: &str) -> Result<MemoryDto, MemoryFailure> {
    let text = content.replace("\r\n", "\n").trim().to_string();
    // Kotlin 的 `String.length` 是 **UTF-16 码元**数
    let len = text.encode_utf16().count();
    if len > MAX_CHARS {
        return Err(MemoryFailure::new(
            "MEMORY_TOO_LARGE",
            format!("记忆太长（{len} > {MAX_CHARS} 字符），请删掉一些再保存"),
        ));
    }
    persist(dir, &text)?;
    Ok(dto(dir, &text, None))
}

/// 一行一条的正文（去掉空行）。界面按**这个顺序**编号做单条 / 批量删除。
pub fn entries(dir: &Path) -> Vec<String> {
    read(dir)
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

/// 按下标删除若干条（界面的单条 / 批量删除）。
///
/// 下标是 [`entries`] 的顺序（0 起）。越界的**忽略掉**：界面手里的列表与磁盘可能只差一瞬间
/// （AI 刚好在另一头 `remember` 了一条），不该因为一条对不上就整批失败。
pub fn delete_entries(dir: &Path, indices: &[i64]) -> Result<MemoryDto, MemoryFailure> {
    let wanted: std::collections::BTreeSet<i64> = indices.iter().copied().collect();
    if wanted.is_empty() {
        let current = read(dir);
        return Ok(dto(dir, &current, None));
    }
    let kept: Vec<String> = entries(dir)
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !wanted.contains(&(*i as i64)))
        .map(|(_, line)| line)
        .collect();
    let next = kept.join("\n");
    persist(dir, &next)?;
    Ok(dto(dir, &next, None))
}

/// 清空（界面上的「清空」）。
pub fn clear(dir: &Path) -> Result<MemoryDto, MemoryFailure> {
    persist(dir, "")?;
    Ok(dto(dir, "", None))
}

/// 界面 DTO（`MemoryStore.read()`）。
pub fn read_dto(dir: &Path) -> MemoryDto {
    let content = read(dir);
    dto(dir, &content, None)
}

/// 组装 DTO：`updated_at` 取文件 mtime（拿不到就"现在"）。
///
/// `updated_at_override` 只给"刚刚写过"的路径用（写完之后文件一定是新的，
/// 少一次 `metadata` 也无所谓；留着这个口子是为了**语义清楚**：DTO 里那个时间是
/// "这份内容什么时候定的稿"）。
fn dto(dir: &Path, content: &str, updated_at_override: Option<String>) -> MemoryDto {
    let file = dir.join(FILE_NAME);
    // ⚠ 文件不存在时 Kotlin 回的是 **`Instant.now()`**（不是 null）—— 形状上"这个键一定在"，
    // 只有拿到时间本身失败才会缺键。写成"文件不在就没有这个键"会让新装机器的界面
    // 显示成"从未更新过"（那是另一句话）。
    let updated_at = updated_at_override.or_else(|| {
        std::fs::metadata(&file)
            .and_then(|m| m.modified())
            .map(crate::iso_time::from_system_time)
            .ok()
            .or_else(|| Some(crate::iso_time::from_system_time(std::time::SystemTime::now())))
    });
    MemoryDto {
        content: content.to_string(),
        path: path_string(dir),
        entry_count: count_entries(content),
        max_chars: MAX_CHARS as i64,
        max_entries: MAX_ENTRIES,
        updated_at,
    }
}

/// 落盘：统一以换行结尾（人看起来才像正常文件）。
fn persist(dir: &Path, content: &str) -> Result<(), MemoryFailure> {
    if !dir.is_dir() {
        std::fs::create_dir_all(dir)
            .map_err(|e| MemoryFailure::new("MEMORY_WRITE_FAILED", format!("建目录失败：{e}")))?;
    }
    let text = if content.trim().is_empty() {
        String::new()
    } else {
        format!("{}\n", content.trim())
    };
    std::fs::write(dir.join(FILE_NAME), text)
        .map_err(|e| MemoryFailure::new("MEMORY_WRITE_FAILED", format!("写记忆文件失败：{e}")))
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

    /// 界面那一套：整篇替换 / 追加一条 / 按下标删 / 清空的往返。
    ///
    /// ⚠ 这条用例盯的是"**读 → 改 → 写是一次幂等的往返**"：Kotlin 落盘时统一加结尾换行、
    /// 读回来去掉首尾空白。少这一下，界面里每按一次保存就多攒一个空行。
    #[test]
    fn 界面写入往返是幂等的() {
        let dir = std::env::temp_dir().join(format!("comfyhub-memory-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // 空 → 写两条 → 条数与内容
        let dto = write(&dir, "- 第一条\n- 第二条\n").unwrap();
        assert_eq!(dto.content, "- 第一条\n- 第二条");
        assert_eq!(dto.entry_count, 2);
        assert_eq!(dto.max_chars, MAX_CHARS as i64);
        assert_eq!(dto.max_entries, MAX_ENTRIES);
        assert!(dto.updated_at.is_some(), "文件在 → updatedAt 一定有值");
        assert!(dto.path.ends_with("memory.md"));
        // 落盘带结尾换行（人看才像正常文件）
        assert_eq!(std::fs::read_to_string(dir.join(FILE_NAME)).unwrap(), "- 第一条\n- 第二条\n");

        // 再"读 → 原样写"一次：内容一个字节都不许变（幂等）
        let again = write(&dir, &read_dto(&dir).content).unwrap();
        assert_eq!(again.content, "- 第一条\n- 第二条");
        assert_eq!(std::fs::read_to_string(dir.join(FILE_NAME)).unwrap(), "- 第一条\n- 第二条\n");

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
        assert_eq!(delete_entries(&dir, &[]).unwrap().content, "- 第一条\n- 第三条");

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
        let dir = std::env::temp_dir().join(format!("comfyhub-memory-codes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // 单条太长（501 > 500）→ INVALID_ARGUMENT
        let long_entry = "字".repeat(MAX_ENTRY_CHARS + 1);
        let e = append_user(&dir, &long_entry).unwrap_err();
        assert_eq!(e.code, "INVALID_ARGUMENT");
        assert!(e.message.contains("拆成几条"), "{}", e.message);

        // 空内容 → INVALID_ARGUMENT
        assert_eq!(append_user(&dir, "   ").unwrap_err().code, "INVALID_ARGUMENT");

        // 整篇太长（> 8000）→ MEMORY_TOO_LARGE（**不是** INVALID_ARGUMENT）
        let e = write(&dir, &"字".repeat(MAX_CHARS + 1)).unwrap_err();
        assert_eq!(e.code, "MEMORY_TOO_LARGE");
        assert!(e.message.starts_with("记忆太长（8001 > 8000 字符）"), "{}", e.message);

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
}
