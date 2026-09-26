//! 角色卡（P4 第 46 步，Kotlin 侧是 O8 / 2026-09-25 那一轮）。
//!
//! 一张卡 = 一个角色的**规范 tag 栈 + 来源 + 配哪条 LoRA / 权重**。它要解决的现场问题：
//! "AI 每次写角色都靠印象编 tag" —— 库里明明有 `shinku_(nte)` 的完整 tag 栈（prompts 261/310），
//! 模型却把 shinku 当成 iroi 写了两版废图（总纲 §10.3 的 O7/O8）。
//!
//! 逐条对照 `CharacterCardRepo.kt` + `CharacterCardRoutes.kt`。三条纪律一条不改：
//!
//!  1. **同名 = 同一张卡**：`normalized` 唯一索引 + `ON CONFLICT(normalized) DO UPDATE`
//!     （**不用** `INSERT OR REPLACE` —— R3：它内部是 DELETE+INSERT，会点亮 DELETE 触发器
//!     往 `_sync_log` 写**假的 delete**）。本表当前**不在同步范围**（总纲 §3.3），
//!     但这条习惯要留着：哪天进同步范围时不必回头改。
//!  2. **来源必须如实**：`source` / `tagsVerifiedAt` 是"我从 danbooru 查到的"还是"我按印象写的"
//!     的唯一凭据。核对时间由**真去查 danbooru 的那条路**写，不由模型自称。
//!  3. **写不进去就报错，不许静默截断**：字段超长 / 名字为空一律 400 + 人话
//!     （与 `MemoryStore` 的取向一致 —— 悄悄丢用户的话更糟）。
//!
//! # 端点（与 Kotlin 逐字同形）
//!
//! ```text
//!  GET    /api/ai/character-cards           列表（?q= 关键字 / ?limit=）
//!  POST   /api/ai/character-cards           新建 → 201 + 卡
//!  GET    /api/ai/character-cards/{id}      详情
//!  PUT    /api/ai/character-cards/{id}      编辑（全量替换，界面弹窗用）
//!  DELETE /api/ai/character-cards/{id}      删除 → {"deleted": id}
//! ```
//!
//! **没有 `/save` 那条路**：AI 的"按名字合并保存"直接走 [`save`]（不绕 HTTP），
//! 所以这里只提供界面的全量增删改查。
//!
//! ⚠ 差异记录（与第 20 步 `media/{id}/prompt` 同一类）：Kotlin 的 404 body 会被
//! `Application.kt` 的 StatusPages 改写成 `{"error":"接口不存在: <路径>"}`，
//! 而 Rust 给的是原话 `{"error":"角色卡 5 不存在"}`。Dart 只判 2xx / 404 状态码，
//! **行为无差异**，而且 Rust 那句更有信息量 —— 不改，但记在这里免得下次有人当回归去"修"。

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::iso_time::iso_time;
use crate::repo::WriteError;
use crate::routes::{ApiError, AppState, Id};

/// 名字上限（与 `tags.name` 的 96 同一档）。
pub const MAX_NAME_CHARS: usize = 96;
/// 一张卡的 tag 栈上限：100 个 tag 已经远超任何角色（再多是模型跑飞了）。
pub const MAX_TAGS: usize = 100;
/// 单个 tag 上限。
pub const MAX_TAG_CHARS: usize = 96;
/// 备注上限（够写一整段"这个角色怎么画才对"）。
pub const MAX_NOTES_CHARS: usize = 4000;
/// 搜索一次最多回几张卡（给模型看的，不是分页）。
pub const MAX_SEARCH: usize = 50;

const BASE_SELECT: &str = "SELECT id, name, normalized, name_en, aliases, character_tag, series_tag, tags, negative, \
     lora_name, lora_weight, lora_trigger, checkpoint, reference_media_id, source, \
     tags_verified_at, notes, created_at, updated_at FROM character_cards";

/// 一张角色卡（`CharacterCardDto`）。
///
/// ⚠ 可空字段一律 `skip_serializing_if`（Kotlin 的 `explicitNulls = false`）——
/// 少了它界面上会多出一堆 `null` 键，`tests/shape_lint.rs` 也会红。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterCardDto {
    pub id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_en: Option<String>,
    pub aliases: Vec<String>,
    /// danbooru 角色 tag（形如 `shinku_(nte)`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character_tag: Option<String>,
    /// 作品 tag（形如 `neverness_to_everness`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_tag: Option<String>,
    /// 规范 tag 栈（外观 / 服装 / 动作基线），一个 tag 一项
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lora_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lora_weight: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lora_trigger: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
    /// 参考图（画廊里的产物 id）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_media_id: Option<i64>,
    /// 来源：链接 / "用户口述" / "danbooru" …（**如实写**，不许编）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags_verified_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// 写入用的一份卡（`CharacterCardInput`）—— 界面弹窗与 AI 工具共用的形状。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CharacterCardInput {
    pub name: String,
    pub name_en: Option<String>,
    pub aliases: Vec<String>,
    pub character_tag: Option<String>,
    pub series_tag: Option<String>,
    pub tags: Vec<String>,
    pub negative: Option<String>,
    pub lora_name: Option<String>,
    pub lora_weight: Option<f64>,
    pub lora_trigger: Option<String>,
    pub checkpoint: Option<String>,
    pub reference_media_id: Option<i64>,
    pub source: Option<String>,
    pub tags_verified_at: Option<String>,
    pub notes: Option<String>,
}

impl CharacterCardInput {
    /// 从模型的工具参数造一份（键名与 schema 一致，缺的一律 `None` / 空）。
    pub fn from_arguments(arguments: &Value) -> Self {
        serde_json::from_value(arguments.clone()).unwrap_or_default()
    }
}

pub fn normalize(name: &str) -> String {
    name.trim().to_lowercase()
}

/// 把一串 tag 拆成一项一项。
///
/// 分隔符认三种：英文逗号、中文逗号、换行（模型三种都会写；`derpibooru` 风格的空格分隔
/// **不认** —— 那会把 `red eyes` 拆成两个不存在的 tag，比不拆更糟）。
/// 去重保序（同一批里重复的 tag 没有意义，还会稀释权重）。
pub fn split_tags(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for piece in text.split([',', '，', '\n', '\r', ';', '；']) {
        let tag = normalize_tag(piece);
        if tag.is_empty() {
            continue;
        }
        if !out.contains(&tag) {
            out.push(tag);
        }
    }
    out
}

/// 单个 tag 的清洗：压空白、去掉列表符号（`- ` / `* ` / 行首数字序号）。
pub fn normalize_tag(raw: &str) -> String {
    let mut tag: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    for prefix in ["- ", "* "] {
        if let Some(rest) = tag.strip_prefix(prefix) {
            tag = rest.trim().to_string();
        }
    }
    // 行首数字序号：`1. ` / `1、` / `1) `
    let trimmed = tag.trim_start();
    let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let rest = &trimmed[digits.len()..];
        for sep in [".", "、", ")"] {
            if let Some(after) = rest.strip_prefix(sep) {
                tag = after.trim_start().to_string();
                break;
            }
        }
    }
    take_chars(tag.trim(), MAX_TAG_CHARS)
}

fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let tag = normalize_tag(tag);
        if tag.is_empty() || out.contains(&tag) {
            continue;
        }
        out.push(tag);
        if out.len() >= MAX_TAGS {
            break;
        }
    }
    out
}

fn clean_aliases(aliases: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for alias in aliases {
        let alias = alias.trim().to_string();
        if alias.is_empty() || out.contains(&alias) {
            continue;
        }
        out.push(alias);
        if out.len() >= 20 {
            break;
        }
    }
    out
}

fn take_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// 字段超长**报错**而不是截断（第 3 条纪律）。
fn require_fits(value: Option<&String>, limit: usize, what: &str) -> Result<Option<String>, WriteError> {
    let Some(raw) = value else { return Ok(None) };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let len = trimmed.chars().count();
    if len > limit {
        return Err(WriteError::Validation(format!(
            "{what} 超出上限（{len} > {limit} 字符）；请拆成多条，不要指望被悄悄截断"
        )));
    }
    Ok(Some(trimmed.to_string()))
}

fn decode_list(raw: Option<String>) -> Vec<String> {
    let Some(raw) = raw else { return Vec::new() };
    if raw.trim().is_empty() {
        return Vec::new();
    }
    serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default()
}

/// 空列表存成 NULL（与 Kotlin `encodeList` 一致：库里不写 `[]`）。
fn encode_list(value: &[String]) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        serde_json::to_string(value).ok()
    }
}

fn row_to_card(row: &Row<'_>) -> rusqlite::Result<CharacterCardDto> {
    let created: Option<String> = row.get("created_at")?;
    let updated: Option<String> = row.get("updated_at")?;
    let verified: Option<String> = row.get("tags_verified_at")?;
    Ok(CharacterCardDto {
        id: row.get("id")?,
        name: row.get::<_, Option<String>>("name")?.unwrap_or_default(),
        name_en: row.get("name_en")?,
        aliases: decode_list(row.get("aliases")?),
        character_tag: row.get("character_tag")?,
        series_tag: row.get("series_tag")?,
        tags: decode_list(row.get("tags")?),
        negative: row.get("negative")?,
        lora_name: row.get("lora_name")?,
        lora_weight: row.get("lora_weight")?,
        lora_trigger: row.get("lora_trigger")?,
        checkpoint: row.get("checkpoint")?,
        reference_media_id: row.get("reference_media_id")?,
        source: row.get("source")?,
        tags_verified_at: iso_time(verified.as_deref()),
        notes: row.get("notes")?,
        created_at: iso_time(created.as_deref()),
        updated_at: iso_time(updated.as_deref()),
    })
}

// ---------------------------------------------------------------------------
//  读
// ---------------------------------------------------------------------------

pub fn list(conn: &Connection, q: Option<&str>, limit: usize) -> rusqlite::Result<Vec<CharacterCardDto>> {
    let keyword = q.map(|s| s.trim()).filter(|s| !s.is_empty());
    let limit = limit.clamp(1, 500);
    let mut out = Vec::new();
    match keyword {
        Some(q) => {
            let like = format!("%{q}%");
            let sql = format!(
                "{BASE_SELECT} WHERE (name LIKE ?1 OR normalized LIKE ?1 OR name_en LIKE ?1 OR aliases LIKE ?1 \
                 OR character_tag LIKE ?1 OR series_tag LIKE ?1 OR tags LIKE ?1 OR lora_name LIKE ?1 \
                 OR lora_trigger LIKE ?1 OR notes LIKE ?1) ORDER BY updated_at DESC, id DESC LIMIT ?2"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![like, limit as i64], row_to_card)?;
            for row in rows {
                out.push(row?);
            }
        }
        None => {
            let sql = format!("{BASE_SELECT} ORDER BY updated_at DESC, id DESC LIMIT ?1");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![limit as i64], row_to_card)?;
            for row in rows {
                out.push(row?);
            }
        }
    }
    Ok(out)
}

pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Option<CharacterCardDto>> {
    conn.query_row(&format!("{BASE_SELECT} WHERE id = ?1"), params![id], row_to_card)
        .optional()
}

pub fn get_by_normalized(conn: &Connection, name: &str) -> rusqlite::Result<Option<CharacterCardDto>> {
    conn.query_row(
        &format!("{BASE_SELECT} WHERE normalized = ?1"),
        params![normalize(name)],
        row_to_card,
    )
    .optional()
}

// ---------------------------------------------------------------------------
//  写
// ---------------------------------------------------------------------------

fn validate(input: &CharacterCardInput) -> Result<String, WriteError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(WriteError::Validation("角色名不能为空".into()));
    }
    let len = name.chars().count();
    if len > MAX_NAME_CHARS {
        return Err(WriteError::Validation(format!(
            "角色名太长（{len} > {MAX_NAME_CHARS} 字符）"
        )));
    }
    Ok(name.to_string())
}

/// 参考图必须真的存在（外键会拦，但那样报出来的是 500 + SQL 原文，看不懂）。
fn require_media_exists(conn: &Connection, media_id: Option<i64>) -> Result<(), WriteError> {
    let Some(media_id) = media_id else { return Ok(()) };
    let exists: Option<i64> = conn
        .query_row("SELECT 1 FROM media_assets WHERE id = ?1", params![media_id], |row| row.get(0))
        .optional()?;
    if exists.is_none() {
        return Err(WriteError::Validation(format!(
            "参考图（产物 {media_id}）不存在；先确认它在画廊里，再填 referenceMediaId"
        )));
    }
    Ok(())
}

pub fn create(conn: &Connection, input: &CharacterCardInput) -> Result<i64, WriteError> {
    let name = validate(input)?;
    require_media_exists(conn, input.reference_media_id)?;
    conn.execute(
        "INSERT INTO character_cards (name, normalized, name_en, aliases, character_tag, series_tag, \
         tags, negative, lora_name, lora_weight, lora_trigger, checkpoint, reference_media_id, \
         source, tags_verified_at, notes) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        params![
            name,
            normalize(&name),
            require_fits(input.name_en.as_ref(), MAX_NAME_CHARS, "英文名")?,
            encode_list(&clean_aliases(&input.aliases)),
            require_fits(input.character_tag.as_ref(), MAX_TAG_CHARS, "角色 tag")?,
            require_fits(input.series_tag.as_ref(), MAX_TAG_CHARS, "作品 tag")?,
            encode_list(&clean_tags(&input.tags)),
            require_fits(input.negative.as_ref(), MAX_NOTES_CHARS, "负面词")?,
            require_fits(input.lora_name.as_ref(), 256, "LoRA 文件名")?,
            input.lora_weight,
            require_fits(input.lora_trigger.as_ref(), MAX_NOTES_CHARS, "LoRA 触发词")?,
            require_fits(input.checkpoint.as_ref(), 256, "底模名")?,
            input.reference_media_id,
            require_fits(input.source.as_ref(), 500, "来源")?,
            require_fits(input.tags_verified_at.as_ref(), 64, "核对时间")?,
            require_fits(input.notes.as_ref(), MAX_NOTES_CHARS, "备注")?,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 全量替换（界面上的"编辑保存"用它：弹窗里所有字段都在）。
pub fn update(conn: &Connection, id: i64, input: &CharacterCardInput) -> Result<bool, WriteError> {
    let name = validate(input)?;
    require_media_exists(conn, input.reference_media_id)?;
    let changed = conn.execute(
        "UPDATE character_cards SET name = ?1, normalized = ?2, name_en = ?3, aliases = ?4, \
         character_tag = ?5, series_tag = ?6, tags = ?7, negative = ?8, lora_name = ?9, \
         lora_weight = ?10, lora_trigger = ?11, checkpoint = ?12, reference_media_id = ?13, \
         source = ?14, tags_verified_at = ?15, notes = ?16 WHERE id = ?17",
        params![
            name,
            normalize(&name),
            require_fits(input.name_en.as_ref(), MAX_NAME_CHARS, "英文名")?,
            encode_list(&clean_aliases(&input.aliases)),
            require_fits(input.character_tag.as_ref(), MAX_TAG_CHARS, "角色 tag")?,
            require_fits(input.series_tag.as_ref(), MAX_TAG_CHARS, "作品 tag")?,
            encode_list(&clean_tags(&input.tags)),
            require_fits(input.negative.as_ref(), MAX_NOTES_CHARS, "负面词")?,
            require_fits(input.lora_name.as_ref(), 256, "LoRA 文件名")?,
            input.lora_weight,
            require_fits(input.lora_trigger.as_ref(), MAX_NOTES_CHARS, "LoRA 触发词")?,
            require_fits(input.checkpoint.as_ref(), 256, "底模名")?,
            input.reference_media_id,
            require_fits(input.source.as_ref(), 500, "来源")?,
            require_fits(input.tags_verified_at.as_ref(), 64, "核对时间")?,
            require_fits(input.notes.as_ref(), MAX_NOTES_CHARS, "备注")?,
            id,
        ],
    )?;
    Ok(changed > 0)
}

pub fn delete(conn: &Connection, id: i64) -> Result<bool, WriteError> {
    Ok(conn.execute("DELETE FROM character_cards WHERE id = ?1", params![id])? > 0)
}

/// 合并（**AI 保存角色卡时用这个**，不是全量替换）。
///
/// 为什么：模型一次只会说一件事（"这个角色的 LoRA 是 iroi-anima"），
/// 全量替换会把它上一轮存下来的 tag 栈、来源、别名一起抹掉 —— 那是**悄悄丢用户的数据**。
/// 所以：`incoming` 里为空的字段一律保留 `existing` 的值；只有明确给了值才覆盖。
///
/// `tags` 是**追加去重**（不是覆盖）：模型常常一次补一两个 tag（"再想想，她还戴蝴蝶结"），
/// 覆盖同样会丢东西。要去掉某个 tag 就整串重给一次并在界面里删 —— 这条写在工具描述里。
pub fn merge(existing: Option<&CharacterCardDto>, incoming: CharacterCardInput) -> CharacterCardInput {
    let Some(existing) = existing else { return incoming };
    let pick = |new: Option<String>, old: Option<String>| match new {
        Some(v) if !v.trim().is_empty() => Some(v),
        _ => old,
    };
    let mut aliases = existing.aliases.clone();
    for alias in incoming.aliases.iter() {
        let alias = alias.trim().to_string();
        if !alias.is_empty() && !aliases.contains(&alias) {
            aliases.push(alias);
        }
    }
    let mut tags = existing.tags.clone();
    for tag in incoming.tags.iter() {
        let tag = normalize_tag(tag);
        if !tag.is_empty() && !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    let name = if incoming.name.trim().is_empty() {
        existing.name.clone()
    } else {
        incoming.name.trim().to_string()
    };
    CharacterCardInput {
        name,
        name_en: pick(incoming.name_en, existing.name_en.clone()),
        aliases,
        character_tag: pick(incoming.character_tag, existing.character_tag.clone()),
        series_tag: pick(incoming.series_tag, existing.series_tag.clone()),
        tags,
        negative: pick(incoming.negative, existing.negative.clone()),
        lora_name: pick(incoming.lora_name, existing.lora_name.clone()),
        lora_weight: incoming.lora_weight.or(existing.lora_weight),
        lora_trigger: pick(incoming.lora_trigger, existing.lora_trigger.clone()),
        checkpoint: pick(incoming.checkpoint, existing.checkpoint.clone()),
        reference_media_id: incoming.reference_media_id.or(existing.reference_media_id),
        source: pick(incoming.source, existing.source.clone()),
        tags_verified_at: pick(incoming.tags_verified_at, existing.tags_verified_at.clone()),
        notes: pick(incoming.notes, existing.notes.clone()),
    }
}

/// 按名字幂等保存：没有就建、有就合并。返回 `(id, created)`。
pub fn save(conn: &Connection, input: CharacterCardInput) -> Result<(i64, bool), WriteError> {
    let name = validate(&input)?;
    let existing = get_by_normalized(conn, &name)?;
    match existing {
        None => Ok((create(conn, &input)?, true)),
        Some(existing) => {
            let merged = merge(Some(&existing), input);
            update(conn, existing.id, &merged)?;
            Ok((existing.id, false))
        }
    }
}

// ---------------------------------------------------------------------------
//  AI 工具那两条（P4 第 46 步的尾巴）
// ---------------------------------------------------------------------------

/// 资料库类查询一次最多回几条（与 Kotlin `MAX_LIBRARY_RESULTS` 一致）。
pub const MAX_LIBRARY_RESULTS: i64 = 20;

fn clip(text: &str, max: usize) -> String {
    let len = text.chars().count();
    if len <= max {
        text.to_string()
    } else {
        format!("{}…（已截断，共 {len} 字符）", take_chars(text, max))
    }
}

/// 角色卡在工具结果里的正文形状（**逐字**照抄 Kotlin `renderCard`：模型要能一眼看出"卡里已经有什么"）。
pub fn render_card(card: &CharacterCardDto) -> String {
    let mut out = String::new();
    let name_en = card
        .name_en
        .as_ref()
        .map(|v| format!("（{v}）"))
        .unwrap_or_default();
    out.push_str(&format!("  #{}「{}」{}\n", card.id, card.name, name_en));
    if let Some(tag) = card.character_tag.as_ref() {
        out.push_str(&format!("  角色 tag: {tag}\n"));
    }
    if let Some(tag) = card.series_tag.as_ref() {
        out.push_str(&format!("  作品 tag: {tag}\n"));
    }
    if !card.aliases.is_empty() {
        out.push_str(&format!("  别名: {}\n", card.aliases.join("、")));
    }
    if !card.tags.is_empty() {
        out.push_str(&format!(
            "  tag 栈({}): {}\n",
            card.tags.len(),
            card.tags.iter().take(40).cloned().collect::<Vec<_>>().join("、")
        ));
    }
    let mut lora: Vec<String> = Vec::new();
    if let Some(name) = card.lora_name.as_ref() {
        lora.push(name.clone());
    }
    if let Some(weight) = card.lora_weight {
        lora.push(format!("权重 {weight}"));
    }
    if let Some(trigger) = card.lora_trigger.as_ref() {
        lora.push(format!("触发词 {trigger}"));
    }
    if !lora.is_empty() {
        out.push_str(&format!("  LoRA: {}\n", lora.join(" · ")));
    }
    if let Some(checkpoint) = card.checkpoint.as_ref() {
        out.push_str(&format!("  底模: {checkpoint}\n"));
    }
    if let Some(media) = card.reference_media_id {
        out.push_str(&format!("  参考图: 产物 {media}\n"));
    }
    if let Some(negative) = card.negative.as_ref() {
        out.push_str(&format!("  负面: {}\n", clip(negative, 300)));
    }
    if let Some(source) = card.source.as_ref() {
        out.push_str(&format!("  来源: {source}\n"));
    }
    out.push_str(&format!(
        "  danbooru 核对: {}\n",
        card.tags_verified_at
            .as_deref()
            .unwrap_or("（没有——这些 tag 未经 danbooru 核对）")
    ));
    if let Some(notes) = card.notes.as_ref() {
        out.push_str(&format!("  备注: {}\n", clip(notes, 300)));
    }
    out.push_str(&format!("  更新于 {}\n", card.updated_at.as_deref().unwrap_or("?")));
    out
}

/// 角色卡的结构化形状（**逐字**照抄 Kotlin `cardJson`：可空字段整键省略）。
pub fn card_json(card: &CharacterCardDto) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), Value::Number(card.id.into()));
    out.insert("name".into(), Value::String(card.name.clone()));
    let mut put = |key: &str, value: Value| {
        out.insert(key.to_string(), value);
    };
    if let Some(v) = card.name_en.as_ref() {
        put("nameEn", Value::String(v.clone()));
    }
    put(
        "aliases",
        Value::Array(card.aliases.iter().cloned().map(Value::String).collect()),
    );
    if let Some(v) = card.character_tag.as_ref() {
        put("characterTag", Value::String(v.clone()));
    }
    if let Some(v) = card.series_tag.as_ref() {
        put("seriesTag", Value::String(v.clone()));
    }
    put("tags", Value::Array(card.tags.iter().cloned().map(Value::String).collect()));
    if let Some(v) = card.negative.as_ref() {
        put("negative", Value::String(v.clone()));
    }
    if let Some(v) = card.lora_name.as_ref() {
        put("loraName", Value::String(v.clone()));
    }
    if let Some(v) = card.lora_weight {
        put("loraWeight", serde_json::json!(v));
    }
    if let Some(v) = card.lora_trigger.as_ref() {
        put("loraTrigger", Value::String(v.clone()));
    }
    if let Some(v) = card.checkpoint.as_ref() {
        put("checkpoint", Value::String(v.clone()));
    }
    if let Some(v) = card.reference_media_id {
        put("referenceMediaId", Value::Number(v.into()));
    }
    if let Some(v) = card.source.as_ref() {
        put("source", Value::String(v.clone()));
    }
    if let Some(v) = card.tags_verified_at.as_ref() {
        put("tagsVerifiedAt", Value::String(v.clone()));
    }
    if let Some(v) = card.notes.as_ref() {
        put("notes", Value::String(v.clone()));
    }
    if let Some(v) = card.created_at.as_ref() {
        put("createdAt", Value::String(v.clone()));
    }
    if let Some(v) = card.updated_at.as_ref() {
        put("updatedAt", Value::String(v.clone()));
    }
    Value::Object(out)
}

fn arg_str(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn arg_i64(arguments: &Value, key: &str) -> Option<i64> {
    arguments.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    })
}

fn arg_f64(arguments: &Value, key: &str) -> Option<f64> {
    arguments.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    })
}

/// `search_character_cards`：查本机存下来的角色卡（写角色之前先查这里）。
pub fn tool_search(state: &AppState, arguments: &Value) -> Result<crate::tools::ToolOutcome, crate::tools::ToolFailure> {
    let query = arg_str(arguments, "query");
    let limit = arg_i64(arguments, "limit").unwrap_or(5).clamp(1, MAX_LIBRARY_RESULTS);
    let cards = state
        .with(|conn| list(conn, query.as_deref(), limit as usize))
        .map_err(api_failure)?;
    if cards.is_empty() {
        return Err(crate::tools::ToolFailure::new(
            "NOT_FOUND",
            format!(
                "本机没有匹配的角色卡（关键字：{}）。下一步：`danbooru_lookup(query=\"角色名\")` 核对 tag 与同现 tag，\
                 再用 `save_character_card` 存成卡；**不要**凭印象编 tag。",
                query.unwrap_or_else(|| "（空）".to_string())
            ),
        ));
    }
    let mut text = format!("角色卡：命中 {} 张\n", cards.len());
    for card in cards.iter() {
        text.push('\n');
        text.push_str(&render_card(card));
    }
    let payload = serde_json::json!({
        "count": cards.len(),
        "items": cards.iter().map(card_json).collect::<Vec<_>>(),
    });
    Ok(crate::tools::ToolOutcome::new(text, payload))
}

/// `save_character_card`：把一个角色的规范 tag 栈 / LoRA / 权重 / 别名 / 来源存成角色卡。
///
/// ⚠ **`tagsVerifiedAt` 只认"这一轮真查过 danbooru 的那个 tag"**（Kotlin 的
/// `ToolContext.danbooruVerified`，Rust 侧是 [`crate::tools::ToolSession`] 里那个集合）——
/// 判据是**事实**（`danbooru_lookup` 真的成功返回过），不是模型的自称。
/// 没查过就如实留空并在正文里说明，**绝不许**自己编一个时间。
pub fn tool_save(
    state: &AppState,
    arguments: &Value,
    session: &crate::tools::ToolSession,
) -> Result<crate::tools::ToolOutcome, crate::tools::ToolFailure> {
    let name = arg_str(arguments, "name")
        .ok_or_else(|| crate::tools::ToolFailure::new("INVALID_ARGUMENT", "缺少参数 name"))?;
    let character_tag = arg_str(arguments, "characterTag");
    let verified_key = normalize_tag(character_tag.as_deref().unwrap_or(&name));
    let verified = session.danbooru_verified(&verified_key);
    let input = CharacterCardInput {
        name: name.clone(),
        name_en: arg_str(arguments, "nameEn"),
        aliases: split_tags(arg_str(arguments, "aliases").unwrap_or_default().as_str()),
        character_tag,
        series_tag: arg_str(arguments, "seriesTag"),
        tags: split_tags(arg_str(arguments, "tags").unwrap_or_default().as_str()),
        negative: arg_str(arguments, "negative"),
        lora_name: arg_str(arguments, "loraName"),
        lora_weight: arg_f64(arguments, "loraWeight"),
        lora_trigger: arg_str(arguments, "loraTrigger"),
        checkpoint: arg_str(arguments, "checkpoint"),
        reference_media_id: arg_i64(arguments, "referenceMediaId"),
        source: arg_str(arguments, "source"),
        tags_verified_at: if verified { Some(crate::danbooru::now_iso()) } else { None },
        notes: arg_str(arguments, "notes"),
    };
    let (id, created) = state.write(|conn| save(conn, input)).map_err(api_failure)?;
    let card = state
        .with(|conn| get(conn, id))
        .map_err(api_failure)?
        .ok_or_else(|| {
            crate::tools::ToolFailure::new(
                "TOOL_ERROR",
                format!("角色卡已保存但读不回来（id={id}）；这是本机的 bug，请如实告诉用户"),
            )
        })?;
    let mut text = format!(
        "已保存角色卡 #{id}「{}」（{}）\n",
        card.name,
        if created { "新建" } else { "合并更新" }
    );
    text.push_str(&render_card(&card));
    text.push('\n');
    if verified {
        text.push_str(&format!(
            "danbooru 核对时间已记上：{}\n",
            card.tags_verified_at.as_deref().unwrap_or("")
        ));
    } else {
        text.push_str(&format!(
            "⚠ 这张卡**没有** danbooru 核对记录（本轮没有查过 `{verified_key}`）—— \
             想标上核对时间就先 `danbooru_lookup`，再回来保存一次；**不要**自己编时间。"
        ));
    }
    Ok(crate::tools::ToolOutcome::new(text, card_json(&card)))
}

/// 把 `state.with` / `state.write` 折出来的 `ApiError` 翻成 `ToolFailure`。
///
/// ⚠ **一次工具调用失败不该打断整个 Run**（Kotlin `ToolRegistry.invoke` 的同一条纪律），
/// 而 `state.write` 那层已经把仓储的 `WriteError` 折成 `ApiError`（`write_routes.rs` 的 `From`），
/// 所以这里按**状态码**分档：400 = 参数不合法、404 = 没这条，其余一律 `TOOL_ERROR`。
fn api_failure(error: crate::routes::ApiError) -> crate::tools::ToolFailure {
    use axum::http::StatusCode;
    let code = match error.status {
        StatusCode::BAD_REQUEST => "INVALID_ARGUMENT",
        StatusCode::NOT_FOUND => "NOT_FOUND",
        _ => "TOOL_ERROR",
    };
    crate::tools::ToolFailure::new(code, error.human().to_string())
}

// ---------------------------------------------------------------------------
//  路由
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    // ⚠ 这里必须写 `axum::routing::get` 的全路径：本模块的仓储函数就叫
    // `get` / `list` / `create` / `update` / `delete`（与 Kotlin 同名同义），
    // 直接 `use axum::routing::get` 会被同名遮蔽，报的错还很绕。
    Router::new()
        .route(
            "/api/ai/character-cards",
            axum::routing::get(list_cards).post(create_card),
        )
        .route(
            "/api/ai/character-cards/{id}",
            axum::routing::get(get_card).put(update_card).delete(delete_card),
        )
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    q: Option<String>,
    limit: Option<String>,
}

async fn list_cards(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<CharacterCardDto>>, ApiError> {
    // Kotlin `p.int("limit", MAX_SEARCH)`：解析不出来就用默认值（不报错）。
    let limit = query
        .limit
        .as_deref()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(MAX_SEARCH);
    let cards = state.with(|conn| list(conn, query.q.as_deref(), limit))?;
    Ok(Json(cards))
}

async fn create_card(
    State(state): State<AppState>,
    Json(input): Json<CharacterCardInput>,
) -> Result<(StatusCode, Json<CharacterCardDto>), ApiError> {
    let id = state.write(|conn| create(conn, &input))?;
    let card = state
        .with(|conn| get(conn, id))?
        .ok_or_else(|| ApiError::internal("创建失败"))?;
    Ok((StatusCode::CREATED, Json(card)))
}

async fn get_card(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<Json<CharacterCardDto>, ApiError> {
    state
        .with(|conn| get(conn, id))?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("角色卡 {id} 不存在")))
}

async fn update_card(
    State(state): State<AppState>,
    Id(id): Id,
    Json(input): Json<CharacterCardInput>,
) -> Result<Json<CharacterCardDto>, ApiError> {
    let updated = state.write(|conn| update(conn, id, &input))?;
    if !updated {
        return Err(ApiError::not_found(format!("角色卡 {id} 不存在")));
    }
    state
        .with(|conn| get(conn, id))?
        .map(Json)
        .ok_or_else(|| ApiError::internal("读取失败"))
}

async fn delete_card(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<Json<Value>, ApiError> {
    let deleted = state.write(|conn| delete(conn, id))?;
    if !deleted {
        return Err(ApiError::not_found(format!("角色卡 {id} 不存在")));
    }
    let mut out = Map::new();
    out.insert("deleted".into(), Value::Number(id.into()));
    Ok(Json(Value::Object(out)))
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = crate::db::open_memory().unwrap();
        // 内存库不带 DDL：真 schema 从 `db/schema.sqlite.sql` 灌（与 `tests/api_test.rs` 同一条路；
        // 用真 DDL 而不是测试里另写一份建表语句 —— 否则 `normalized` 唯一索引这类
        // "承重的约束"在用例里根本不存在，测出来的绿是假的）。
        let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("db")
            .join("schema.sqlite.sql");
        let schema = std::fs::read_to_string(&schema_path)
            .unwrap_or_else(|e| panic!("读不到 {}：{e}", schema_path.display()));
        conn.execute_batch(&schema).expect("建 schema");
        crate::db::assert_ready(&conn).unwrap();
        conn
    }

    fn input(name: &str, tags: &[&str]) -> CharacterCardInput {
        CharacterCardInput {
            name: name.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn 建卡读回形状与_kotlin_一致() {
        let conn = conn();
        let mut card = input("Shinku", &["shinku_(nte)", "pink streaks"]);
        card.character_tag = Some("shinku_(nte)".into());
        card.lora_name = Some("shinku-anima".into());
        card.lora_weight = Some(0.85);
        card.source = Some("danbooru".into());
        let id = create(&conn, &card).unwrap();

        let got = get(&conn, id).unwrap().unwrap();
        assert_eq!(got.name, "Shinku");
        assert_eq!(got.tags, vec!["shinku_(nte)".to_string(), "pink streaks".to_string()]);
        assert_eq!(got.lora_weight, Some(0.85));
        assert_eq!(got.character_tag.as_deref(), Some("shinku_(nte)"));
        assert!(got.created_at.is_some() && got.updated_at.is_some(), "时间列要有");
        // 可空字段不出现（Kotlin `explicitNulls = false`）
        let json = serde_json::to_value(&got).unwrap();
        assert!(!json.as_object().unwrap().contains_key("negative"));
        assert!(!json.as_object().unwrap().contains_key("seriesTag"));
        assert!(json.as_object().unwrap().contains_key("aliases"), "空数组照样给键");
    }

    #[test]
    fn 同名是同一张卡_合并保存不抹掉上一轮() {
        let conn = conn();
        let mut first = input("Iroi", &["green hair", "sheep horns"]);
        first.source = Some("civitai 触发词".into());
        first.aliases = vec!["iroi_(nte)".into()];
        let (id1, created1) = save(&conn, first).unwrap();
        assert!(created1);

        // 第二轮模型只说了一件事：LoRA 名
        let mut second = CharacterCardInput { name: "iroi".into(), ..Default::default() };
        second.lora_name = Some("iroi-anima".into());
        second.tags = vec!["white dress".into()];
        let (id2, created2) = save(&conn, second).unwrap();
        assert!(!created2, "同名必须走合并");
        assert_eq!(id1, id2);

        let got = get(&conn, id1).unwrap().unwrap();
        assert_eq!(got.lora_name.as_deref(), Some("iroi-anima"));
        assert_eq!(
            got.tags,
            vec!["green hair".to_string(), "sheep horns".to_string(), "white dress".to_string()],
            "tags 是追加去重，不许覆盖"
        );
        assert_eq!(got.source.as_deref(), Some("civitai 触发词"), "来源不能被空值抹掉");
        assert_eq!(got.aliases, vec!["iroi_(nte)".to_string()]);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM character_cards", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "同名不许攒出第二行");
    }

    #[test]
    fn 全量替换会清掉没给的字段() {
        let conn = conn();
        let mut card = input("Shinku", &["a"]);
        card.notes = Some("原来有备注".into());
        let id = create(&conn, &card).unwrap();
        let replaced = input("Shinku", &["b"]);
        assert!(update(&conn, id, &replaced).unwrap());
        let got = get(&conn, id).unwrap().unwrap();
        assert_eq!(got.tags, vec!["b".to_string()]);
        assert!(got.notes.is_none(), "PUT 是全量替换（界面弹窗里所有字段都在）");
    }

    #[test]
    fn 字段超长报错而不是截断() {
        let conn = conn();
        let mut card = input("Shinku", &[]);
        card.notes = Some("x".repeat(MAX_NOTES_CHARS + 1));
        let err = create(&conn, &card).unwrap_err();
        let message = format!("{err:?}");
        assert!(message.contains("备注 超出上限"), "{message}");
        assert!(message.contains("不要指望被悄悄截断"), "{message}");
        // 名字为空 / 太长同样是 400 那一档
        assert!(create(&conn, &CharacterCardInput::default()).is_err());
        let long_name = "名".repeat(MAX_NAME_CHARS + 1);
        assert!(create(&conn, &input(&long_name, &[])).is_err());
    }

    #[test]
    fn 参考图不存在时报人话而不是外键_500() {
        let conn = conn();
        let mut card = input("Shinku", &[]);
        card.reference_media_id = Some(999_999);
        let err = create(&conn, &card).unwrap_err();
        let message = format!("{err:?}");
        assert!(message.contains("产物 999999") && message.contains("不存在"), "{message}");
    }

    #[test]
    fn tag_清洗与拆分的三条纪律() {
        // 三种分隔符都认，空格分隔**不认**
        assert_eq!(
            split_tags("red eyes, blue hair，white dress\nsheep horns;green hair；pink"),
            vec!["red eyes", "blue hair", "white dress", "sheep horns", "green hair", "pink"]
        );
        // 去重保序
        assert_eq!(split_tags("a, b, a"), vec!["a", "b"]);
        // 列表符号与序号
        assert_eq!(normalize_tag("- pink streaks"), "pink streaks");
        assert_eq!(normalize_tag("2. sheep horns"), "sheep horns");
        assert_eq!(normalize_tag("3、green hair"), "green hair");
        assert_eq!(normalize_tag("  很多    空格 "), "很多 空格");
        // 上限：100 个 tag、96 字符一个
        let many: Vec<String> = (0..150).map(|i| format!("tag{i}")).collect();
        assert_eq!(clean_tags(&many).len(), MAX_TAGS);
        assert_eq!(normalize_tag(&"x".repeat(200)).chars().count(), MAX_TAG_CHARS);
    }

    #[test]
    fn 搜索命中名字别名与_tag_栈() {
        let conn = conn();
        let mut card = input("Shinku", &["pink streaks", "purple horns"]);
        card.aliases = vec!["真红".into()];
        card.notes = Some("neverness_to_everness".into());
        create(&conn, &card).unwrap();
        create(&conn, &input("Iroi", &["green hair"])).unwrap();

        assert_eq!(list(&conn, None, MAX_SEARCH).unwrap().len(), 2, "q 为空 = 列全部");
        assert_eq!(list(&conn, Some("shin"), MAX_SEARCH).unwrap().len(), 1);
        assert_eq!(list(&conn, Some("真红"), MAX_SEARCH).unwrap().len(), 1, "别名要命中");
        assert_eq!(list(&conn, Some("purple horns"), MAX_SEARCH).unwrap().len(), 1, "tag 栈要命中");
        assert_eq!(list(&conn, Some("everness"), MAX_SEARCH).unwrap().len(), 1, "备注要命中");
        assert!(list(&conn, Some("不存在的东西"), MAX_SEARCH).unwrap().is_empty());
    }

    #[test]
    fn 工具_搜索查不到时不许编并且要说下一步() {
        let state = test_state();
        let err = tool_search(&state, &serde_json::json!({"query": "shinku"})).unwrap_err();
        assert_eq!(err.code, "NOT_FOUND");
        assert!(err.message.contains("shinku"), "{}", err.message);
        assert!(err.message.contains("danbooru_lookup"), "{}", err.message);
        assert!(err.message.contains("不要**因为") || err.message.contains("不要"), "{}", err.message);
    }

    #[test]
    fn 工具_保存是合并保存且没查过_danbooru_就如实留空() {
        let state = test_state();
        let session = crate::tools::ToolSession::default();
        let first = tool_save(
            &state,
            &serde_json::json!({
                "name": "Shinku",
                "characterTag": "shinku_(nte)",
                "tags": "pink streaks, purple horns",
                "source": "danbooru"
            }),
            &session,
        )
        .unwrap();
        assert!(first.content.contains("已保存角色卡 #1「Shinku」（新建）"), "{}", first.content);
        assert!(first.content.contains("角色 tag: shinku_(nte)"), "{}", first.content);
        assert!(first.content.contains("tag 栈(2)"), "{}", first.content);
        // 这一轮**没查过** danbooru：核对时间必须如实留空 + 明确提醒
        assert!(first.content.contains("**没有** danbooru 核对记录"), "{}", first.content);
        assert!(
            !first.payload.as_object().unwrap().contains_key("tagsVerifiedAt"),
            "没查过就不许出现核对时间键：{}",
            first.payload
        );

        // 第二轮只说 LoRA —— 合并保存，tag 栈与来源都不许被抹掉
        let second = tool_save(
            &state,
            &serde_json::json!({ "name": "shinku", "loraName": "shinku-anima", "loraWeight": 0.85 }),
            &session,
        )
        .unwrap();
        assert!(second.content.contains("（合并更新）"), "{}", second.content);
        assert_eq!(second.payload["id"], 1);
        assert_eq!(
            second.payload["tags"],
            serde_json::json!(["pink streaks", "purple horns"])
        );
        assert_eq!(second.payload["source"], "danbooru");
        assert_eq!(second.payload["loraName"], "shinku-anima");
        let count: i64 = state
            .with(|conn| conn.query_row("SELECT COUNT(*) FROM character_cards", [], |r| r.get(0)))
            .unwrap();
        assert_eq!(count, 1, "同名不许攒出第二行");
    }

    /// **核对时间是事实、不是自述**：只有这一轮 `danbooru_lookup` 真的查过那个 tag
    /// （`ToolSession.danbooru_verified` 里有归一之后的键）才写得进去。
    ///
    /// 两条方向都要盯：查过 → 必须写；没查过（哪怕模型把 `source` 写成 `danbooru`）→ 必须留空。
    #[test]
    fn 工具_查过_danbooru_才写得进核对时间() {
        let state = test_state();
        let mut session = crate::tools::ToolSession::default();
        // 查的是 zh 写法 → 归一之后同一个键（`Shinku (NTE)` 与 `shinku_(nte)` 是一回事）
        session.mark_danbooru_verified("Shinku (NTE)");
        let saved = tool_save(
            &state,
            &serde_json::json!({ "name": "shinku", "characterTag": "shinku_(nte)" }),
            &session,
        )
        .unwrap();
        assert!(saved.content.contains("danbooru 核对时间已记上"), "{}", saved.content);
        let verified_at = saved.payload["tagsVerifiedAt"].as_str().unwrap_or("");
        assert!(verified_at.len() >= 20 && verified_at.ends_with('Z'), "核对时间要是 ISO：{verified_at}");

        // 另一个 tag 没查过 → 再存一次不许有核对时间（合并保存也不会把它带出来）
        let other = tool_save(
            &state,
            &serde_json::json!({ "name": "iroi", "characterTag": "iroi_(nte)", "source": "danbooru" }),
            &session,
        )
        .unwrap();
        assert!(other.content.contains("**没有** danbooru 核对记录"), "{}", other.content);
        assert!(!other.payload.as_object().unwrap().contains_key("tagsVerifiedAt"), "{}", other.payload);
    }

    #[test]
    fn 工具_缺名字与超长都是参数错误() {
        let state = test_state();
        let session = crate::tools::ToolSession::default();
        let err = tool_save(&state, &serde_json::json!({}), &session).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");
        assert!(err.message.contains("name"), "{}", err.message);

        let long = "x".repeat(MAX_NOTES_CHARS + 1);
        let err = tool_save(&state, &serde_json::json!({ "name": "x", "notes": long }), &session).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");
        assert!(err.message.contains("备注 超出上限"), "{}", err.message);
    }

    fn test_state() -> AppState {
        // 内存库要自己灌真 DDL（`AppState::new` 只包连接，不建表）
        let conn = conn();
        AppState::new(conn, ":memory:", std::env::temp_dir())
    }

    #[test]
    fn 删除与不存在的行为() {
        let conn = conn();
        let id = create(&conn, &input("Shinku", &[])).unwrap();
        assert!(delete(&conn, id).unwrap());
        assert!(!delete(&conn, id).unwrap(), "删第二次要如实说没删到");
        assert!(get(&conn, id).unwrap().is_none());
        assert!(update(&conn, id, &input("Shinku", &[])).unwrap() == false);
    }
}
