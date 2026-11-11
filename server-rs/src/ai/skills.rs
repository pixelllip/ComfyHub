//! Skills 的**读写两侧**（run-kit 拼系统提示 + `/api/ai/skills/*` 界面端点）。
//!
//! 真源是磁盘（`<storage>\ai\skills` 与 `<项目根>\skills\builtin`），**刻意不做缓存**：
//! "注册后下一次回复即可使用、删除后立即失效"是需求（AGENTS §10）。
//!
//! P4 第 22 步先搬了 run-kit 用得到的那一半（扫描 + frontmatter 解析 + 校验 + 读正文），
//! P4 第 35 步补齐**写侧**（`save` / `delete` / `autoRegister`）与**界面 DTO**（`SkillDto`
//! 的 15 个键）—— 在此之前设置页那张卡只能看、什么也做不了。
//!
//! # 三条容易做错的地方（都照抄 Kotlin）
//!
//! 1. **非法项不静默忽略**：照样列出来、带 `validationError`，但**不进系统提示**
//!    （`catalog()` 把它们滤掉）—— AIH-038。这里若图省事直接跳过坏文件，
//!    否则会表现为"安装了 skill 但未生效"且没有提示。
//! 2. **YAML 块标量必须认**（`description: |` / `>-`）：真实 SKILL.md 十有八九这么写，
//!    不认就读出字面量 `|`，等于这个 skill 在目录里"没有描述"。
//! 3. **同名时"自定义 > 内置"、同根内"bundle（`<name>/SKILL.md`）> 平铺 `<name>.md`"**
//!    —— 与 `fileOf` 的查找顺序保持一致。
//!
//! # 写侧四条约束（违反时不报错，表现为"自定义 skill 消失"）
//!
//! * **自动登记只补没有 frontmatter 的文件**：已经有 frontmatter 的**一个字节都不许改**
//!   （即使不合法 —— 属于自定义内容，按既有规则"列出 + 带 validationError"）；
//! * **内置 skill 只能停用不能删除**（`SKILL_READONLY`），删除的是**自定义**那份；
//! * `save` 写入的始终是 `<自定义根>/<name>/SKILL.md`（不存在"写入内置目录"的路径）；
//! * 路径先经 `safe_child`（`..` 等不得逃出自定义根）。

use std::collections::{BTreeMap, HashSet};
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
            while collected
                .first()
                .map(|l| l.trim().is_empty())
                .unwrap_or(false)
            {
                collected.remove(0);
            }
            while collected
                .last()
                .map(|l| l.trim().is_empty())
                .unwrap_or(false)
            {
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
                .map(|l| {
                    if l.len() >= indent {
                        l[indent..].to_string()
                    } else {
                        String::new()
                    }
                })
                .collect();
            let joined = if fold {
                body.join(" ")
            } else {
                body.join("\n")
            };
            meta.insert(key, joined.trim().to_string());
            i = j;
            continue;
        }

        if !value.starts_with('"') && !value.starts_with('\'') {
            if let Some(pos) = value.find(" #") {
                value = value[..pos].trim().to_string();
            }
        }
        let trimmed = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
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
    let Some(first) = chars.next() else {
        return false;
    };
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
    /// `user-invocable`（缺省 true）—— 界面显示 + `snapshot` 中"是否可点击"
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
    out.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        .chars()
        .take(16)
        .collect()
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
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
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
        fail(format!(
            "frontmatter 的 name「{name}」与目录名「{fallback_name}」不一致"
        ));
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
        .map(crate::infra::iso_time::from_system_time)
        .unwrap_or_else(|_| crate::infra::iso_time::from_system_time(std::time::SystemTime::now()));

    Some(ParsedSkill {
        entry: SkillEntry {
            name,
            description,
            when_to_use: meta
                .get("whentouse")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            version: meta
                .get("version")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
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

/// 扫一个根目录（同名时 **bundle（`<name>/SKILL.md`）胜出**），**带上真实路径**。
///
/// 路径必须由扫描结果带出来，而不是拿 name 反拼一个 —— 平铺 `<x>.md` 的
/// frontmatter `name` 可以与文件名不同，反拼就会出现"列表里有、加载时说没有"。
fn scan_root_paths(root: &Path, source: &'static str) -> Vec<(String, PathBuf, ParsedSkill)> {
    if !is_dir_no_follow(root) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    // 目录枚举顺序是文件系统给的：这里**按文件名排一次**，否则同名冲突时哪份生效
    // 取决于磁盘顺序（Kotlin 侧是 `Files.list`，同样没有保证 —— 排序至少让两端可复现）
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();

    let mut by_name: Vec<(String, PathBuf, ParsedSkill, bool)> = Vec::new();
    for entry in paths {
        let file_name = entry
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
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
        let Some(parsed) = parse_one(&file, &fallback, source) else {
            continue;
        };
        let name = parsed.entry.name.clone();
        match by_name.iter_mut().find(|(n, _, _, _)| *n == name) {
            Some(slot) => {
                // 已有一条：只有"这条是目录形态、那条不是"时才换（bundle 胜出）
                if is_dir && !slot.3 {
                    *slot = (name, file, parsed, is_dir);
                }
            }
            None => by_name.push((name, file, parsed, is_dir)),
        }
    }
    by_name
        .into_iter()
        .map(|(name, file, parsed, _)| (name, file, parsed))
        .collect()
}

/// 扫一个根目录（丢掉路径）—— 界面 / 冲突判定用。
fn scan_root(root: &Path, source: &'static str) -> Vec<(String, ParsedSkill)> {
    scan_root_paths(root, source)
        .into_iter()
        .map(|(name, _, parsed)| (name, parsed))
        .collect()
}

/// 在**一个**根目录里按 name 找条目（连同真实路径）。
fn find_in_root(root: &Path, name: &str, source: &'static str) -> Option<(PathBuf, ParsedSkill)> {
    scan_root_paths(root, source)
        .into_iter()
        .find(|(n, _, _)| n == name)
        .map(|(_, path, parsed)| (path, parsed))
}

/// 一次 Run / 一次加载**真正生效**的那一份：
/// 自定义版合法 → 自定义版；否则内置版合法 → 内置版；再否则把自定义版（可能非法）作为诊断返回。
///
/// 这条顺序与 [`catalog`] 必须**逐字一致**，否则会出现"系统提示里是内置版、
/// `load_skill` 却读到自定义非法版"的矛盾。
pub fn effective_entry(
    builtin_root: &Path,
    user_root: &Path,
    name: &str,
) -> Option<(PathBuf, ParsedSkill)> {
    let user = find_in_root(user_root, name, "user");
    let builtin = find_in_root(builtin_root, name, "builtin");
    match (user, builtin) {
        (Some((path, parsed)), _) if parsed.valid => Some((path, parsed)),
        (_, Some((path, parsed))) if parsed.valid => Some((path, parsed)),
        (Some((path, parsed)), _) => Some((path, parsed)),
        (_, builtin) => builtin,
    }
}

/// 全量扫描（内置 + 自定义），同名冲突 **自定义优先**（AIH-037）。
///
/// 返回 `(条目, conflict)`：`conflict` 非空 = 该条目是**覆盖同名内置版**的自定义版
/// （Kotlin `SkillDto.conflict` 的来源；`save()` 那条路径也会用 `origin` 记来源，
/// 我们这边**只有界面的 upsert 一种来源**，所以 `origin` 恒为 null）。
fn scan_with_conflict(builtin_root: &Path, user_root: &Path) -> Vec<(ParsedSkill, Option<String>)> {
    let builtin = scan_root(builtin_root, "builtin");
    let user = scan_root(user_root, "user");
    let mut out: Vec<(ParsedSkill, Option<String>)> = Vec::new();
    for (name, dto) in &builtin {
        match user.iter().find(|(n, _)| n == name) {
            // 自定义版优先；`conflict` 仅作提示，不影响可用性
            Some((_, override_dto)) => out.push((
                override_dto.clone(),
                Some("覆盖了同名内置 skill".to_string()),
            )),
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
    scan_with_conflict(builtin_root, user_root)
        .into_iter()
        .map(|(s, _)| s)
        .collect()
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
pub fn roots_of(state: &crate::web::routes::AppState) -> (PathBuf, PathBuf) {
    let storage = state
        .media_dir()
        .parent()
        .unwrap_or(state.media_dir())
        .to_path_buf();
    (
        state.project_root().join("skills").join("builtin"),
        storage.join("ai").join("skills"),
    )
}

/// `GET /api/ai/skills` —— 全量清单（**含非法项**，AIH-038）。
pub fn list(state: &crate::web::routes::AppState) -> Vec<SkillDto> {
    let (builtin, user) = roots_of(state);
    let disabled = disabled_of(state);
    scan_with_conflict(&builtin, &user)
        .iter()
        .map(|(parsed, conflict)| {
            // 自定义版覆盖同名内置版、但自身不合法时：对话回落到内置版。
            // 必须说明该情况，否则会表现为"已覆盖但未生效"且没有提示。
            let conflict = match (conflict, parsed.valid) {
                (Some(_), false) => {
                    Some("覆盖了同名内置 skill；用户版不合法，对话仍用内置版".to_string())
                }
                (conflict, _) => conflict.clone(),
            };
            let mut dto = SkillDto::of(parsed, conflict);
            dto.enabled = !disabled.contains(parsed.entry.name.as_str());
            dto
        })
        .collect()
}

/// 停用名单的落点：`<storage>\ai\skills.disabled.json`（与投放口同级，不进投放口目录，
/// 免得被扫描当成一个 skill 条目）。
pub fn disabled_path(state: &crate::web::routes::AppState) -> PathBuf {
    let (_, user) = roots_of(state);
    user.parent().unwrap_or(&user).join("skills.disabled.json")
}

/// 读停用名单。读不到 / 坏了都当"没有停用"（**不拦对话**），但要留一行日志。
pub fn disabled_of(state: &crate::web::routes::AppState) -> HashSet<String> {
    let path = disabled_path(state);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return HashSet::new();
    };
    match serde_json::from_str::<Vec<String>>(&text) {
        Ok(names) => names.into_iter().collect(),
        Err(e) => {
            eprintln!("skills：停用名单读不了（{}）：{e}", path.display());
            HashSet::new()
        }
    }
}

fn write_disabled(path: &Path, names: &HashSet<String>) -> Result<(), String> {
    if names.is_empty() {
        let _ = std::fs::remove_file(path);
        return Ok(());
    }
    let mut sorted: Vec<&String> = names.iter().collect();
    sorted.sort();
    let body = serde_json::to_string_pretty(&sorted).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

/// 停用 / 启用一个 skill（内置与自定义均可；停用只影响对话，文件不改动）。
pub fn set_enabled(
    state: &crate::web::routes::AppState,
    name: &str,
    enabled: bool,
) -> Result<SkillDto, SkillFailure> {
    validate_name(name)?;
    let found = list(state)
        .into_iter()
        .find(|dto| dto.name == name)
        .ok_or_else(|| SkillFailure::new("SKILL_NOT_FOUND", format!("没有名为 {name} 的 skill")))?;
    let mut disabled = disabled_of(state);
    if enabled {
        disabled.remove(name);
    } else {
        disabled.insert(name.to_string());
    }
    write_disabled(&disabled_path(state), &disabled)
        .map_err(|e| SkillFailure::new("IO_ERROR", format!("保存 skill 启停状态失败：{e}")))?;
    Ok(SkillDto { enabled, ..found })
}

/// 投放口的位置（`SkillRootsDto`）：界面显示绝对路径，用于确定复制目标文件夹。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRootsDto {
    pub user_root: String,
    pub builtin_root: String,
    pub user_root_exists: bool,
}

pub fn roots_dto(state: &crate::web::routes::AppState) -> SkillRootsDto {
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
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn into_api_error(self) -> crate::web::routes::ApiError {
        crate::web::routes::ApiError::ai(self.code, self.message)
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

/// `safeChild`：把 `<name>` 拼到自定义根下，**并确认未越界**（`..` / 绝对路径均不允许）。
fn safe_child(root: &Path, name: &str) -> Result<PathBuf, SkillFailure> {
    // `Path::join` 遇到绝对路径会**整段替换**（`join("D:\\x")` = `D:\x`）—— 先挡掉
    if Path::new(name).is_absolute() || name.contains(['/', '\\']) {
        return Err(SkillFailure::new(
            "PATH_DENIED",
            format!("非法 skill 路径：{name}"),
        ));
    }
    let child = root.join(name);
    if !child.starts_with(root) {
        return Err(SkillFailure::new(
            "PATH_DENIED",
            format!("非法 skill 路径：{name}"),
        ));
    }
    Ok(child)
}

/// 读一个 skill 的**详情**（元数据 + 正文）：`(dto, 去 frontmatter 的正文)`。
///
/// 三档"没有"分清（照抄 Kotlin）：没有这个名字 → `None`（路由翻成 `CONFIG_ERROR`）；
/// 有但不合法 → `SKILL_INVALID`；有合法但正文文件找不到 → `SKILL_NOT_FOUND`。
pub fn read_detail(
    state: &crate::web::routes::AppState,
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
    // 按**该 DTO 自身的来源**取文件：自定义版覆盖内置版时，详情返回自定义版。
    let file = if dto.source == "user" {
        find_in_root(&user, name, "user")
    } else {
        find_in_root(&builtin, name, "builtin")
    }
    .map(|(path, _)| path)
    .ok_or_else(|| {
        SkillFailure::new(
            "SKILL_NOT_FOUND",
            format!("找不到 skill「{name}」的正文文件"),
        )
    })?;
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
    out.push_str(&format!(
        "description: {}\n",
        description.replace('\n', " ")
    ));
    if let Some(when) = when_to_use.map(str::trim).filter(|w| !w.is_empty()) {
        out.push_str(&format!("whenToUse: {}\n", when.replace('\n', " ")));
    }
    out.push_str("version: 1\n");
    out.push_str("---\n\n");
    out.push_str(body.trim());
    out.push('\n');
    out
}

/// `POST /api/ai/skills` —— 注册 / 覆盖一个 skill（**始终写自定义根**）。
pub fn save(
    state: &crate::web::routes::AppState,
    name: &str,
    description: &str,
    when_to_use: Option<&str>,
    content: &str,
) -> Result<SkillDto, SkillFailure> {
    validate_name(name)?;
    let desc = description.trim();
    if desc.is_empty() {
        return Err(SkillFailure::new(
            "INVALID_ARGUMENT",
            "description 必填：说明这个 skill 什么时候用",
        ));
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
    std::fs::create_dir_all(&user_root).map_err(|e| {
        SkillFailure::new(
            "IO_ERROR",
            format!("建不了 skill 目录（{}）：{e}", user_root.display()),
        )
    })?;
    let dir = safe_child(&user_root, name)?;
    std::fs::create_dir_all(&dir).map_err(|e| {
        SkillFailure::new(
            "IO_ERROR",
            format!("建不了 skill 目录（{}）：{e}", dir.display()),
        )
    })?;
    let file = dir.join("SKILL.md");
    std::fs::write(&file, render(name, desc, when_to_use, body)).map_err(|e| {
        SkillFailure::new(
            "IO_ERROR",
            format!("写不了 skill 文件（{}）：{e}", file.display()),
        )
    })?;

    // 刚显式写入该名称 = 需要使用它；从停用名单中移除。
    let mut disabled = disabled_of(state);
    if disabled.remove(name) {
        write_disabled(&disabled_path(state), &disabled)
            .map_err(|e| SkillFailure::new("IO_ERROR", format!("保存 skill 启停状态失败：{e}")))?;
    }

    list(state)
        .into_iter()
        .find(|dto| dto.name == name)
        .ok_or_else(|| SkillFailure::new("IO_ERROR", format!("写入后读不到 skill「{name}」")))
}

/// `DELETE /api/ai/skills/{name}` —— **只允许删除自定义来源**（内置受保护，AIH-037）。
///
/// 返回 `Ok(false)` = 没有这个名字（Kotlin 回 `{deleted:false}`，**不是 404**）；
/// 删目录时连 `references/` 一起删（那是 bundle 的一部分）。
pub fn delete(state: &crate::web::routes::AppState, name: &str) -> Result<bool, SkillFailure> {
    validate_name(name)?;
    let found = list(state).into_iter().find(|dto| dto.name == name);
    let Some(dto) = found else { return Ok(false) };
    if dto.source != "user" {
        return Err(SkillFailure::new(
            "SKILL_READONLY",
            format!("「{name}」是内置 skill，只能停用不能删除"),
        ));
    }
    let (_builtin, user_root) = roots_of(state);
    // ⚠ 只查找**自定义根**：自定义版非法覆盖内置版时，`file_of` 会回落到内置版路径 ——
    // 用它来删会删掉内置文件。
    let Some((file, _)) = find_in_root(&user_root, name, "user") else {
        return Ok(false);
    };
    let _ = std::fs::remove_file(&file);
    // bundle 目录：整个删掉（含 references/）；平铺 `<name>.md` 只删文件
    if let Some(parent) = file.parent() {
        if parent != user_root && parent.starts_with(&user_root) {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
    // 删掉的技能不该继续占着停用名单
    let mut disabled = disabled_of(state);
    if disabled.remove(name) {
        let _ = write_disabled(&disabled_path(state), &disabled);
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
        .map(|line| {
            line.trim()
                .trim_start_matches(['#', '>', '-', '*', ' ', '\t'])
                .trim()
        })
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let text = if first.is_empty() {
        format!("从 {label} 自动登记")
    } else {
        first.to_string()
    };
    // Kotlin `take(160)` 是 UTF-16 码元
    crate::ai::memory::take_utf16(&text, AUTO_DESCRIPTION_CHARS)
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

/// `POST /api/ai/skills/rescan` —— 把放入自定义目录、**还没有 frontmatter** 的 skill 自动登记。
///
/// 自定义目录即"复制进来即完成安装"的投放口，而从其他位置复制的 `.md`
/// 通常不带 frontmatter，按 AIH-038 属于非法、不能加载 —— 表现为"复制后无效"。
///
/// **已有 frontmatter 的文件一律不改动**（即使不合法）：属于自定义内容。
pub fn auto_register(state: &crate::web::routes::AppState) -> SkillAutoRegisterResult {
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
        let Some((file, fallback)) = entry_of(&entry) else {
            continue;
        };
        let Ok(raw) = std::fs::read_to_string(&file) else {
            errors.push(format!("{label}：读不了"));
            continue;
        };
        // 已有 frontmatter：已由自定义内容（或 AI）写入，不改动
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

/// 同名时**生效的那一份**：自定义版合法 → 自定义版；否则内置版合法 → 内置版。
///
/// 与 [`effective_entry`] 同一条顺序；与 `scan_with_conflict` 的区别是后者要给界面
/// 查看"自定义版不合法"的诊断，这里只回答"对话中使用哪一份"。
fn effective_scan(builtin_root: &Path, user_root: &Path) -> Vec<ParsedSkill> {
    let mut by_name: BTreeMap<String, ParsedSkill> = BTreeMap::new();
    for (name, parsed) in scan_root(builtin_root, "builtin") {
        by_name.insert(name, parsed);
    }
    for (name, parsed) in scan_root(user_root, "user") {
        let keep_builtin = by_name
            .get(&name)
            .map(|existing| !parsed.valid && existing.valid)
            .unwrap_or(false);
        if !keep_builtin {
            by_name.insert(name, parsed);
        }
    }
    by_name.into_values().collect()
}

/// 可以进系统提示的 skills（校验不过的**不进**，但也不是"没看见"——见文件头第 1 条）。
///
/// 过滤条件为**三项**：合法、`model-invocable`、未被停用。
/// 顺序 = 按 name 升序（BTreeMap 保证；进得了这里的名字都是 kebab-case，ASCII 序一致）。
pub fn catalog(builtin_root: &Path, user_root: &Path) -> Vec<SkillEntry> {
    catalog_with(builtin_root, user_root, &HashSet::new())
}

/// 带"停用名单"的 [`catalog`]（桌面执行器 / run-kit 走这一条）。
pub fn catalog_with(
    builtin_root: &Path,
    user_root: &Path,
    disabled: &HashSet<String>,
) -> Vec<SkillEntry> {
    effective_scan(builtin_root, user_root)
        .into_iter()
        .filter(|s| s.valid && s.model_invocable && !disabled.contains(s.entry.name.as_str()))
        .map(|s| s.entry)
        .collect()
}

/// 一次 Run 的 **Skills 快照**（AIH-047）：`{count, skills:[{name, digest, source}]}`。
///
/// 为什么 Run 里要存这一份：事后才回答得了"这条回答当时用的是哪版规则"。
/// 缺少它时，修改 skill 后无法查清"当时为何那样执行"。
///
/// 形状与 Kotlin `AiRunRepo.create(skillSnapshot = …)` **逐字一致**：
/// `count` 是**这次能用的**条数（校验不过的、被停用的不算；`disable-model-invocation`
/// 的**算**——可显式调用），按 name 升序。
///
/// ⚠ `digest` 是**文件原文** sha256 的前 16 位（不是解析后的 body）——
/// 与 Kotlin `SkillStore.sha256(raw)` 同一个口径，改一个字就不同。
pub fn snapshot(
    state: &crate::web::routes::AppState,
) -> Result<serde_json::Value, crate::web::routes::ApiError> {
    let (builtin, user) = roots_of(state);
    let disabled = disabled_of(state);
    let entries = effective_scan(&builtin, &user);
    let skills: Vec<serde_json::Value> = entries
        .into_iter()
        .filter(|s| s.valid && !disabled.contains(s.entry.name.as_str()))
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
/// 返回**实际生效**的那一份（自定义版合法优先，否则内置版）。
pub fn file_of(
    builtin_root: &Path,
    user_root: &Path,
    name: &str,
) -> Option<(PathBuf, &'static str)> {
    effective_entry(builtin_root, user_root, name).map(|(path, parsed)| (path, parsed.source))
}

/// 一个 skill 的正文（**去掉 frontmatter**）—— `load_skill` 返回给模型的就是它。
pub struct SkillBody {
    pub entry: SkillEntry,
    pub body: String,
    /// 校验通过没有（`valid=false` 的不许加载 —— 与 `catalog` 用的是同一个 `parse_one`）
    pub valid: bool,
    /// 文件原文 sha256 的前 16 位（`load_skill` 的结构化结果里要带上它）
    pub digest: String,
    /// builtin / user
    pub source: &'static str,
    /// 正文文件真实路径（`load_skill` 由它算 bundle 目录与附带文件）
    pub path: PathBuf,
    /// `disable-model-invocation` 取反：false 的只允许显式调用
    pub model_invocable: bool,
    /// `user-invocable`（缺省 true）：false 的不出现在 `/` 菜单里
    pub user_invocable: bool,
}

/// 读一个 skill 的正文。
///
/// `None` = 没有这个名字的 skill。**不缓存**：AI 刚注册完，下一次 `load_skill` 就要读到新的。
pub fn read_body(builtin_root: &Path, user_root: &Path, name: &str) -> Option<SkillBody> {
    // 直接用扫描结果里的 `ParsedSkill`：不要拿 name 反拼路径、也不要重新猜 fallback ——
    // 平铺 `<x>.md` 的 frontmatter name 可以与文件名不同，反拼会把它判成"name 与目录名不一致"。
    let (path, parsed) = effective_entry(builtin_root, user_root, name)?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let (_, body) = parse_front_matter(&raw);
    Some(SkillBody {
        entry: parsed.entry,
        body: body.trim().to_string(),
        valid: parsed.valid,
        digest: short_digest(&raw),
        source: parsed.source,
        path,
        model_invocable: parsed.model_invocable,
        user_invocable: parsed.user_invocable,
    })
}

/// 扫全量（**含非法项**）—— 给 `list_skills` 工具用。
///
/// 为什么不复用 [`catalog`]：那个**刻意**只给能用的（它进系统提示），
/// 而 `list_skills` 要的是"装了但有问题"的那些 —— 静默过滤掉正是 AIH-038 要防的事
/// （否则表现为"安装了 skill 但未生效"且没有提示）。
pub fn scan_all(
    builtin_root: &Path,
    user_root: &Path,
    disabled: &HashSet<String>,
) -> Vec<ScannedSkill> {
    scan(builtin_root, user_root)
        .into_iter()
        .map(|s| {
            // `validation_error` 现在就是 `parse_one` 里那条**原话**（以前这里是按 `valid`
            // 反推一句概括，而 `valid=false` 的原因有五种，概括出来的话与界面看到的不一样）
            let error = s.validation_error.clone().unwrap_or_default();
            let enabled = !disabled.contains(s.entry.name.as_str());
            ScannedSkill {
                name: s.entry.name,
                description: s.entry.description,
                when_to_use: s.entry.when_to_use,
                source: s.source,
                valid: s.valid,
                enabled,
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
    /// 是否已停用（停用只影响对话，文件保留）
    pub enabled: bool,
    /// 非空 = 不合格，内容是给人看的原因
    pub error: String,
}

/// bundle 的根目录：只有 `<name>/SKILL.md` 这种形态才有附带文件；平铺 `.md` 返回 `None`。
pub fn bundle_root(body: &SkillBody) -> Option<&Path> {
    let file_name = body.path.file_name()?.to_string_lossy().to_lowercase();
    if file_name != "skill.md" {
        return None;
    }
    body.path.parent()
}

/// bundle 里的附带文件（相对路径 + 字节数，按路径升序，最多 200 条）。
///
/// 给 `load_skill` 的结构化结果用：模型得先知道 `references/` 里有什么，
/// 才谈得上按需读。平铺技能没有 bundle，返回空表。
pub fn bundle_files(body: &SkillBody) -> Vec<(String, i64)> {
    let Some(root) = bundle_root(body) else {
        return Vec::new();
    };
    fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<(String, i64)>) {
        if depth > 4 || out.len() >= 200 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if out.len() >= 200 {
                return;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                walk(root, &path, depth + 1, out);
            } else if meta.is_file() {
                let Ok(rel) = path.strip_prefix(root) else {
                    continue;
                };
                let rel = rel.to_string_lossy().replace('\\', "/");
                // 正文自己不算"附带文件"
                if rel.eq_ignore_ascii_case("SKILL.md") {
                    continue;
                }
                out.push((rel, meta.len() as i64));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, 1, &mut out);
    out
}

/// `Files.isDirectory(path, NOFOLLOW_LINKS)`：**不看符号链接指向什么**。
fn is_dir_no_follow(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.is_dir())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests;
