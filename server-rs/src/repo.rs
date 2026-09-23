//! 写仓储（Rust 版）—— 逐条对照 Kotlin 的 `TagRepo.kt` / `PromptRepo.kt`。
//!
//! # 三条从 Kotlin 侧抄过来的语义（别"顺手优化"）
//!
//! 1. **标签按 `normalized`（trim + 小写）去重**，但**保留首次写入时的展示名大小写**；
//!    改名时如果撞上别的标签 → 400（`WriteError::Conflict`），不是静默合并。
//! 2. **`use_count` 采用"改动后整体重算"**（[`refresh_use_counts`]）：
//!    数据量是个人级，重算保证计数永远正确、也没有增量维护的 bug 面。
//!    ⚠ SQL 里**不能给 tags 起别名**（`UPDATE tags t SET t.use_count = …`）——
//!    MySQL 认、SQLite 报 `near "t": syntax error`（Kotlin 侧踩过，注释留在 `TagRepo.kt:139`）。
//! 3. **`title` 为空时自动起名**（[`auto_title`]：正文第一段逗号前那截，最多 60 字）。
//!
//! # 与同步的关系（**不用额外做什么**）
//!
//! `db/schema.sqlite.sql` 给每张被同步的表挂了 3 个触发器往 `_sync_log` 写行，
//! 所以这里的写操作**自动**成为手机能看到的下行变更 —— 用例里有一条专门盯这个
//! （"写完之后 `_sync_log` 里有 upsert / delete"）。
//!
//! ⚠ 唯一要自己守的是 **R3**：被同步的表禁止 `INSERT OR REPLACE`（它内部是 DELETE+INSERT，
//! 会点亮 DELETE 触发器写**假的 delete**）。所以 upsert 一律 `ON CONFLICT … DO UPDATE`。

use crate::inputs::{LoraRefInput, PromptInput, TagInput};
use crate::models::{PromptDto, TagDto};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::collections::BTreeMap;

/// 写入失败的分类（routes 层据它给状态码）。
#[derive(Debug)]
pub enum WriteError {
    /// 400：参数不合法（空名字 / 内容为空）
    Validation(String),
    /// 400：撞上已有数据（标签重名）
    Conflict(String),
    /// 404：目标不存在
    NotFound,
    /// 500：数据库错误
    Db(rusqlite::Error),
}

impl From<rusqlite::Error> for WriteError {
    fn from(e: rusqlite::Error) -> Self {
        WriteError::Db(e)
    }
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Validation(m) | WriteError::Conflict(m) => write!(f, "{m}"),
            WriteError::NotFound => write!(f, "目标不存在"),
            WriteError::Db(e) => write!(f, "数据库错误：{e}"),
        }
    }
}

pub const KINDS: [&str; 4] = ["IMAGE", "VIDEO", "AUDIO", "MIXED"];

/// 标签的归一化（trim + 小写）—— 与 Kotlin `TagRepo.normalize` 同义。
pub fn normalize_tag(name: &str) -> String {
    name.trim().to_lowercase()
}

/// 按字符截断（Kotlin 的 `take(n)` 是**字符**不是字节；中文按字节截会切碎）。
fn take_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 空串当 `NULL`（Kotlin 到处写 `.takeIf { it.isNotBlank() }`）。
fn blank_to_none(value: Option<&String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

// ---------------------------------------------------------------------------
//  标签
// ---------------------------------------------------------------------------

/// 确保标签存在并返回 id（已存在则返回原 id，**保留原有展示名**）。
pub fn ensure_tag(
    conn: &Connection,
    name: &str,
    category: Option<&str>,
    color: Option<&str>,
) -> Result<Option<i64>, WriteError> {
    let display = take_chars(name.trim(), 96);
    if display.is_empty() {
        return Ok(None);
    }
    let normalized = take_chars(&normalize_tag(&display), 96);
    let existing: Option<i64> = conn
        .query_row("SELECT id FROM tags WHERE normalized = ?1", params![normalized], |row| row.get(0))
        .optional()?;
    if let Some(id) = existing {
        return Ok(Some(id));
    }
    conn.execute(
        "INSERT INTO tags (name, normalized, category, color) VALUES (?1, ?2, ?3, ?4)",
        params![
            display,
            normalized,
            category.map(|c| c.trim()).filter(|c| !c.is_empty()),
            color.map(|c| c.trim()).filter(|c| !c.is_empty())
        ],
    )?;
    Ok(Some(conn.last_insert_rowid()))
}

/// 改标签（重命名 / 分类 / 颜色 / 描述）。撞上已有 normalized → [`WriteError::Conflict`]。
pub fn update_tag(conn: &Connection, id: i64, input: &TagInput) -> Result<bool, WriteError> {
    let display = take_chars(input.name.trim(), 96);
    if display.is_empty() {
        return Err(WriteError::Validation("标签名不能为空".into()));
    }
    let normalized = take_chars(&normalize_tag(&display), 96);
    let dup: Option<i64> = conn
        .query_row(
            "SELECT id FROM tags WHERE normalized = ?1 AND id <> ?2",
            params![normalized, id],
            |row| row.get(0),
        )
        .optional()?;
    if dup.is_some() {
        return Err(WriteError::Conflict(format!("标签「{display}」已存在")));
    }
    let affected = conn.execute(
        "UPDATE tags SET name = ?1, normalized = ?2, category = ?3, color = ?4, description = ?5 WHERE id = ?6",
        params![
            display,
            normalized,
            blank_to_none(input.category.as_ref()),
            blank_to_none(input.color.as_ref()),
            blank_to_none(input.description.as_ref()),
            id
        ],
    )?;
    Ok(affected > 0)
}

/// 删标签；关联行靠 `ON DELETE CASCADE` 一起走（R1 生效才有这个效果）。
pub fn delete_tag(conn: &Connection, id: i64) -> Result<bool, WriteError> {
    Ok(conn.execute("DELETE FROM tags WHERE id = ?1", params![id])? > 0)
}

/// **整体重算** `use_count`（见文件头第 2 条：SQL 里不许给 tags 起别名）。
pub fn refresh_use_counts(conn: &Connection) -> Result<(), WriteError> {
    conn.execute(
        "UPDATE tags SET use_count = (
             (SELECT COUNT(*) FROM prompt_tags pt WHERE pt.tag_id = tags.id)
           + (SELECT COUNT(*) FROM media_tags  mt WHERE mt.tag_id = tags.id)
         )",
        [],
    )?;
    Ok(())
}

pub fn add_prompt_tags(conn: &Connection, prompt_id: i64, names: &[String]) -> Result<(), WriteError> {
    let mut seen: Vec<i64> = Vec::new();
    for name in names {
        if let Some(id) = ensure_tag(conn, name, None, None)? {
            if !seen.contains(&id) {
                seen.push(id);
            }
        }
    }
    for tag_id in seen {
        conn.execute(
            "INSERT OR IGNORE INTO prompt_tags (prompt_id, tag_id) VALUES (?1, ?2)",
            params![prompt_id, tag_id],
        )?;
    }
    Ok(())
}

pub fn replace_prompt_tags(conn: &Connection, prompt_id: i64, names: &[String]) -> Result<(), WriteError> {
    conn.execute("DELETE FROM prompt_tags WHERE prompt_id = ?1", params![prompt_id])?;
    add_prompt_tags(conn, prompt_id, names)
}

pub fn remove_prompt_tag(conn: &Connection, prompt_id: i64, tag_id: i64) -> Result<(), WriteError> {
    conn.execute(
        "DELETE FROM prompt_tags WHERE prompt_id = ?1 AND tag_id = ?2",
        params![prompt_id, tag_id],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
//  提示词
// ---------------------------------------------------------------------------

/// 正文为空时的自动命名（第一行、逗号前那截、最多 60 字）。
pub fn auto_title(positive: &str) -> String {
    let first_line = positive.trim().lines().next().unwrap_or("");
    let head = first_line.split(',').next().unwrap_or("").trim();
    let title = take_chars(head, 60);
    if title.is_empty() {
        "未命名提示词".to_string()
    } else {
        title
    }
}

/// `loras` 列（JSON 数组文本）。
fn encode_loras(loras: &[LoraRefInput]) -> Option<String> {
    if loras.is_empty() {
        return None;
    }
    let value = serde_json::json!(loras
        .iter()
        .map(|l| serde_json::json!({ "name": l.name, "weight": l.weight }))
        .collect::<Vec<_>>());
    Some(value.to_string())
}

/// `extra_params` 列（JSON 对象文本）。
fn encode_extra(extra: &BTreeMap<String, String>) -> Option<String> {
    if extra.is_empty() {
        return None;
    }
    let value = serde_json::json!(extra);
    Some(value.to_string())
}

fn normalized_kind(kind: &str) -> String {
    let upper = kind.trim().to_uppercase();
    if KINDS.contains(&upper.as_str()) {
        upper
    } else {
        "IMAGE".to_string()
    }
}

/// 建提示词，返回新 id。**正文与标题都为空 → 400**（与 Kotlin `require(...)` 同义）。
pub fn create_prompt(conn: &Connection, input: &PromptInput) -> Result<i64, WriteError> {
    if input.positive_prompt.trim().is_empty() && input.title.trim().is_empty() {
        return Err(WriteError::Validation("提示词内容不能为空".into()));
    }
    let title = if input.title.trim().is_empty() {
        auto_title(&input.positive_prompt)
    } else {
        take_chars(input.title.trim(), 255)
    };
    conn.execute(
        "INSERT INTO prompts
           (title, kind, positive_prompt, negative_prompt, checkpoint, loras, sampler, scheduler,
            steps, cfg_scale, seed, width, height, batch_size, extra_params, notes, favorite)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
        params![
            title,
            normalized_kind(&input.kind),
            input.positive_prompt,
            blank_to_none(input.negative_prompt.as_ref()),
            blank_to_none(input.checkpoint.as_ref()),
            encode_loras(&input.loras),
            blank_to_none(input.sampler.as_ref()),
            blank_to_none(input.scheduler.as_ref()),
            input.steps,
            input.cfg_scale,
            input.seed,
            input.width,
            input.height,
            input.batch_size,
            encode_extra(&input.extra_params),
            blank_to_none(input.notes.as_ref()),
            if input.favorite { 1 } else { 0 },
        ],
    )?;
    let id = conn.last_insert_rowid();
    if !input.tags.is_empty() {
        add_prompt_tags(conn, id, &input.tags)?;
    }
    refresh_use_counts(conn)?;
    Ok(id)
}

/// 全量更新（含标签整体替换）；返回 `false` = 这条不存在。
pub fn update_prompt(conn: &Connection, id: i64, input: &PromptInput) -> Result<bool, WriteError> {
    let title = if input.title.trim().is_empty() {
        auto_title(&input.positive_prompt)
    } else {
        take_chars(input.title.trim(), 255)
    };
    let affected = conn.execute(
        "UPDATE prompts SET
           title = ?1, kind = ?2, positive_prompt = ?3, negative_prompt = ?4, checkpoint = ?5,
           loras = ?6, sampler = ?7, scheduler = ?8, steps = ?9, cfg_scale = ?10, seed = ?11,
           width = ?12, height = ?13, batch_size = ?14, extra_params = ?15, notes = ?16, favorite = ?17
         WHERE id = ?18",
        params![
            title,
            normalized_kind(&input.kind),
            input.positive_prompt,
            blank_to_none(input.negative_prompt.as_ref()),
            blank_to_none(input.checkpoint.as_ref()),
            encode_loras(&input.loras),
            blank_to_none(input.sampler.as_ref()),
            blank_to_none(input.scheduler.as_ref()),
            input.steps,
            input.cfg_scale,
            input.seed,
            input.width,
            input.height,
            input.batch_size,
            encode_extra(&input.extra_params),
            blank_to_none(input.notes.as_ref()),
            if input.favorite { 1 } else { 0 },
            id
        ],
    )?;
    if affected > 0 {
        replace_prompt_tags(conn, id, &input.tags)?;
        refresh_use_counts(conn)?;
    }
    Ok(affected > 0)
}

pub fn set_favorite(conn: &Connection, id: i64, favorite: bool) -> Result<bool, WriteError> {
    let affected = conn.execute(
        "UPDATE prompts SET favorite = ?1 WHERE id = ?2",
        params![if favorite { 1 } else { 0 }, id],
    )?;
    Ok(affected > 0)
}

/// 删提示词：关联产物**保留但解除关联**（FK `ON DELETE SET NULL`，R1 生效才有）。
pub fn delete_prompt(conn: &Connection, id: i64) -> Result<bool, WriteError> {
    let ok = conn.execute("DELETE FROM prompts WHERE id = ?1", params![id])? > 0;
    if ok {
        refresh_use_counts(conn)?;
    }
    Ok(ok)
}

/// 复制一条（标题加「 副本」后缀，收藏状态不复制 —— 与 Kotlin 一致）。
pub fn duplicate_prompt(conn: &Connection, id: i64, source: &PromptWithTags) -> Result<i64, WriteError> {
    let input = PromptInput {
        title: format!("{} 副本", source.prompt.title),
        kind: source.prompt.kind.clone(),
        positive_prompt: source.prompt.positive_prompt.clone(),
        negative_prompt: source.prompt.negative_prompt.clone(),
        checkpoint: source.prompt.checkpoint.clone(),
        loras: source
            .prompt
            .loras
            .iter()
            .map(|l| LoraRefInput { name: l.name.clone(), weight: l.weight })
            .collect(),
        sampler: source.prompt.sampler.clone(),
        scheduler: source.prompt.scheduler.clone(),
        steps: source.prompt.steps,
        cfg_scale: source.prompt.cfg_scale,
        seed: source.prompt.seed,
        width: source.prompt.width,
        height: source.prompt.height,
        batch_size: source.prompt.batch_size,
        extra_params: source
            .prompt
            .extra_params
            .as_object()
            .map(|obj| {
                obj.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        notes: source.prompt.notes.clone(),
        favorite: false,
        tags: source.tags.iter().map(|t| t.name.clone()).collect(),
    };
    let _ = id;
    create_prompt(conn, &input)
}

/// 复制时要用的"完整源行"（DTO + 它的标签）。
pub struct PromptWithTags {
    pub prompt: PromptDto,
    pub tags: Vec<TagDto>,
}

/// 便捷：在一个事务里执行（把 R2 的 `BEGIN IMMEDIATE` 留在 `db` 模块，别在这里重写）。
pub fn in_tx<T>(
    conn: &mut Connection,
    f: impl FnOnce(&Transaction<'_>) -> Result<T, WriteError>,
) -> Result<T, WriteError> {
    crate::db::tx_immediate(conn, f)
}
