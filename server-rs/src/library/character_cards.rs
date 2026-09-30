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

use crate::infra::iso_time::iso_time;
use crate::library::repo::WriteError;
use crate::web::routes::{ApiError, AppState, Id};

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

/// **同时启用的上限**（生图角色卡框架：单次生成最多 6 个）。
///
/// 超了要**报错**，不许静默只取前 6 个 —— 用户以为 7 个人都在画面里、出图少一个，
/// 那种"哪儿不对但说不上来"比一句错误难查得多。
pub const MAX_ENABLED: usize = 6;

/// 位置网格：5 行（A~E）× 5 列（1~5）。
pub const GRID_ROWS: [char; 5] = ['A', 'B', 'C', 'D', 'E'];
/// 位置网格的列号。
pub const GRID_COLS: [u8; 5] = [1, 2, 3, 4, 5];
/// 新建角色的默认位置（**完全居中**，框架里点名的那一格）。
pub const DEFAULT_POSITION: &str = "C3";

/// 位置是不是一个合法格子（`A1`~`E5`，大小写不敏感）。
pub fn is_grid_cell(text: &str) -> bool {
    let mut chars = text.trim().chars();
    let (Some(row), Some(col), None) = (chars.next(), chars.next(), chars.next()) else {
        return false;
    };
    let Some(col) = col.to_digit(10) else { return false };
    GRID_ROWS.contains(&row.to_ascii_uppercase()) && GRID_COLS.contains(&(col as u8))
}

/// 位置归一化：`None` / 空 / `auto` / `自动` → `None`（= 让 AI 自己摆）；
/// `c3` → `C3`；其余**报错并说清该填什么**（静默当自动就是"设了没用"）。
pub fn normalize_position(raw: Option<&String>) -> Result<Option<String>, WriteError> {
    let Some(text) = raw else { return Ok(None) };
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") || trimmed == "自动" {
        return Ok(None);
    }
    if !is_grid_cell(trimmed) {
        return Err(WriteError::Validation(format!(
            "position「{trimmed}」不合法：要么留空 = 自动（让 AI 自己摆），\
             要么给 5×5 网格里的一个格子（行 A~E、列 1~5，例如默认的 {DEFAULT_POSITION}）"
        )));
    }
    Ok(Some(trimmed.to_ascii_uppercase()))
}

/// 提示词 / 排除词的 **token 估算**（界面上那行「本 X · 总 Y/512」用的同一把尺子）。
///
/// ⚠ 是**估算**、不是真分词：这些模型走 CLIP 系分词器，Dart 与 Rust 两侧都没有它。
/// 尺子：ASCII 连续串按 `ceil(字符数 / 4)`（英文 tag 的经验值）、CJK 每字 1 个、
/// 空白与分隔符 0 —— 只在"离 512 还有多远"这件事上有意义，别拿它当精确值。
///
/// **前端不许再写一份**（AGENTS §7 的两份实现纪律）：要显示就调 `GET /api/characters/summary`。
pub fn estimate_tokens(text: &str) -> usize {
    let mut total = 0usize;
    let mut ascii_run = 0usize;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '\'' || ch == '-' || ch == '_' || ch == '.' {
            ascii_run += 1;
            continue;
        }
        // 非 ASCII 字母（中文 / 日文假名 / 汉字）按 1 个 token 算
        if !ch.is_whitespace() && !ch.is_ascii() {
            total += 1;
        }
        if ascii_run > 0 {
            total += ascii_run.div_ceil(4);
            ascii_run = 0;
        }
    }
    if ascii_run > 0 {
        total += ascii_run.div_ceil(4);
    }
    total
}

/// 一张卡的提示词 token 估算（tag 栈拼起来算）。
pub fn prompt_tokens_of(card: &CharacterCardDto) -> usize {
    estimate_tokens(&card.tags.join(", "))
}

/// 一张卡的排除词 token 估算。
pub fn negative_tokens_of(card: &CharacterCardDto) -> usize {
    card.negative.as_deref().map(estimate_tokens).unwrap_or(0)
}

const BASE_SELECT: &str = "SELECT id, name, normalized, name_en, aliases, character_tag, series_tag, tags, negative, \
     lora_name, lora_weight, lora_trigger, checkpoint, reference_media_id, source, \
     tags_verified_at, notes, position, enabled, created_at, updated_at FROM character_cards";

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
    /// 5×5 网格里的目标位置（`A1`~`E5`）；`None` = 自动（让 AI 摆）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    /// 是否参与本次生成（同时启用最多 [`MAX_ENABLED`] 个）
    pub enabled: bool,
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
    /// 目标位置（`A1`~`E5`）；`None` / `"auto"` / 空 = 自动
    pub position: Option<String>,
    /// `None` = **这次没提这件事**（合并时保留原值）；`Some(true/false)` = 明确要开/关。
    pub enabled: Option<bool>,
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
        position: row.get("position")?,
        enabled: row.get::<_, Option<i64>>("enabled")?.unwrap_or(0) != 0,
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

/// 现在**已经启用**了几张（`exclude` 是"正在改的这张"，它自己的旧值不算）。
fn enabled_count(conn: &Connection, exclude: Option<i64>) -> Result<usize, WriteError> {
    let count: i64 = match exclude {
        Some(id) => conn.query_row(
            "SELECT COUNT(*) FROM character_cards WHERE enabled = 1 AND id <> ?1",
            params![id],
            |row| row.get(0),
        )?,
        None => conn.query_row("SELECT COUNT(*) FROM character_cards WHERE enabled = 1", [], |row| {
            row.get(0)
        })?,
    };
    Ok(count.max(0) as usize)
}

/// 要启用时就检查 6 个上限（**报错而不是静默排掉**）。
fn require_enabled_room(conn: &Connection, enabled: Option<bool>, exclude: Option<i64>) -> Result<bool, WriteError> {
    let want = enabled.unwrap_or(false);
    if !want {
        return Ok(false);
    }
    let already = enabled_count(conn, exclude)?;
    if already >= MAX_ENABLED {
        return Err(WriteError::Validation(format!(
            "已经启用了 {already} 个角色（上限 {MAX_ENABLED} 个）：先关掉一个再启用这个。\
             没启用的角色**留在库里、不占 token**，不是删掉。"
        )));
    }
    Ok(true)
}

pub fn create(conn: &Connection, input: &CharacterCardInput) -> Result<i64, WriteError> {
    let name = validate(input)?;
    require_media_exists(conn, input.reference_media_id)?;
    let position = normalize_position(input.position.as_ref())?;
    let enabled = require_enabled_room(conn, input.enabled, None)?;
    conn.execute(
        "INSERT INTO character_cards (name, normalized, name_en, aliases, character_tag, series_tag, \
         tags, negative, lora_name, lora_weight, lora_trigger, checkpoint, reference_media_id, \
         source, tags_verified_at, notes, position, enabled) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
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
            position,
            enabled as i64,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 全量替换（界面上的"编辑保存"用它：弹窗里所有字段都在）。
pub fn update(conn: &Connection, id: i64, input: &CharacterCardInput) -> Result<bool, WriteError> {
    let name = validate(input)?;
    require_media_exists(conn, input.reference_media_id)?;
    let position = normalize_position(input.position.as_ref())?;
    let enabled = require_enabled_room(conn, input.enabled, Some(id))?;
    let changed = conn.execute(
        "UPDATE character_cards SET name = ?1, normalized = ?2, name_en = ?3, aliases = ?4, \
         character_tag = ?5, series_tag = ?6, tags = ?7, negative = ?8, lora_name = ?9, \
         lora_weight = ?10, lora_trigger = ?11, checkpoint = ?12, reference_media_id = ?13, \
         source = ?14, tags_verified_at = ?15, notes = ?16, position = ?17, enabled = ?18 WHERE id = ?19",
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
            position,
            enabled as i64,
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
        // 位置：这次没说就保留原值；说了 `"auto"` 这类**明确要清空**的写法照旧归一化
        position: match incoming.position.as_ref().map(|v| v.trim()).filter(|v| !v.is_empty()) {
            Some(_) => incoming.position.clone(),
            None => existing.position.clone(),
        },
        // 启用开关：`None` = 这次没提（保留原值，别把用户开着的关掉）
        enabled: incoming.enabled.or(Some(existing.enabled)),
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
    // 生图角色卡框架那两件：位置（5×5 网格）与"参不参与本次生成"
    out.push_str(&format!(
        "  位置: {}（5×5 网格 A1~E5；自动 = 让 AI 自己摆）\n",
        card.position.as_deref().unwrap_or("自动")
    ));
    out.push_str(&format!(
        "  {}（同时启用上限 {MAX_ENABLED} 个；未启用不占 token）\n",
        if card.enabled { "✓ 已启用：参与本次生成" } else { "未启用：留在库里待命" }
    ));
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
    put("enabled", Value::Bool(card.enabled));
    if let Some(v) = card.position.as_ref() {
        put("position", Value::String(v.clone()));
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
pub fn tool_search(state: &AppState, arguments: &Value) -> Result<crate::tools::tools::ToolOutcome, crate::tools::tools::ToolFailure> {
    let query = arg_str(arguments, "query");
    let limit = arg_i64(arguments, "limit").unwrap_or(5).clamp(1, MAX_LIBRARY_RESULTS);
    let cards = state
        .with(|conn| list(conn, query.as_deref(), limit as usize))
        .map_err(api_failure)?;
    if cards.is_empty() {
        return Err(crate::tools::tools::ToolFailure::new(
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
    Ok(crate::tools::tools::ToolOutcome::new(text, payload))
}

/// `save_character_card`：把一个角色的规范 tag 栈 / LoRA / 权重 / 别名 / 来源存成角色卡。
///
/// ⚠ **`tagsVerifiedAt` 只认"这一轮真查过 danbooru 的那个 tag"**（Kotlin 的
/// `ToolContext.danbooruVerified`，Rust 侧是 [`crate::tools::tools::ToolSession`] 里那个集合）——
/// 判据是**事实**（`danbooru_lookup` 真的成功返回过），不是模型的自称。
/// 没查过就如实留空并在正文里说明，**绝不许**自己编一个时间。
pub fn tool_save(
    state: &AppState,
    arguments: &Value,
    session: &crate::tools::tools::ToolSession,
) -> Result<crate::tools::tools::ToolOutcome, crate::tools::tools::ToolFailure> {
    let name = arg_str(arguments, "name")
        .ok_or_else(|| crate::tools::tools::ToolFailure::new("INVALID_ARGUMENT", "缺少参数 name"))?;
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
        tags_verified_at: if verified { Some(crate::tools::danbooru::now_iso()) } else { None },
        notes: arg_str(arguments, "notes"),
        // 位置 / 启用：模型没提就 `None`（`merge` 会保留原值，别把用户开着的关掉）
        position: arg_str(arguments, "position"),
        enabled: arguments.get("enabled").and_then(Value::as_bool),
    };
    let (id, created) = state.write(|conn| save(conn, input)).map_err(api_failure)?;
    let card = state
        .with(|conn| get(conn, id))
        .map_err(api_failure)?
        .ok_or_else(|| {
            crate::tools::tools::ToolFailure::new(
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
    Ok(crate::tools::tools::ToolOutcome::new(text, card_json(&card)))
}

/// 把 `state.with` / `state.write` 折出来的 `ApiError` 翻成 `ToolFailure`。
///
/// ⚠ **一次工具调用失败不该打断整个 Run**（Kotlin `ToolRegistry.invoke` 的同一条纪律），
/// 而 `state.write` 那层已经把仓储的 `WriteError` 折成 `ApiError`（`write_routes.rs` 的 `From`），
/// 所以这里按**状态码**分档：400 = 参数不合法、404 = 没这条，其余一律 `TOOL_ERROR`。
fn api_failure(error: crate::web::routes::ApiError) -> crate::tools::tools::ToolFailure {
    use axum::http::StatusCode;
    let code = match error.status {
        StatusCode::BAD_REQUEST => "INVALID_ARGUMENT",
        StatusCode::NOT_FOUND => "NOT_FOUND",
        _ => "TOOL_ERROR",
    };
    crate::tools::tools::ToolFailure::new(code, error.human().to_string())
}

// ---------------------------------------------------------------------------
//  路由
// ---------------------------------------------------------------------------

/// 主提示词 / 排除词的 token 上限（生图角色卡框架里的那个 512）。
pub const PROMPT_LIMIT: usize = 512;

/// 界面顶部那行「本 X · 总 Y/512」需要的全部数字（**服务端算，前端不重算**）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterSummary {
    /// 现在启用了几张
    pub enabled_count: usize,
    pub max_enabled: usize,
    /// **只算启用的那些**（未启用不占 token —— 框架原话）
    pub prompt_tokens: usize,
    pub negative_tokens: usize,
    pub prompt_limit: usize,
    pub negative_limit: usize,
    /// 新建角色的默认位置（界面上的 5×5 网格拿它当选中格）
    pub default_position: String,
    /// 网格的行 / 列（界面照它画 5×5，别自己写死）
    pub grid_rows: Vec<String>,
    pub grid_cols: Vec<u8>,
}

/// 算一次汇总（启用数 + token 口径）。
pub fn summary(conn: &Connection) -> rusqlite::Result<CharacterSummary> {
    let sql = format!("{BASE_SELECT} WHERE enabled = 1 ORDER BY position IS NULL, position, id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_card)?;
    let mut enabled_count = 0usize;
    let mut prompt_tokens = 0usize;
    let mut negative_tokens = 0usize;
    for row in rows {
        let card = row?;
        enabled_count += 1;
        prompt_tokens += prompt_tokens_of(&card);
        negative_tokens += negative_tokens_of(&card);
    }
    Ok(CharacterSummary {
        enabled_count,
        max_enabled: MAX_ENABLED,
        prompt_tokens,
        negative_tokens,
        prompt_limit: PROMPT_LIMIT,
        negative_limit: PROMPT_LIMIT,
        default_position: DEFAULT_POSITION.to_string(),
        grid_rows: GRID_ROWS.iter().map(|c| c.to_string()).collect(),
        grid_cols: GRID_COLS.to_vec(),
    })
}

pub fn router() -> Router<AppState> {
    // ⚠ 这里必须写 `axum::routing::get` 的全路径：本模块的仓储函数就叫
    // `get` / `list` / `create` / `update` / `delete`（与 Kotlin 同名同义），
    // 直接 `use axum::routing::get` 会被同名遮蔽，报的错还很绕。
    Router::new()
        .route(
            "/api/ai/character-cards",
            axum::routing::get(list_cards).post(create_card),
        )
        // ⚠ 这条**必须在 `{id}` 之前**声明：matchit 里静态段本来就比参数段优先，
        //   排前面只是把这件事写在明处（`summary` 不会被当成一个 id）。
        .route(
            "/api/ai/character-cards/summary",
            axum::routing::get(summary_cards),
        )
        .route(
            "/api/ai/character-cards/{id}",
            axum::routing::get(get_card).put(update_card).delete(delete_card),
        )
}

/// `GET /api/ai/character-cards/summary` —— 启用数与 token 口径。
async fn summary_cards(State(state): State<AppState>) -> Result<Json<CharacterSummary>, ApiError> {
    let out = state.with(summary)?;
    Ok(Json(out))
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
mod tests;
