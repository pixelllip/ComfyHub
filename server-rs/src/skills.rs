//! Skills 的**读写两侧**（run-kit 拼系统提示 + `/api/ai/skills/*` 界面端点）。
//!
//! 真源是磁盘（`<storage>\ai\skills` 与 `<项目根>\skills\builtin`），**刻意不做缓存**：
//! "AI 注册完下一次回复就能用、用户删掉立刻消失"是需求（AGENTS §10）。
//!
//! P4 第 22 步先搬了 run-kit 用得到的那一半（扫描 + frontmatter 解析 + 校验 + 读正文），
//! P4 第 35 步补齐**写侧**（`save` / `delete` / `autoRegister`）与**界面 DTO**（`SkillDto`
//! 的 15 个键）—— 在此之前设置页那张卡只能看、什么也做不了。
//!
//! # 三条容易做错的地方（都照抄 Kotlin）
//!
//! 1. **非法项不静默忽略**：照样列出来、带 `validationError`，但**不进系统提示**
//!    （`catalog()` 把它们滤掉）—— AIH-038。这里若图省事直接跳过坏文件，
//!    用户就会看到"装了个 skill 却没生效"而没有任何线索。
//! 2. **YAML 块标量必须认**（`description: |` / `>-`）：真实 SKILL.md 十有八九这么写，
//!    不认就读出字面量 `|`，等于这个 skill 在目录里"没有描述"。
//! 3. **同名时"用户 > 内置"、同根内"bundle（`<name>/SKILL.md`）> 平铺 `<name>.md`"**
//!    —— 与 `fileOf` 的查找顺序保持一致。
//!
//! # 写侧的四条纪律（错了都不报错，只是"用户的 skill 不见了"）
//!
//! * **自动登记只补没有 frontmatter 的文件**：已经有 frontmatter 的**一个字节都不许改**
//!   （哪怕它不合法 —— 那是用户写的内容，按既有规则"列出来 + 带 validationError"）；
//! * **内置 skill 只能停用不能删除**（`SKILL_READONLY`），删的是**用户**那份；
//! * `save` 写的永远是 `<用户根>/<name>/SKILL.md`（不存在"写进内置目录"这条路）；
//! * 路径先过 `safe_child`（`..` 之类不许逃出用户根）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// 进系统提示的一个 skill（提示词只用得到前三个字段）。
#[derive(Debug, Clone, PartialEq)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    /// frontmatter 里的 `version`（可选）—— `load_skill` 把它写进
    /// `<skill_content name="…" version="…">`，与 Kotlin 同形。
    pub version: Option<String>,
}

impl SkillEntry {
    /// 进提示词的描述（**单行**）：目录是一行一条，块标量写出来的 description 自带换行，
    /// 不压平会把提示词撑乱（Kotlin `oneLineForPrompt`）。
    pub fn one_line_for_prompt(&self) -> String {
        collapse_whitespace(&self.description).trim().to_string()
    }
}

/// 名称规则（kebab-case，≤64）；与 Kotlin `SkillStore.NAME_RE` 同一条。
fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Java 的 `\s+` → 单个空格（**ASCII** 空白集：`Java Regex` 默认不开 UNICODE_CHARACTER_CLASS）。
///
/// 用 Rust 的 `\s`（Unicode 感知）会连全角空格一起吃掉，与 Kotlin 的输出不一致 ——
/// 而这段文本要进系统提示，差一个字符就是"两端规则不一样"。
pub fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_run = false;
    for ch in text.chars() {
        let is_ascii_space = matches!(ch, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r');
        if is_ascii_space {
            if !in_run {
                out.push(' ');
                in_run = true;
            }
        } else {
            out.push(ch);
            in_run = false;
        }
    }
    out
}

/// 极简 frontmatter 解析（Kotlin `parseFrontMatter` 的逐行对照）。
///
/// 返回 `(meta, body)`；没有 frontmatter 时 meta 为空、body 是原文。
pub fn parse_front_matter(raw: &str) -> (BTreeMap<String, String>, String) {
    let text = raw.strip_prefix('\u{FEFF}').unwrap_or(raw);
    let mut lines: Vec<String> = split_lines(text);
    if lines.first().map(|l| l.trim() != "---").unwrap_or(true) {
        return (BTreeMap::new(), text.to_string());
    }

    let mut meta: BTreeMap<String, String> = BTreeMap::new();
    let mut body_start: Option<usize> = None;
    let mut i = 1usize;
    while i < lines.len() {
        let line = lines[i].trim().to_string();
        if line == "---" {
            body_start = Some(i + 1);
            break;
        }
        if line.is_empty() || line.starts_with('#') {
            i += 1;
            continue;
        }
        let Some(idx) = line.find(':') else {
            i += 1;
            continue;
        };
        if idx == 0 {
            i += 1;
            continue;
        }
        let key = line[..idx].trim().to_lowercase().replace('_', "-");
        let mut value = line[idx + 1..].trim().to_string();

        if is_block_scalar(&value) {
            let fold = value.starts_with('>');
            let mut collected: Vec<String> = Vec::new();
            let mut j = i + 1;
            while j < lines.len() {
                let next = lines[j].clone();
                if !next.trim().is_empty() && !next.starts_with(' ') && !next.starts_with('\t') {
                    break;
                }
                collected.push(next);
                j += 1;
            }
            while collected.first().map(|l| l.trim().is_empty()).unwrap_or(false) {
                collected.remove(0);
            }
            while collected.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
                collected.pop();
            }
            let indent = collected
                .iter()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.chars().take_while(|c| *c == ' ').count())
                .min()
                .unwrap_or(0);
            let body: Vec<String> = collected
                .iter()
                .map(|l| if l.len() >= indent { l[indent..].to_string() } else { String::new() })
                .collect();
            let joined = if fold { body.join(" ") } else { body.join("\n") };
            meta.insert(key, joined.trim().to_string());
            i = j;
            continue;
        }

        if !value.starts_with('"') && !value.starts_with('\'') {
            if let Some(pos) = value.find(" #") {
                value = value[..pos].trim().to_string();
            }
        }
        let trimmed = value.trim().trim_matches('"').trim_matches('\'').to_string();
        meta.insert(key, trimmed);
        i += 1;
    }

    let Some(body_start) = body_start else {
        return (BTreeMap::new(), text.to_string());
    };
    let body = lines.split_off(body_start.min(lines.len()));
    (meta, body.join("\n"))
}

/// `|` / `|-` / `>` / `>-`（可带一位缩进位数）。
fn is_block_scalar(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else { return false };
    if first != '|' && first != '>' {
        return false;
    }
    let rest: String = chars.collect();
    if rest.is_empty() {
        return true;
    }
    let rest = rest.strip_prefix(['-', '+']).unwrap_or(&rest);
    rest.len() <= 1 && rest.chars().all(|c| c.is_ascii_digit())
}

/// Kotlin `String.lines()`：按 `\n` / `\r\n` / `\r` 拆，且末尾换行不产生空行。
fn split_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\n' => {
                out.push(std::mem::take(&mut current));
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// 一个 skill 文件 → 条目；**校验不过也返回**（`valid=false`），由调用方决定怎么处理。
#[derive(Debug, Clone)]
pub struct ParsedSkill {
    pub entry: SkillEntry,
    pub valid: bool,
    /// builtin / user（Run 的 Skills 快照要如实记下来源，AIH-047）
    pub source: &'static str,
    /// 正文 `sha256` 的**前 16 位十六进制**（与 Kotlin `SkillStore.sha256` 同一个口径：
    /// 对**文件原文**取哈希，不是对解析后的 body）
    pub digest: String,
    /// `user-invocable`（缺省 true）—— 界面显示 + `snapshot` 里"能不能被用户点"
    pub user_invocable: bool,
    /// `disable-model-invocation` 取反（缺省 true）
    pub model_invocable: bool,
    /// 文件原文的字节数
    pub size_bytes: i64,
    /// bundle 目录里的文件数（平铺 `.md` 恒为 1）—— 与 Kotlin `Files.walk(parent, 3)` 同口径
    pub file_count: i64,
    /// 文件 mtime（Java `Instant.toString()` 同形）；拿不到就退回"现在"
    pub updated_at: String,
    /// **第一条**没过的校验（Kotlin `parseOne` 里 `fail()` 只记第一条）—— 非空 = 不合法
    pub validation_error: Option<String>,
}

impl ParsedSkill {
    pub fn name(&self) -> &str {
        &self.entry.name
    }
}

/// `String.toBooleanStrictOrNull()`：只认全小写的 `true` / `false`，别的一律 `None`。
fn strict_bool(value: Option<&String>) -> Option<bool> {
    match value.map(|v| v.as_str()) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

/// `sha256(原文)` 的前 16 位（Kotlin `SkillStore.sha256` 的逐字对应物）。
fn short_digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let out = hasher.finalize();
    out.iter().map(|b| format!("{b:02x}")).collect::<String>().chars().take(16).collect()
}

/// description 的**安全上限**（只约束 `save()` 这条写入路径，扫描已有 skill 时不判它）。
pub const MAX_DESCRIPTION: usize = 4000;
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// 自动登记时描述的兜底长度（取正文第一行的前若干字）。
const AUTO_DESCRIPTION_CHARS: usize = 160;

/// `Files.walk(parent, 3)` 里的最大深度（Kotlin `SkillStore.parseOne`）。
const FILE_COUNT_MAX_DEPTH: usize = 3;

/// `bundle 目录里的文件数`：`parent.fileName == name` 时数到 3 层，否则 1。
fn count_files_under(parent: &Path, name: &str) -> i64 {
    let is_bundle = parent
        .file_name()
        .map(|n| n.to_string_lossy() == name)
        .unwrap_or(false);
    if !is_bundle {
        return 1;
    }
    fn walk(dir: &Path, depth: usize, count: &mut i64) {
        if depth > FILE_COUNT_MAX_DEPTH {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => walk(&path, depth + 1, count),
                Ok(meta) if meta.is_file() => *count += 1,
                _ => {}
            }
        }
    }
    let mut count = 0i64;
    walk(parent, 1, &mut count);
    count
}

pub fn parse_one(file: &Path, fallback_name: &str, source: &'static str) -> Option<ParsedSkill> {
    let raw = std::fs::read_to_string(file).ok()?;
    let (meta, body) = parse_front_matter(&raw);
    let name = meta
        .get("name")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback_name.to_string());

    // `fail(msg)`：**只记第一条**（Kotlin 的 `if (error == null) error = msg`）——
    // 顺序也是 Kotlin 那个顺序，界面上的诊断文案才对得上。
    let mut error: Option<String> = None;
    let mut fail = |msg: String| {
        if error.is_none() {
            error = Some(msg);
        }
    };
    if !valid_name(&name) {
        fail("name 必须是 kebab-case（小写字母 / 数字 / 连字符，≤64）".to_string());
    }
    // frontmatter 的 name 与目录名不一致 → 非法（同一个 skill 两个名字，加载时会打架）
    if name != fallback_name
        && file
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy() == fallback_name)
            .unwrap_or(false)
    {
        fail(format!("frontmatter 的 name「{name}」与目录名「{fallback_name}」不一致"));
    }
    let description = meta.get("description").cloned().unwrap_or_default();
    if description.trim().is_empty() {
        fail("缺少 description".to_string());
    }
    if body.trim().is_empty() {
        fail("正文为空".to_string());
    } else if body.len() > MAX_BODY_BYTES {
        fail(format!("正文超过 {MAX_BODY_BYTES} 字节"));
    }

    let parent = file.parent().unwrap_or(Path::new("."));
    let file_count = count_files_under(parent, &name);
    let updated_at = std::fs::metadata(file)
        .and_then(|m| m.modified())
        .map(crate::iso_time::from_system_time)
        .unwrap_or_else(|_| crate::iso_time::from_system_time(std::time::SystemTime::now()));

    Some(ParsedSkill {
        entry: SkillEntry {
            name,
            description,
            when_to_use: meta.get("whentouse").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            version: meta.get("version").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        },
        valid: error.is_none(),
        source,
        digest: short_digest(&raw),
        user_invocable: strict_bool(meta.get("user-invocable")).unwrap_or(true),
        model_invocable: !strict_bool(meta.get("disable-model-invocation")).unwrap_or(false),
        size_bytes: raw.len() as i64,
        file_count,
        updated_at,
        validation_error: error,
    })
}

/// 扫一个根目录（同名时 **bundle（`<name>/SKILL.md`）胜出**）。
fn scan_root(root: &Path, source: &'static str) -> Vec<(String, ParsedSkill)> {
    if !is_dir_no_follow(root) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    // 目录枚举顺序是文件系统给的：这里**按文件名排一次**，否则同名冲突时哪份生效
    // 取决于磁盘顺序（Kotlin 侧是 `Files.list`，同样没有保证 —— 排序至少让两端可复现）
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();

    let mut by_name: Vec<(String, ParsedSkill, bool)> = Vec::new();
    for entry in paths {
        let file_name = entry.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let is_dir = is_dir_no_follow(&entry);
        let file = if is_dir {
            let bundle = entry.join("SKILL.md");
            let lower = entry.join("skill.md");
            if bundle.is_file() {
                Some(bundle)
            } else if lower.is_file() {
                Some(lower)
            } else {
                None
            }
        } else if file_name.to_lowercase().ends_with(".md") {
            Some(entry.clone())
        } else {
            None
        };
        let Some(file) = file else { continue };
        let fallback = file_name
            .strip_suffix(".md")
            .or_else(|| file_name.strip_suffix(".MD"))
            .unwrap_or(&file_name)
            .to_string();
        let Some(parsed) = parse_one(&file, &fallback, source) else { continue };
        let name = parsed.entry.name.clone();
        match by_name.iter_mut().find(|(n, _, _)| *n == name) {
            Some(slot) => {
                // 已有一条：只有"这条是目录形态、那条不是"时才换（bundle 胜出）
                if is_dir && !slot.2 {
                    *slot = (name, parsed, is_dir);
                }
            }
            None => by_name.push((name, parsed, is_dir)),
        }
    }
    by_name.into_iter().map(|(name, parsed, _)| (name, parsed)).collect()
}

/// 全量扫描（内置 + 用户），同名冲突 **用户胜出**（AIH-037）。
///
/// 返回 `(条目, conflict)`：`conflict` 非空 = 这份是**覆盖了同名内置版**的用户版
/// （Kotlin `SkillDto.conflict` 的来源；`save()` 那条路径也会用 `origin` 记来源，
/// 我们这边**只有界面的 upsert 一种来源**，所以 `origin` 恒为 null）。
fn scan_with_conflict(builtin_root: &Path, user_root: &Path) -> Vec<(ParsedSkill, Option<String>)> {
    let builtin = scan_root(builtin_root, "builtin");
    let user = scan_root(user_root, "user");
    let mut out: Vec<(ParsedSkill, Option<String>)> = Vec::new();
    for (name, dto) in &builtin {
        match user.iter().find(|(n, _)| n == name) {
            // 用户那份胜出；`conflict` 只是提示，不影响可用性
            Some((_, override_dto)) => out.push((override_dto.clone(), Some("覆盖了同名内置 skill".to_string()))),
            None => out.push((dto.clone(), None)),
        }
    }
    for (name, dto) in &user {
        if !builtin.iter().any(|(n, _)| n == name) {
            out.push((dto.clone(), None));
        }
    }
    out.sort_by(|a, b| a.0.entry.name.cmp(&b.0.entry.name));
    out
}

/// 全量扫描（丢掉 `conflict`）—— `catalog` / `snapshot` / `scan_all` 用。
fn scan(builtin_root: &Path, user_root: &Path) -> Vec<ParsedSkill> {
    scan_with_conflict(builtin_root, user_root).into_iter().map(|(s, _)| s).collect()
}

// ---------------------------------------------------------------------------
//  界面 DTO（`/api/ai/skills/*`）—— 15 个键与 Kotlin `SkillDto` 逐字对齐
// ---------------------------------------------------------------------------

/// 界面上的一个 skill（与 Kotlin `SkillDto` 同形）。
///
/// ⚠ **可空键一个都不能自己冒出来**（`explicitNulls = false`）：`whenToUse` / `version` /
/// `digest` / `updatedAt` / `validationError` / `conflict` / `origin` 都是"没有就整个键不出现"。
/// `shape_lint.rs` 会替我们盯住这条。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDto {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// builtin / user
    pub source: String,
    pub enabled: bool,
    pub user_invocable: bool,
    pub model_invocable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    pub size_bytes: i64,
    pub file_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// 非空表示这个 skill 不合法：列表里显示诊断，且不参与对话
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_error: Option<String>,
    /// 非空的"提示"类信息（例如覆盖了同名内置 skill）：不影响可用性
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

impl SkillDto {
    fn of(parsed: &ParsedSkill, conflict: Option<String>) -> Self {
        Self {
            name: parsed.entry.name.clone(),
            description: parsed.entry.description.clone(),
            when_to_use: parsed.entry.when_to_use.clone(),
            version: parsed.entry.version.clone(),
            source: parsed.source.to_string(),
            // `enabled` 是界面上的"停用"开关，扫描这条路上恒为 true（Kotlin 同）
            enabled: true,
            user_invocable: parsed.user_invocable,
            model_invocable: parsed.model_invocable,
            digest: Some(parsed.digest.clone()),
            size_bytes: parsed.size_bytes,
            file_count: parsed.file_count,
            updated_at: Some(parsed.updated_at.clone()),
            validation_error: parsed.validation_error.clone(),
            conflict,
            origin: None,
        }
    }
}

/// 两个根目录（`<storage>\ai\skills` 与 `<项目根>\skills\builtin`）。
///
/// **只有这一处拼路径**：端点的 `roots` / `scan` / `save` 全从它来 ——
/// 分开拼的话，发布包（便携式）与源码树两种布局就会有一处拼错。
pub fn roots_of(state: &crate::routes::AppState) -> (PathBuf, PathBuf) {
    let storage = state.media_dir().parent().unwrap_or(state.media_dir()).to_path_buf();
    (
        state.project_root().join("skills").join("builtin"),
        storage.join("ai").join("skills"),
    )
}

/// `GET /api/ai/skills` —— 全量清单（**含非法项**，AIH-038）。
pub fn list(state: &crate::routes::AppState) -> Vec<SkillDto> {
    let (builtin, user) = roots_of(state);
    scan_with_conflict(&builtin, &user)
        .iter()
        .map(|(parsed, conflict)| SkillDto::of(parsed, conflict.clone()))
        .collect()
}

/// 投放口的位置（`SkillRootsDto`）：界面要显示绝对路径，用户得知道往哪个文件夹拷。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRootsDto {
    pub user_root: String,
    pub builtin_root: String,
    pub user_root_exists: bool,
}

pub fn roots_dto(state: &crate::routes::AppState) -> SkillRootsDto {
    let (builtin, user) = roots_of(state);
    SkillRootsDto {
        user_root: user.to_string_lossy().to_string(),
        builtin_root: builtin.to_string_lossy().to_string(),
        user_root_exists: user.is_dir(),
    }
}

/// 工具层的稳定错误码（Kotlin `ToolFailure(code, message)`，经 `toolGuard` 变成
/// `AiException` → **400 + `{error: 稳定码, detail: 人话}`**）。
///
/// 为什么单列一个类型：这三档（非法名 / 只读 / 不合法）**必须**原样把码传出去 ——
/// 一律写成 `CONFIG_ERROR` 的话，界面就没法按码分支（"这个名字不能用"与"内置的不能删"
/// 是两种完全不同的处置）。
pub struct SkillFailure {
    pub code: &'static str,
    pub message: String,
}

impl SkillFailure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    pub fn into_api_error(self) -> crate::routes::ApiError {
        crate::routes::ApiError::ai(self.code, self.message)
    }
}

/// `SkillStore.validateName`。
pub fn validate_name(name: &str) -> Result<(), SkillFailure> {
    if valid_name(name) {
        return Ok(());
    }
    Err(SkillFailure::new(
        "INVALID_SKILL_NAME",
        format!("非法 skill 名「{name}」：只允许小写字母 / 数字 / 连字符，且以字母或数字开头（≤64 字符）"),
    ))
}

/// `safeChild`：把 `<name>` 拼到用户根下，**并确认没逃出去**（`..` / 绝对路径都不行）。
fn safe_child(root: &Path, name: &str) -> Result<PathBuf, SkillFailure> {
    // `Path::join` 遇到绝对路径会**整段替换**（`join("D:\\x")` = `D:\x`）—— 先挡掉
    if Path::new(name).is_absolute() || name.contains(['/', '\\']) {
        return Err(SkillFailure::new("PATH_DENIED", format!("非法 skill 路径：{name}")));
    }
    let child = root.join(name);
    if !child.starts_with(root) {
        return Err(SkillFailure::new("PATH_DENIED", format!("非法 skill 路径：{name}")));
    }
    Ok(child)
}

/// 读一个 skill 的**详情**（元数据 + 正文）：`(dto, 去 frontmatter 的正文)`。
///
/// 三档"没有"分清（照抄 Kotlin）：没有这个名字 → `None`（路由翻成 `CONFIG_ERROR`）；
/// 有但不合法 → `SKILL_INVALID`；有合法但正文文件找不到 → `SKILL_NOT_FOUND`。
pub fn read_detail(
    state: &crate::routes::AppState,
    name: &str,
) -> Result<Option<(SkillDto, String)>, SkillFailure> {
    let found = list(state).into_iter().find(|dto| dto.name == name);
    let Some(dto) = found else { return Ok(None) };
    if let Some(error) = &dto.validation_error {
        return Err(SkillFailure::new(
            "SKILL_INVALID",
            format!("skill「{name}」不合法，不能加载：{error}"),
        ));
    }
    let (builtin, user) = roots_of(state);
    let file = file_of(&builtin, &user, name)
        .map(|(path, _)| path)
        .ok_or_else(|| SkillFailure::new("SKILL_NOT_FOUND", format!("找不到 skill「{name}」的正文文件")))?;
    let raw = std::fs::read_to_string(&file)
        .map_err(|e| SkillFailure::new("IO_ERROR", format!("读不了 skill「{name}」的正文：{e}")))?;
    let (_, body) = parse_front_matter(&raw);
    Ok(Some((dto, body.trim().to_string())))
}

/// `SkillStore.render`：写盘的格式（**逐字**，包括 `version: 1` 那个固定值）。
fn render(name: &str, description: &str, when_to_use: Option<&str>, body: &str) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("name: {name}\n"));
    out.push_str(&format!("description: {}\n", description.replace('\n', " ")));
    if let Some(when) = when_to_use.map(str::trim).filter(|w| !w.is_empty()) {
        out.push_str(&format!("whenToUse: {}\n", when.replace('\n', " ")));
    }
    out.push_str("version: 1\n");
    out.push_str("---\n\n");
    out.push_str(body.trim());
    out.push('\n');
    out
}

/// `POST /api/ai/skills` —— 注册 / 覆盖一个 skill（**永远写用户根**）。
pub fn save(
    state: &crate::routes::AppState,
    name: &str,
    description: &str,
    when_to_use: Option<&str>,
    content: &str,
) -> Result<SkillDto, SkillFailure> {
    validate_name(name)?;
    let desc = description.trim();
    if desc.is_empty() {
        return Err(SkillFailure::new("INVALID_ARGUMENT", "description 必填：说明这个 skill 什么时候用"));
    }
    // Kotlin 的 `String.length` 是 **UTF-16 码元**数
    let desc_len = desc.encode_utf16().count();
    if desc_len > MAX_DESCRIPTION {
        return Err(SkillFailure::new(
            "INVALID_ARGUMENT",
            format!("description 太长（{desc_len} > {MAX_DESCRIPTION} 字符）"),
        ));
    }
    let body = content.trim();
    if body.is_empty() {
        return Err(SkillFailure::new("INVALID_ARGUMENT", "skill 正文不能为空"));
    }
    if body.len() > MAX_BODY_BYTES {
        return Err(SkillFailure::new(
            "INVALID_ARGUMENT",
            format!("skill 正文太大（{} > {MAX_BODY_BYTES} 字节）", body.len()),
        ));
    }

    let (_, user_root) = roots_of(state);
    std::fs::create_dir_all(&user_root)
        .map_err(|e| SkillFailure::new("IO_ERROR", format!("建不了 skill 目录（{}）：{e}", user_root.display())))?;
    let dir = safe_child(&user_root, name)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| SkillFailure::new("IO_ERROR", format!("建不了 skill 目录（{}）：{e}", dir.display())))?;
    let file = dir.join("SKILL.md");
    std::fs::write(&file, render(name, desc, when_to_use, body))
        .map_err(|e| SkillFailure::new("IO_ERROR", format!("写不了 skill 文件（{}）：{e}", file.display())))?;

    list(state)
        .into_iter()
        .find(|dto| dto.name == name)
        .ok_or_else(|| SkillFailure::new("IO_ERROR", format!("写入后读不到 skill「{name}」")))
}

/// `DELETE /api/ai/skills/{name}` —— **只允许删用户来源**（内置受保护，AIH-037）。
///
/// 返回 `Ok(false)` = 没有这个名字（Kotlin 回 `{deleted:false}`，**不是 404**）；
/// 删目录时连 `references/` 一起删（那是 bundle 的一部分）。
pub fn delete(state: &crate::routes::AppState, name: &str) -> Result<bool, SkillFailure> {
    validate_name(name)?;
    let found = list(state).into_iter().find(|dto| dto.name == name);
    let Some(dto) = found else { return Ok(false) };
    if dto.source != "user" {
        return Err(SkillFailure::new(
            "SKILL_READONLY",
            format!("「{name}」是内置 skill，只能停用不能删除"),
        ));
    }
    let (builtin, user_root) = roots_of(state);
    let Some((file, _)) = file_of(&builtin, &user_root, name) else { return Ok(false) };
    let _ = std::fs::remove_file(&file);
    // bundle 目录：整个删掉（含 references/）；平铺 `<name>.md` 只删文件
    if let Some(parent) = file.parent() {
        if parent != user_root && parent.starts_with(&user_root) {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
    Ok(true)
}

/// `SkillAutoRegisterResult`：这次补了哪些、哪些放进来也没法用。
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillAutoRegisterResult {
    pub registered: i64,
    pub names: Vec<String>,
    pub errors: Vec<String>,
}

/// 名字兜底：把文件名 / 目录名收敛成 kebab-case（`SkillStore.slugOf`）。
pub fn slug_of(raw: &str) -> Option<String> {
    let mut slug = String::new();
    let mut last_dash = true; // 前导的连字符要去掉
    for ch in raw.trim().to_lowercase().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(64).collect();
    let slug = slug.trim_matches('-').to_string();
    if !slug.is_empty() && valid_name(&slug) {
        Some(slug)
    } else {
        None
    }
}

/// 没有 description 时，用正文第一行（去掉标题井号之类的标记）兜一个。
fn describe(body: &str, label: &str) -> String {
    let first = body
        .lines()
        .map(|line| line.trim().trim_start_matches(['#', '>', '-', '*', ' ', '\t']).trim())
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let text = if first.is_empty() { format!("从 {label} 自动登记") } else { first.to_string() };
    // Kotlin `take(160)` 是 UTF-16 码元
    crate::memory::take_utf16(&text, AUTO_DESCRIPTION_CHARS)
}

/// 目录项 → (正文文件, 名字兜底)；不是 skill 条目返回 `None`（`SkillStore.entryOf`）。
fn entry_of(entry: &Path) -> Option<(PathBuf, String)> {
    let label = entry.file_name()?.to_string_lossy().to_string();
    if is_dir_no_follow(entry) {
        for candidate in ["SKILL.md", "skill.md"] {
            let file = entry.join(candidate);
            if file.is_file() {
                return Some((file, label));
            }
        }
        return None;
    }
    if label.to_lowercase().ends_with(".md") {
        let stem = label
            .strip_suffix(".md")
            .or_else(|| label.strip_suffix(".MD"))
            .unwrap_or(&label)
            .to_string();
        return Some((entry.to_path_buf(), stem));
    }
    None
}

/// `POST /api/ai/skills/rescan` —— 把丢进用户目录、**还没有 frontmatter** 的 skill 自动登记。
///
/// 为什么需要它：用户目录就是"把 skill 拷进来就算装好"的投放口，而从别处拷来的 `.md`
/// 通常不带 frontmatter，按 AIH-038 的规矩那属于非法、不能加载 —— 用户会以为"拷进去没用"。
///
/// **已经有 frontmatter 的文件一律不碰**（哪怕它不合法）：那是用户自己写的内容。
pub fn auto_register(state: &crate::routes::AppState) -> SkillAutoRegisterResult {
    let (_, user_root) = roots_of(state);
    if let Err(e) = std::fs::create_dir_all(&user_root) {
        return SkillAutoRegisterResult {
            registered: 0,
            names: Vec::new(),
            errors: vec![format!("建不了投放口（{}）：{e}", user_root.display())],
        };
    }
    let mut names: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    let mut entries: Vec<PathBuf> = match std::fs::read_dir(&user_root) {
        Ok(read) => read.flatten().map(|e| e.path()).collect(),
        Err(e) => {
            return SkillAutoRegisterResult {
                registered: 0,
                names: Vec::new(),
                errors: vec![format!("扫描投放口失败：{e}")],
            }
        }
    };
    // Kotlin 是 `Files.list(userRoot).sorted()` —— 按**整个路径**排，这里同样
    entries.sort();

    for entry in entries {
        let label = entry
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some((file, fallback)) = entry_of(&entry) else { continue };
        let Ok(raw) = std::fs::read_to_string(&file) else {
            errors.push(format!("{label}：读不了"));
            continue;
        };
        // 已经有 frontmatter：用户（或 AI）自己写过了，不碰
        if !parse_front_matter(&raw).0.is_empty() {
            continue;
        }
        let Some(name) = slug_of(&fallback) else {
            errors.push(format!(
                "{label}：文件名转不成 kebab-case 的 skill 名（中文名请在里面写 frontmatter 的 name）"
            ));
            continue;
        };
        let body = raw.trim();
        if body.is_empty() {
            errors.push(format!("{label}：正文为空"));
            continue;
        }
        if body.len() > MAX_BODY_BYTES {
            errors.push(format!("{label}：正文超过 {MAX_BODY_BYTES} 字节"));
            continue;
        }
        match std::fs::write(&file, render(&name, &describe(body, &label), None, body)) {
            Ok(()) => names.push(name),
            Err(e) => errors.push(format!("{label}：写入失败（{e}）")),
        }
    }

    errors.truncate(20);
    SkillAutoRegisterResult {
        registered: names.len() as i64,
        names,
        errors,
    }
}

/// 可以进系统提示的 skills（校验不过的**不进**，但也不是"没看见"——见文件头第 1 条）。
///
/// 顺序 = 按 name 升序（Kotlin `scan()` 末尾就是 `sortedBy { it.name }`；
/// 进得了这里的名字都是 kebab-case，ASCII 序与 Kotlin 的 UTF-16 序一致）。
pub fn catalog(builtin_root: &Path, user_root: &Path) -> Vec<SkillEntry> {
    scan(builtin_root, user_root)
        .into_iter()
        .filter(|s| s.valid)
        .map(|s| s.entry)
        .collect()
}

/// 一次 Run 的 **Skills 快照**（AIH-047）：`{count, skills:[{name, digest, source}]}`。
///
/// 为什么 Run 里要存这一份：事后才回答得了"这条回答当时用的是哪版规则"。
/// 没有它，用户改了 skill 之后再问"为什么它当时那么做"就永远查不清。
///
/// 形状与 Kotlin `AiRunRepo.create(skillSnapshot = …)` **逐字一致**：
/// `count` 是**这次生效的**条数（校验不过的与 `user-invocable=false` 的不算），
/// 每条三个键 `name` / `digest` / `source`，按 name 升序。
///
/// ⚠ `digest` 是**文件原文** sha256 的前 16 位（不是解析后的 body）——
/// 与 Kotlin `SkillStore.sha256(raw)` 同一个口径，改一个字就不同。
pub fn snapshot(state: &crate::routes::AppState) -> Result<serde_json::Value, crate::routes::ApiError> {
    let storage = state.media_dir().parent().unwrap_or(state.media_dir()).to_path_buf();
    let entries = scan(
        &state.project_root().join("skills").join("builtin"),
        &storage.join("ai").join("skills"),
    );
    let skills: Vec<serde_json::Value> = entries
        .into_iter()
        .filter(|s| s.valid)
        .map(|s| {
            serde_json::json!({
                "name": s.entry.name,
                "digest": s.digest,
                "source": s.source,
            })
        })
        .collect();
    Ok(serde_json::json!({ "count": skills.len(), "skills": skills }))
}

/// 一个 skill 的**正文文件位置**与来源（`load_skill` 要读它）。
///
/// 返回 `(路径, source)`。与 [`catalog`] 走**同一次扫描**：分开扫两次会出现
/// "目录里有它、load 说找不到"这种自相矛盾。
pub fn file_of(builtin_root: &Path, user_root: &Path, name: &str) -> Option<(PathBuf, &'static str)> {
    let trimmed = name.trim();
    // 走 `scan_root` 而不是 `scan`：要连**不合法**的那条也找得到，
    // 这样 `load_skill` 才能区分"没有这个 skill"与"有这个 skill 但它不合法"
    for (root, source) in [(user_root, "user"), (builtin_root, "builtin")] {
        // 目录形态优先（bundle 胜出，与 `scan_root` 同一条规则）
        let bundle = root.join(trimmed).join("SKILL.md");
        if bundle.is_file() {
            return Some((bundle, source));
        }
        let lower = root.join(trimmed).join("skill.md");
        if lower.is_file() {
            return Some((lower, source));
        }
        let flat = root.join(format!("{trimmed}.md"));
        if flat.is_file() {
            return Some((flat, source));
        }
    }
    None
}

/// 一个 skill 的正文（**去掉 frontmatter**）—— `load_skill` 返回给模型的就是它。
pub struct SkillBody {
    pub entry: SkillEntry,
    pub body: String,
    /// 校验通过没有（`valid=false` 的不许加载 —— 与 `catalog` 用的是同一个 `parse_one`）
    pub valid: bool,
    /// 文件原文 sha256 的前 16 位（`load_skill` 的结构化结果里要带上它）
    pub digest: String,
}

/// 读一个 skill 的正文。
///
/// `None` = 没有这个名字的 skill。**不缓存**：AI 刚注册完，下一次 `load_skill` 就要读到新的。
pub fn read_body(builtin_root: &Path, user_root: &Path, name: &str) -> Option<SkillBody> {
    let (file, _) = file_of(builtin_root, user_root, name)?;
    let raw = std::fs::read_to_string(&file).ok()?;
    let (_, body) = parse_front_matter(&raw);
    let fallback = file
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| name.trim().to_string());
    let parsed = parse_one(&file, &fallback, "user")?;
    Some(SkillBody {
        entry: parsed.entry,
        body: body.trim().to_string(),
        valid: parsed.valid,
        digest: parsed.digest,
    })
}

/// 扫全量（**含非法项**）—— 给 `list_skills` 工具用。
///
/// 为什么不复用 [`catalog`]：那个**刻意**只给能用的（它进系统提示），
/// 而 `list_skills` 要的是"装了但有问题"的那些 —— 静默过滤掉正是 AIH-038 要防的事
/// （用户看到"装了个 skill 却没生效"而没有任何线索）。
pub fn scan_all(builtin_root: &Path, user_root: &Path) -> Vec<ScannedSkill> {
    scan(builtin_root, user_root)
        .into_iter()
        .map(|s| {
            // `validation_error` 现在就是 `parse_one` 里那条**原话**（以前这里是按 `valid`
            // 反推一句概括，而 `valid=false` 的原因有五种，概括出来的话与界面看到的不一样）
            let error = s.validation_error.clone().unwrap_or_default();
            ScannedSkill {
                name: s.entry.name,
                description: s.entry.description,
                when_to_use: s.entry.when_to_use,
                source: s.source,
                valid: s.valid,
                error,
            }
        })
        .collect()
}

/// 一个 skill 的全量信息（`list_skills` 的返回项）。
pub struct ScannedSkill {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub source: &'static str,
    pub valid: bool,
    /// 非空 = 不合格，内容是给人看的原因
    pub error: String,
}

/// `Files.isDirectory(path, NOFOLLOW_LINKS)`：**不看符号链接指向什么**。
fn is_dir_no_follow(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_认块标量与引号() {
        let raw = "---\nname: demo\ndescription: |\n  第一行\n  第二行\nwhenToUse: '场景 X'\n---\n正文\n";
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
        let dir = crate::testpaths::scratch("skills");
        let user = dir.join("user");
        std::fs::create_dir_all(user.join("good-skill")).unwrap();
        std::fs::write(
            user.join("good-skill").join("SKILL.md"),
            "---\nname: good-skill\ndescription: 好用\n---\n正文",
        )
        .unwrap();
        std::fs::write(user.join("flat.md"), "---\nname: flat\ndescription: 平铺\n---\n正文").unwrap();
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
        assert_eq!(names, vec!["flat", "good-skill"], "非法项不进 catalog，合法的按 name 升序");

        let all = scan(&dir.join("builtin"), &user);
        assert_eq!(all.len(), 4, "scan() 照样列出非法项（界面要显示诊断）");
        assert_eq!(all.iter().filter(|e| !e.valid).count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 同名时用户版胜出() {
        let dir = crate::testpaths::scratch("skills-override");
        let builtin = dir.join("builtin");
        let user = dir.join("user");
        std::fs::create_dir_all(&builtin).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(builtin.join("same.md"), "---\nname: same\ndescription: 内置\n---\n正文").unwrap();
        std::fs::write(user.join("same.md"), "---\nname: same\ndescription: 用户\n---\n正文").unwrap();
        let found = catalog(&builtin, &user);
        assert_eq!(found.len(), 1, "同名只留一条");
        assert_eq!(found[0].description, "用户");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
