//! Skills 的**读侧**（run-kit 要把目录拼进系统提示，红线 R13：正文只在磁盘上）。
//!
//! 真源是磁盘（`<storage>\ai\skills` 与 `<项目根>\skills\builtin`），**刻意不做缓存**：
//! "AI 注册完下一次回复就能用、用户删掉立刻消失"是需求（AGENTS §10）。
//!
//! 这里只搬 run-kit 用得到的那一半：**扫描 + frontmatter 解析 + 校验**，
//! 拼出 `catalog()`（进提示词的那份）。**没搬**：`save()` / `autoRegister()` /
//! `read()`（正文加载）/ `digest` / `fileCount` —— 那些属于工具层（`register_skill` /
//! `delete_skill` / `load_skill`），等搬工具时再逐条对照。
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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
struct ParsedSkill {
    entry: SkillEntry,
    valid: bool,
    /// builtin / user（Run 的 Skills 快照要如实记下来源，AIH-047）
    source: &'static str,
    /// 正文 `sha256` 的**前 16 位十六进制**（与 Kotlin `SkillStore.sha256` 同一个口径：
    /// 对**文件原文**取哈希，不是对解析后的 body）
    digest: String,
}

/// `sha256(原文)` 的前 16 位（Kotlin `SkillStore.sha256` 的逐字对应物）。
fn short_digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let out = hasher.finalize();
    out.iter().map(|b| format!("{b:02x}")).collect::<String>().chars().take(16).collect()
}

const MAX_BODY_BYTES: usize = 256 * 1024;

fn parse_one(file: &Path, fallback_name: &str, source: &'static str) -> Option<ParsedSkill> {
    let raw = std::fs::read_to_string(file).ok()?;
    let (meta, body) = parse_front_matter(&raw);
    let name = meta
        .get("name")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback_name.to_string());

    let mut valid = true;
    if !valid_name(&name) {
        valid = false;
    }
    // frontmatter 的 name 与目录名不一致 → 非法（同一个 skill 两个名字，加载时会打架）
    if name != fallback_name {
        if let Some(parent) = file.parent() {
            if parent.file_name().map(|n| n.to_string_lossy() == fallback_name).unwrap_or(false) {
                valid = false;
            }
        }
    }
    let description = meta.get("description").cloned().unwrap_or_default();
    if description.trim().is_empty() || body.trim().is_empty() || body.len() > MAX_BODY_BYTES {
        valid = false;
    }

    Some(ParsedSkill {
        entry: SkillEntry {
            name,
            description,
            when_to_use: meta.get("whentouse").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            version: meta.get("version").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        },
        valid,
        source,
        digest: short_digest(&raw),
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
fn scan(builtin_root: &Path, user_root: &Path) -> Vec<ParsedSkill> {
    let builtin = scan_root(builtin_root, "builtin");
    let user = scan_root(user_root, "user");
    let mut out: Vec<ParsedSkill> = Vec::new();
    for (name, dto) in &builtin {
        match user.iter().find(|(n, _)| n == name) {
            // 用户那份胜出（`conflict` 只是提示，不影响可用性）
            Some((_, override_dto)) => out.push(override_dto.clone()),
            None => out.push(dto.clone()),
        }
    }
    for (name, dto) in &user {
        if !builtin.iter().any(|(n, _)| n == name) {
            out.push(dto.clone());
        }
    }
    out.sort_by(|a, b| a.entry.name.cmp(&b.entry.name));
    out
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
            // 先算错误再搬字段（`entry.when_to_use` 是 move，之后不能再借 `s`）
            let error = if s.valid { String::new() } else { validation_error(&s) };
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

/// 不合格的原因（与 Kotlin `parseOne` 里那几条 `fail(...)` 一一对应）。
fn validation_error(skill: &ParsedSkill) -> String {
    let name = &skill.entry.name;
    if !valid_name(name) {
        return "name 必须是 kebab-case（小写字母 / 数字 / 连字符，≤64）".to_string();
    }
    if skill.entry.description.trim().is_empty() {
        return "缺少 description".to_string();
    }
    "正文为空或超过 256KB".to_string()
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
        let dir = std::env::temp_dir().join(format!("comfyhub-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
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
        let dir = std::env::temp_dir().join(format!("comfyhub-skills-override-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
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
