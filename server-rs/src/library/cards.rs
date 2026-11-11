//! 卡片库：**角色卡**与**画风卡**（2026-10-21）。
//!
//! 两种卡是**同等地位**的两类知识：
//!  * 角色卡管"画的是谁"—— 一套人物的外观 / 服饰基线；
//!  * 画风卡管"画成什么样"—— 画师 / 媒介 / 上色 / 光线 / 质感。
//!
//! 每张卡只有**一版正面提示词** `prompt`：整段自然语言，给自然语言模型 / 人读。
//!
//! 约束：**不再保留 Danbooru tag 版**。卡片中的一句描述比 tag 栈
//! 更耐修改（更换底模、tag 体系都无需重写），确需 tag 版时**当场再转换一版**
//! 就行。老库里 `tag_prompt` 那一列的内容由 `infra::db` 的迁移**转成自然语言后灌进
//! `prompt`**（已有自然语言版的卡一个字都不动），随后那一列再没人读。
//!
//! 正面提示词为空时工具会**明确提醒**，不许假装卡已经写好了。
//!
//! 一张卡还可以挂若干条**示例产物**（`character_card_examples` /
//! `style_card_examples`）。它们是"这套设定 / 这个画风长什么样"的样本：
//! 界面画成缩略图，AI 工具里用 `#mediaId「标题」` 的形式回给模型。
//! 示例只允许来自 `media_assets`（外键 + `ON DELETE CASCADE`），
//! 不许编一个不存在的 id。
//!
//! # 端点
//!
//! ```text
//!  GET    /api/ai/character-cards            列表（?q= / ?limit=）
//!  POST   /api/ai/character-cards            新建 → 201 + 卡
//!  GET    /api/ai/character-cards/{id}       详情
//!  PUT    /api/ai/character-cards/{id}       编辑（全量替换，界面弹窗用）
//!  DELETE /api/ai/character-cards/{id}       删除 → {"deleted": id}
//!  GET    /api/ai/character-cards/{id}/examples
//!  POST   /api/ai/character-cards/{id}/examples  追加（{"mediaIds":[...]}）
//!  PUT    /api/ai/character-cards/{id}/examples  全量替换（界面"管理示例"用）
//!  DELETE /api/ai/character-cards/{id}/examples/{mediaId}
//!
//!  style-cards 那五条与之逐字同形。
//! ```
//!
//! 三条纪律（沿用角色卡那一轮）：
//!  1. **同名 = 同一张卡**（`normalized` 唯一索引；`save` 走"先查再合并"，
//!     不用 `INSERT OR REPLACE` —— R3：它会点亮 DELETE 触发器写假 delete）。
//!  2. **AI 那条路是"合并保存"**：只有这次真给了值的字段才覆盖，其余保持原样。
//!  3. **写不进去就报错**：字段超长 / 名字为空 / 示例超限 / 示例不存在一律 400 + 人话。
//!
//! ⚠ 本模块由原来的 `character_cards.rs` 泛化而来。`character_cards.rs` /
//! `style_cards.rs` 只是两个很薄的 facade（给工具与路由一个稳定的名字），
//! **仓储只有这一份实现** —— 两份实现必然会在"合并 / 校验 / 示例"上漂移。

use axum::{
    extract::{Path, Query, State},
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
/// 正面提示词的上限。角色卡的自然语言版要求 600+ token（约 2400+ 字符），
/// 所以这里给到 8000 —— **超了报错，不截断**。
pub const MAX_PROMPT_CHARS: usize = 8000;
/// 一张卡最多挂多少条示例产物。不是分页，是防"一次把整个画廊塞进一张卡"。
pub const MAX_EXAMPLES: usize = 24;
/// 搜索一次最多回几张卡（给模型看的，不是分页）。
pub const MAX_SEARCH: usize = 50;

/// 资料库类查询一次最多回几条（与 Kotlin `MAX_LIBRARY_RESULTS` 一致）。
pub const MAX_LIBRARY_RESULTS: i64 = 20;

/// 卡片种类。表名与文案的唯一真源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    Character,
    Style,
}

impl CardKind {
    pub const ALL: [CardKind; 2] = [CardKind::Character, CardKind::Style];

    pub fn table(self) -> &'static str {
        match self {
            CardKind::Character => "character_cards",
            CardKind::Style => "style_cards",
        }
    }

    pub fn example_table(self) -> &'static str {
        match self {
            CardKind::Character => "character_card_examples",
            CardKind::Style => "style_card_examples",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            CardKind::Character => "character",
            CardKind::Style => "style",
        }
    }

    /// 界面显示的名称（标题 / 按钮 / 错误文案均取自此处）。
    pub fn label(self) -> &'static str {
        match self {
            CardKind::Character => "角色卡",
            CardKind::Style => "画风卡",
        }
    }

    pub fn name_label(self) -> &'static str {
        match self {
            CardKind::Character => "角色名",
            CardKind::Style => "画风名",
        }
    }

    /// AI 工具名（`save_character_card` / `save_style_card`）。
    pub fn save_tool(self) -> &'static str {
        match self {
            CardKind::Character => "save_character_card",
            CardKind::Style => "save_style_card",
        }
    }

    pub fn search_tool(self) -> &'static str {
        match self {
            CardKind::Character => "search_character_cards",
            CardKind::Style => "search_style_cards",
        }
    }
}

const BASE_COLUMNS: &str = "id, name, prompt, negative, created_at, updated_at";

/// 一张卡（`CharacterCardDto` / `StyleCardDto` 的公共形状）。
///
/// `examples` **总是出现**（空数组也要有）：界面据此决定画不画"添加示例"那一行，
/// 少了键会让老客户端把"有示例"误判成"没示例"。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CardDto {
    pub id: i64,
    pub name: String,
    /// 正面提示词（整段自然语言）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// 负面提示词（整段文本）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// 示例产物（按加入顺序）。
    pub examples: Vec<CardExampleDto>,
}

/// 示例产物的**轻量**形状：够画缩略图、够点进详情，不必把整条 `MediaDto` 背回来。
///
/// `fileUrl` / `thumbUrl` 由后端算好（前端不许自己拼，AGENTS §10.1）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CardExampleDto {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub original_name: String,
    pub stored_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub size_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    pub nsfw: bool,
    pub file_url: String,
    pub thumb_url: String,
}

impl CardExampleDto {
    pub fn display_name(&self) -> &str {
        if self.title.trim().is_empty() {
            &self.original_name
        } else {
            &self.title
        }
    }
}

/// 写入用的一份卡（界面弹窗与 AI 工具共用）。
///
/// `name` 之外都是 `Option`：**界面的 PUT 是全量替换**（`None` = 清空那个字段），
/// 而 AI 那条路走 [`merge`]（`None` = 这次没提，保留原值）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CardInput {
    pub name: String,
    pub prompt: Option<String>,
    pub negative: Option<String>,
}

impl CardInput {
    /// 从模型的工具参数造一份（键名与 schema 一致，缺的一律 `None`）。
    pub fn from_arguments(arguments: &Value) -> Self {
        serde_json::from_value(arguments.clone()).unwrap_or_default()
    }
}

pub fn normalize(name: &str) -> String {
    name.trim().to_lowercase()
}

fn take_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// 字段超长**报错**而不是截断。
fn require_fits(
    value: Option<&String>,
    limit: usize,
    what: &str,
) -> Result<Option<String>, WriteError> {
    let Some(raw) = value else { return Ok(None) };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let len = trimmed.chars().count();
    if len > limit {
        return Err(WriteError::Validation(format!(
            "{what} 超出上限（{len} > {limit} 字符）；请精简一下，不要指望被悄悄截断"
        )));
    }
    Ok(Some(trimmed.to_string()))
}

fn row_to_card(row: &Row<'_>) -> rusqlite::Result<CardDto> {
    let created: Option<String> = row.get("created_at")?;
    let updated: Option<String> = row.get("updated_at")?;
    Ok(CardDto {
        id: row.get("id")?,
        name: row.get::<_, Option<String>>("name")?.unwrap_or_default(),
        prompt: row.get("prompt")?,
        negative: row.get("negative")?,
        created_at: iso_time(created.as_deref()),
        updated_at: iso_time(updated.as_deref()),
        examples: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
//  示例产物
// ---------------------------------------------------------------------------

fn example_from_row(row: &Row<'_>) -> rusqlite::Result<CardExampleDto> {
    let id: i64 = row.get("id")?;
    Ok(CardExampleDto {
        id,
        kind: row.get("kind").unwrap_or_else(|_| "IMAGE".to_string()),
        title: row.get::<_, Option<String>>("title")?.unwrap_or_default(),
        original_name: row
            .get::<_, Option<String>>("original_name")?
            .unwrap_or_default(),
        stored_name: row
            .get::<_, Option<String>>("stored_name")?
            .unwrap_or_default(),
        mime_type: row.get("mime_type")?,
        size_bytes: row.get::<_, Option<i64>>("size_bytes")?.unwrap_or(0),
        width: row.get("width")?,
        height: row.get("height")?,
        duration_ms: row.get("duration_ms")?,
        nsfw: row.get::<_, i64>("nsfw").unwrap_or(0) != 0,
        file_url: format!("/api/media/{id}/file"),
        thumb_url: format!("/api/media/{id}/thumb"),
    })
}

/// 这张卡挂了哪些示例（按加入顺序）。
pub fn list_examples(
    conn: &Connection,
    kind: CardKind,
    card_id: i64,
) -> rusqlite::Result<Vec<CardExampleDto>> {
    let sql = format!(
        "SELECT m.id, m.kind, m.title, m.original_name, m.stored_name, m.mime_type, \
                m.size_bytes, m.width, m.height, m.duration_ms, m.nsfw \
           FROM {examples} e JOIN media_assets m ON m.id = e.media_id \
          WHERE e.card_id = ?1 ORDER BY e.rowid ASC",
        examples = kind.example_table()
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![card_id], example_from_row)?;
    rows.collect()
}

fn with_examples(
    mut card: CardDto,
    conn: &Connection,
    kind: CardKind,
) -> rusqlite::Result<CardDto> {
    card.examples = list_examples(conn, kind, card.id)?;
    Ok(card)
}

/// 清洗并去重示例 id（保持传入顺序）。
fn clean_example_ids(ids: &[i64]) -> Result<Vec<i64>, WriteError> {
    let mut out: Vec<i64> = Vec::new();
    for raw in ids {
        if *raw <= 0 {
            return Err(WriteError::Validation(format!("示例产物 id 非法：{raw}")));
        }
        if !out.contains(raw) {
            out.push(*raw);
        }
    }
    if out.len() > MAX_EXAMPLES {
        return Err(WriteError::Validation(format!(
            "示例产物最多 {MAX_EXAMPLES} 条（这次给了 {} 条）；请先删掉一些再加",
            out.len()
        )));
    }
    Ok(out)
}

/// 每条示例都必须是画廊里真实存在的产物（外键也会拦，但那会报成 500）。
fn ensure_media_exist(conn: &Connection, ids: &[i64]) -> Result<(), WriteError> {
    for id in ids {
        let found: i64 = conn.query_row(
            "SELECT COUNT(*) FROM media_assets WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        if found == 0 {
            return Err(WriteError::Validation(format!(
                "示例产物 #{id} 不存在（可能已经被删掉了）"
            )));
        }
    }
    Ok(())
}

fn touch_card(conn: &Connection, kind: CardKind, card_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "UPDATE {table} SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
            table = kind.table()
        ),
        params![card_id],
    )?;
    Ok(())
}

/// 全量替换一张卡的示例（界面"管理示例"用）。
pub fn replace_examples(
    conn: &Connection,
    kind: CardKind,
    card_id: i64,
    media_ids: &[i64],
) -> Result<Vec<CardExampleDto>, WriteError> {
    let ids = clean_example_ids(media_ids)?;
    ensure_media_exist(conn, &ids)?;
    conn.execute(
        &format!(
            "DELETE FROM {examples} WHERE card_id = ?1",
            examples = kind.example_table()
        ),
        params![card_id],
    )?;
    for id in &ids {
        conn.execute(
            &format!(
                "INSERT INTO {examples} (card_id, media_id) VALUES (?1, ?2)",
                examples = kind.example_table()
            ),
            params![card_id, id],
        )?;
    }
    touch_card(conn, kind, card_id)?;
    Ok(list_examples(conn, kind, card_id)?)
}

/// 追加示例（AI 的 `save_*` 用；已经在卡里的 id 直接忽略）。
pub fn add_examples(
    conn: &Connection,
    kind: CardKind,
    card_id: i64,
    media_ids: &[i64],
) -> Result<Vec<CardExampleDto>, WriteError> {
    let ids = clean_example_ids(media_ids)?;
    ensure_media_exist(conn, &ids)?;
    let existing: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM {examples} WHERE card_id = ?1",
            examples = kind.example_table()
        ),
        params![card_id],
        |row| row.get(0),
    )?;
    let mut fresh = 0i64;
    for id in &ids {
        let found: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM {examples} WHERE card_id = ?1 AND media_id = ?2",
                examples = kind.example_table()
            ),
            params![card_id, id],
            |row| row.get(0),
        )?;
        if found == 0 {
            fresh += 1;
        }
    }
    if existing + fresh > MAX_EXAMPLES as i64 {
        return Err(WriteError::Validation(format!(
            "示例产物最多 {MAX_EXAMPLES} 条（这张卡已有 {existing} 条，这次还要再加 {fresh} 条）"
        )));
    }
    for id in &ids {
        conn.execute(
            &format!(
                "INSERT OR IGNORE INTO {examples} (card_id, media_id) VALUES (?1, ?2)",
                examples = kind.example_table()
            ),
            params![card_id, id],
        )?;
    }
    touch_card(conn, kind, card_id)?;
    Ok(list_examples(conn, kind, card_id)?)
}

pub fn remove_example(
    conn: &Connection,
    kind: CardKind,
    card_id: i64,
    media_id: i64,
) -> Result<bool, WriteError> {
    let changed = conn.execute(
        &format!(
            "DELETE FROM {examples} WHERE card_id = ?1 AND media_id = ?2",
            examples = kind.example_table()
        ),
        params![card_id, media_id],
    )?;
    if changed > 0 {
        touch_card(conn, kind, card_id)?;
    }
    Ok(changed > 0)
}

fn card_exists(conn: &Connection, kind: CardKind, id: i64) -> rusqlite::Result<bool> {
    let found: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM {table} WHERE id = ?1",
            table = kind.table()
        ),
        params![id],
        |row| row.get(0),
    )?;
    Ok(found > 0)
}

// ---------------------------------------------------------------------------
//  读
// ---------------------------------------------------------------------------

/// 把搜索词按空白切成若干段（中英文都按空白切），去重，最多 8 段。
///
/// 单段时行为与旧的"整串子串匹配"完全一致；多段时每一段单独匹配、
/// 按命中段数排序（见 [`list`]）。切词上限是防"把一整段提示词贴进搜索框"
/// 时拼出几百个 OR 条件。
fn search_tokens(q: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in q.split_whitespace() {
        if part.is_empty() || out.iter().any(|token| token == part) {
            continue;
        }
        out.push(part.to_string());
        if out.len() == 8 {
            break;
        }
    }
    out
}

pub fn list(
    conn: &Connection,
    kind: CardKind,
    q: Option<&str>,
    limit: usize,
) -> rusqlite::Result<Vec<CardDto>> {
    let keyword = q.map(|s| s.trim()).filter(|s| !s.is_empty());
    let limit = limit.clamp(1, 500);
    let mut out = Vec::new();
    match keyword {
        Some(q) => {
            // 按空白切词。模型常把描述写成一句话（"二次元三次元混搭 物理渲染
            // physically based anime"），整串 LIKE 一个都命中不了 —— 2026-10-31
            // 的 `search_style_cards` 就是这样把库里明明有的 #1 卡判成
            // NOT_FOUND 的。多词时按"命中几个词"排序，命中多的在前。
            let tokens = search_tokens(q);
            let mut score_parts = Vec::with_capacity(tokens.len());
            let mut terms: Vec<rusqlite::types::Value> = Vec::with_capacity(tokens.len() * 4 + 1);
            for (index, token) in tokens.iter().enumerate() {
                let param = index * 4 + 1;
                score_parts.push(format!(
                    "(CASE WHEN name LIKE ?{param} OR normalized LIKE ?{param} \
                              OR prompt LIKE ?{param} OR negative LIKE ?{param} \
                         THEN 1 ELSE 0 END)"
                ));
                let like = format!("%{token}%");
                for _ in 0..4 {
                    terms.push(rusqlite::types::Value::Text(like.clone()));
                }
            }
            let score = score_parts.join(" + ");
            let sql = format!(
                "SELECT * FROM (SELECT {BASE_COLUMNS}, ({score}) AS match_score \
                   FROM {table}) \
                  WHERE match_score > 0 \
                  ORDER BY match_score DESC, updated_at DESC, id DESC LIMIT ?{}",
                terms.len() + 1,
                table = kind.table()
            );
            terms.push(rusqlite::types::Value::Integer(limit as i64));
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(terms.iter()), row_to_card)?;
            for row in rows {
                out.push(with_examples(row?, conn, kind)?);
            }
        }
        None => {
            let sql = format!(
                "SELECT {BASE_COLUMNS} FROM {table} ORDER BY updated_at DESC, id DESC LIMIT ?1",
                table = kind.table()
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![limit as i64], row_to_card)?;
            for row in rows {
                out.push(with_examples(row?, conn, kind)?);
            }
        }
    }
    Ok(out)
}

pub fn get(conn: &Connection, kind: CardKind, id: i64) -> rusqlite::Result<Option<CardDto>> {
    conn.query_row(
        &format!(
            "SELECT {BASE_COLUMNS} FROM {table} WHERE id = ?1",
            table = kind.table()
        ),
        params![id],
        row_to_card,
    )
    .optional()?
    .map(|card| with_examples(card, conn, kind))
    .transpose()
}

pub fn get_by_normalized(
    conn: &Connection,
    kind: CardKind,
    name: &str,
) -> rusqlite::Result<Option<CardDto>> {
    conn.query_row(
        &format!(
            "SELECT {BASE_COLUMNS} FROM {table} WHERE normalized = ?1",
            table = kind.table()
        ),
        params![normalize(name)],
        row_to_card,
    )
    .optional()?
    .map(|card| with_examples(card, conn, kind))
    .transpose()
}

// ---------------------------------------------------------------------------
//  写
// ---------------------------------------------------------------------------

fn validate(kind: CardKind, input: &CardInput) -> Result<String, WriteError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(WriteError::Validation(format!(
            "{}不能为空",
            kind.name_label()
        )));
    }
    let len = name.chars().count();
    if len > MAX_NAME_CHARS {
        return Err(WriteError::Validation(format!(
            "{}太长（{len} > {MAX_NAME_CHARS} 字符）",
            kind.name_label()
        )));
    }
    Ok(name.to_string())
}

/// 两个文本字段的清洗（超长报错，空串落 NULL）。
fn clean_text(input: &CardInput) -> Result<(Option<String>, Option<String>), WriteError> {
    Ok((
        require_fits(input.prompt.as_ref(), MAX_PROMPT_CHARS, "正面提示词")?,
        require_fits(input.negative.as_ref(), MAX_PROMPT_CHARS, "负面提示词")?,
    ))
}

fn unique_error(kind: CardKind, err: rusqlite::Error) -> WriteError {
    if err.to_string().contains("UNIQUE constraint failed") {
        WriteError::Conflict(format!(
            "同名{}已经存在；请直接编辑那一张，或换一个名字",
            kind.label()
        ))
    } else {
        WriteError::Db(err)
    }
}

pub fn create(conn: &Connection, kind: CardKind, input: &CardInput) -> Result<i64, WriteError> {
    let name = validate(kind, input)?;
    let (prompt, negative) = clean_text(input)?;
    if let Err(err) = conn.execute(
        &format!(
            "INSERT INTO {table} (name, normalized, prompt, negative) \
             VALUES (?1,?2,?3,?4)",
            table = kind.table()
        ),
        params![name, normalize(&name), prompt, negative],
    ) {
        return Err(unique_error(kind, err));
    }
    Ok(conn.last_insert_rowid())
}

/// 全量替换（界面上的"编辑保存"用它：弹窗里所有字段都在；`None` = 清空）。
pub fn update(
    conn: &Connection,
    kind: CardKind,
    id: i64,
    input: &CardInput,
) -> Result<bool, WriteError> {
    let name = validate(kind, input)?;
    let (prompt, negative) = clean_text(input)?;
    let changed = match conn.execute(
        &format!(
            "UPDATE {table} SET name = ?1, normalized = ?2, prompt = ?3, negative = ?4 \
              WHERE id = ?5",
            table = kind.table()
        ),
        params![name, normalize(&name), prompt, negative, id],
    ) {
        Ok(changed) => changed,
        Err(err) => return Err(unique_error(kind, err)),
    };
    Ok(changed > 0)
}

pub fn delete(conn: &Connection, kind: CardKind, id: i64) -> Result<bool, WriteError> {
    Ok(conn.execute(
        &format!("DELETE FROM {table} WHERE id = ?1", table = kind.table()),
        params![id],
    )? > 0)
}

/// 合并（**AI 保存卡片时用这个**，不是全量替换）。
///
/// 模型一次只会说一件事（"这个角色的正面提示词是…"），全量替换会把它上一轮
/// 存下来的负面提示词一起抹掉。所以：`incoming` 里为空 / 没给的字段一律保留
/// `existing` 的值。
///
/// ⚠ 正面字段是**整段替换**（给了就整段覆盖），**不是追加** —— 追加会把
/// 两段风格不同的提示词粘在一起，读起来像一条、其实是两条。
pub fn merge(existing: Option<&CardDto>, incoming: CardInput) -> CardInput {
    let Some(existing) = existing else {
        return incoming;
    };
    let pick = |new: Option<String>, old: Option<String>| match new {
        Some(v) if !v.trim().is_empty() => Some(v),
        _ => old,
    };
    let name = if incoming.name.trim().is_empty() {
        existing.name.clone()
    } else {
        incoming.name.trim().to_string()
    };
    CardInput {
        name,
        prompt: pick(incoming.prompt, existing.prompt.clone()),
        negative: pick(incoming.negative, existing.negative.clone()),
    }
}

/// 按名字幂等保存：没有就建、有就合并。返回 `(id, created)`。
pub fn save(
    conn: &Connection,
    kind: CardKind,
    input: CardInput,
) -> Result<(i64, bool), WriteError> {
    let name = validate(kind, &input)?;
    let existing = get_by_normalized(conn, kind, &name)?;
    match existing {
        None => Ok((create(conn, kind, &input)?, true)),
        Some(existing) => {
            let merged = merge(Some(&existing), input);
            update(conn, kind, existing.id, &merged)?;
            Ok((existing.id, false))
        }
    }
}

// ---------------------------------------------------------------------------
//  AI 工具
// ---------------------------------------------------------------------------

fn clip(text: &str, max: usize) -> String {
    let len = text.chars().count();
    if len <= max {
        text.to_string()
    } else {
        format!("{}…（已截断，共 {len} 字符）", take_chars(text, max))
    }
}

fn examples_line(card: &CardDto) -> String {
    if card.examples.is_empty() {
        return "  示例产物:（无）\n".to_string();
    }
    let names = card
        .examples
        .iter()
        .map(|e| format!("#{}「{}」", e.id, e.display_name()))
        .collect::<Vec<_>>()
        .join("、");
    format!("  示例产物（{}）: {names}\n", card.examples.len())
}

/// 卡片在工具结果里的正文形状：模型要能一眼看出"卡里已经有什么、还缺什么"。
pub fn render_card(kind: CardKind, card: &CardDto) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  #{}「{}」（{}）\n",
        card.id,
        card.name,
        kind.label()
    ));
    match card.prompt.as_deref() {
        // 角色卡的正面提示词要求 600+ token（英文常见 3000~4000 字符）——
        // 这里给到 7000 才不会被截尾，否则模型"优化"时会照着截断版保存、悄悄丢细节。
        Some(prompt) => out.push_str(&format!("  正面提示词: {}\n", clip(prompt, 7000))),
        None => out.push_str("  正面提示词:（空）\n"),
    }
    if let Some(negative) = card.negative.as_deref() {
        out.push_str(&format!("  负面提示词: {}\n", clip(negative, 600)));
    }
    out.push_str(&examples_line(card));
    out.push_str(&format!(
        "  更新于 {}\n",
        card.updated_at.as_deref().unwrap_or("?")
    ));
    out
}

/// 卡片的结构化形状（可空字段整键省略）。
pub fn card_json(card: &CardDto) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), Value::Number(card.id.into()));
    out.insert("name".into(), Value::String(card.name.clone()));
    let mut put = |key: &str, value: Value| {
        out.insert(key.to_string(), value);
    };
    if let Some(v) = card.prompt.as_ref() {
        put("prompt", Value::String(v.clone()));
    }
    if let Some(v) = card.negative.as_ref() {
        put("negative", Value::String(v.clone()));
    }
    if let Some(v) = card.created_at.as_ref() {
        put("createdAt", Value::String(v.clone()));
    }
    if let Some(v) = card.updated_at.as_ref() {
        put("updatedAt", Value::String(v.clone()));
    }
    put(
        "examples",
        serde_json::to_value(&card.examples).unwrap_or_else(|_| Value::Array(Vec::new())),
    );
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

/// `exampleMediaIds` 允许数组或逗号分隔字符串（模型两种写法都见过）。
///
/// 非法项**报错**而不是静默丢弃：否则 AI 认为已挂载、界面却没有，属于"静默丢弃数据"。
fn arg_media_ids(
    arguments: &Value,
    key: &str,
) -> Result<Vec<i64>, crate::tools::tools::ToolFailure> {
    let invalid = |raw: &str| {
        crate::tools::tools::ToolFailure::new(
            "INVALID_ARGUMENT",
            format!("{key} 里有非整数项：{raw:?}；请传 mediaId 数组"),
        )
    };
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::Number(n) if n.as_i64().is_some() => {
                        out.push(n.as_i64().unwrap());
                    }
                    Value::String(s) => match s.trim().parse::<i64>() {
                        Ok(v) => out.push(v),
                        Err(_) => return Err(invalid(s)),
                    },
                    other => return Err(invalid(&other.to_string())),
                }
            }
            Ok(out)
        }
        Some(Value::String(raw)) => {
            let mut out = Vec::new();
            for part in raw.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                match part.parse::<i64>() {
                    Ok(v) => out.push(v),
                    Err(_) => return Err(invalid(part)),
                }
            }
            Ok(out)
        }
        Some(other) => Err(invalid(&other.to_string())),
    }
}

fn missing_prompt(card: &CardDto) -> bool {
    card.prompt.is_none()
}

/// `search_character_cards` / `search_style_cards`：查本机存下来的卡片。
pub fn tool_search(
    state: &AppState,
    kind: CardKind,
    arguments: &Value,
) -> Result<crate::tools::tools::ToolOutcome, crate::tools::tools::ToolFailure> {
    let query = arg_str(arguments, "query");
    let limit = arg_i64(arguments, "limit")
        .unwrap_or(5)
        .clamp(1, MAX_LIBRARY_RESULTS);
    let cards = state
        .with(|conn| list(conn, kind, query.as_deref(), limit as usize))
        .map_err(api_failure)?;
    if cards.is_empty() {
        let next = match kind {
            CardKind::Character => {
                "`danbooru_lookup(query=\"角色名\")` 核对写法与同现 tag（只作参考），\
                 再把角色写成**一版自然语言正面提示词**，用 `save_character_card` 存成角色卡；\
                 **不要**把 tag 栈当正文存进去。"
            }
            CardKind::Style => {
                "先 `search_prompts` 看库里有没有现成的画风提示词；再按用户要的方向写\
                 **一版 100~200 token 起（复杂画风按需更长、材质按部位分开写）的自然语言画风描述**，\
                 用 `save_style_card` 存成画风卡。\
                 画风卡管「画成什么样」，不要把它写成人物设定。"
            }
        };
        return Err(crate::tools::tools::ToolFailure::new(
            "NOT_FOUND",
            format!(
                "本机没有匹配的{}（关键字：{}）。下一步：{next}",
                kind.label(),
                query.unwrap_or_else(|| "（空）".to_string())
            ),
        ));
    }
    let mut text = format!("{}：命中 {} 张\n", kind.label(), cards.len());
    for card in cards.iter() {
        text.push('\n');
        text.push_str(&render_card(kind, card));
    }
    let payload = serde_json::json!({
        "count": cards.len(),
        "items": cards.iter().map(card_json).collect::<Vec<_>>(),
    });
    Ok(crate::tools::tools::ToolOutcome::new(text, payload))
}

/// `save_character_card` / `save_style_card`。
///
/// 同名 = 同一张卡（合并保存：这次没给的字段保持原样）。
/// 只给名字也能存（先占位），但正文会提醒"正面提示词还空着"。
pub fn tool_save(
    state: &AppState,
    kind: CardKind,
    arguments: &Value,
) -> Result<crate::tools::tools::ToolOutcome, crate::tools::tools::ToolFailure> {
    let name = arg_str(arguments, "name").ok_or_else(|| {
        crate::tools::tools::ToolFailure::new("INVALID_ARGUMENT", "缺少参数 name")
    })?;
    let input = CardInput {
        name: name.clone(),
        prompt: arg_str(arguments, "prompt"),
        negative: arg_str(arguments, "negative"),
    };
    let media_ids = arg_media_ids(arguments, "exampleMediaIds")?;
    let (id, created) = state
        .write(|conn| {
            let (id, created) = save(conn, kind, input)?;
            if !media_ids.is_empty() {
                add_examples(conn, kind, id, &media_ids)?;
            }
            Ok((id, created))
        })
        .map_err(api_failure)?;
    let card = state
        .with(|conn| get(conn, kind, id))
        .map_err(api_failure)?
        .ok_or_else(|| {
            crate::tools::tools::ToolFailure::new(
                "TOOL_ERROR",
                format!(
                    "{}已保存但读不回来（id={id}）；这是本机的 bug，请如实告诉用户",
                    kind.label()
                ),
            )
        })?;
    let mut text = format!(
        "已保存{} #{id}「{}」（{}）\n",
        kind.label(),
        card.name,
        if created { "新建" } else { "合并更新" }
    );
    text.push_str(&render_card(kind, &card));
    if missing_prompt(&card) {
        let length_rule = match kind {
            CardKind::Character => "角色卡的正面提示词要写到 600+ token、覆盖全身上下每一个细节",
            CardKind::Style => {
                "画风卡的正面提示词至少 100~200 token、一段话写完画风；\
                复杂画风（材质按面部 / 头发 / 衣物 / 其他物体分开、多套打光）该写多长写多长"
            }
        };
        text.push_str(&format!(
            "\n⚠ 这张卡还空着**正面提示词**。写一版**自然语言**正文再带完整字段保存一次；\
             只写 tag 栈不算数（用户要 tag 版时当场另翻一版给他看，不要存进卡里）。\
             {length_rule}。规则见 skill `card-writer`。"
        ));
    }
    Ok(crate::tools::tools::ToolOutcome::new(
        text,
        card_json(&card),
    ))
}

/// 把 `state.with` / `state.write` 折出来的 `ApiError` 翻成 `ToolFailure`。
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

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/ai/character-cards",
            axum::routing::get(list_characters).post(create_character),
        )
        .route(
            "/api/ai/character-cards/{id}",
            axum::routing::get(get_character)
                .put(update_character)
                .delete(delete_character),
        )
        .route(
            "/api/ai/character-cards/{id}/examples",
            axum::routing::get(list_character_examples)
                .post(add_character_examples)
                .put(replace_character_examples),
        )
        .route(
            "/api/ai/character-cards/{id}/examples/{media_id}",
            axum::routing::delete(delete_character_example),
        )
        .route(
            "/api/ai/style-cards",
            axum::routing::get(list_styles).post(create_style),
        )
        .route(
            "/api/ai/style-cards/{id}",
            axum::routing::get(get_style)
                .put(update_style)
                .delete(delete_style),
        )
        .route(
            "/api/ai/style-cards/{id}/examples",
            axum::routing::get(list_style_examples)
                .post(add_style_examples)
                .put(replace_style_examples),
        )
        .route(
            "/api/ai/style-cards/{id}/examples/{media_id}",
            axum::routing::delete(delete_style_example),
        )
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    q: Option<String>,
    limit: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ExamplesInput {
    media_ids: Vec<i64>,
}

async fn list_cards_impl(
    state: AppState,
    query: ListQuery,
    kind: CardKind,
) -> Result<Json<Vec<CardDto>>, ApiError> {
    let limit = query
        .limit
        .as_deref()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(MAX_SEARCH);
    let cards = state.with(|conn| list(conn, kind, query.q.as_deref(), limit))?;
    Ok(Json(cards))
}

async fn create_card_impl(
    state: AppState,
    input: CardInput,
    kind: CardKind,
) -> Result<(StatusCode, Json<CardDto>), ApiError> {
    let id = state.write(|conn| create(conn, kind, &input))?;
    let card = state
        .with(|conn| get(conn, kind, id))?
        .ok_or_else(|| ApiError::internal("创建失败"))?;
    Ok((StatusCode::CREATED, Json(card)))
}

async fn get_card_impl(
    state: AppState,
    id: i64,
    kind: CardKind,
) -> Result<Json<CardDto>, ApiError> {
    state
        .with(|conn| get(conn, kind, id))?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("{} {id} 不存在", kind.label())))
}

async fn update_card_impl(
    state: AppState,
    id: i64,
    input: CardInput,
    kind: CardKind,
) -> Result<Json<CardDto>, ApiError> {
    let updated = state.write(|conn| update(conn, kind, id, &input))?;
    if !updated {
        return Err(ApiError::not_found(format!("{} {id} 不存在", kind.label())));
    }
    state
        .with(|conn| get(conn, kind, id))?
        .map(Json)
        .ok_or_else(|| ApiError::internal("读取失败"))
}

async fn delete_card_impl(
    state: AppState,
    id: i64,
    kind: CardKind,
) -> Result<Json<Value>, ApiError> {
    let deleted = state.write(|conn| delete(conn, kind, id))?;
    if !deleted {
        return Err(ApiError::not_found(format!("{} {id} 不存在", kind.label())));
    }
    let mut out = Map::new();
    out.insert("deleted".into(), Value::Number(id.into()));
    Ok(Json(Value::Object(out)))
}

async fn list_examples_impl(
    state: AppState,
    id: i64,
    kind: CardKind,
) -> Result<Json<Vec<CardExampleDto>>, ApiError> {
    let exists = state.with(|conn| card_exists(conn, kind, id))?;
    if !exists {
        return Err(ApiError::not_found(format!("{} {id} 不存在", kind.label())));
    }
    let examples = state.with(|conn| list_examples(conn, kind, id))?;
    Ok(Json(examples))
}

async fn replace_examples_impl(
    state: AppState,
    id: i64,
    body: ExamplesInput,
    kind: CardKind,
) -> Result<Json<CardDto>, ApiError> {
    state.write(|conn| {
        if !card_exists(conn, kind, id)? {
            return Err(WriteError::NotFound);
        }
        replace_examples(conn, kind, id, &body.media_ids)?;
        Ok(())
    })?;
    state
        .with(|conn| get(conn, kind, id))?
        .map(Json)
        .ok_or_else(|| ApiError::internal("读取失败"))
}

async fn add_examples_impl(
    state: AppState,
    id: i64,
    body: ExamplesInput,
    kind: CardKind,
) -> Result<Json<CardDto>, ApiError> {
    state.write(|conn| {
        if !card_exists(conn, kind, id)? {
            return Err(WriteError::NotFound);
        }
        add_examples(conn, kind, id, &body.media_ids)?;
        Ok(())
    })?;
    state
        .with(|conn| get(conn, kind, id))?
        .map(Json)
        .ok_or_else(|| ApiError::internal("读取失败"))
}

async fn delete_example_impl(
    state: AppState,
    card_id: i64,
    media_id: i64,
    kind: CardKind,
) -> Result<Json<Value>, ApiError> {
    state.write(|conn| {
        if !card_exists(conn, kind, card_id)? {
            return Err(WriteError::NotFound);
        }
        remove_example(conn, kind, card_id, media_id)?;
        Ok(())
    })?;
    let mut out = Map::new();
    out.insert("deleted".into(), Value::Number(media_id.into()));
    Ok(Json(Value::Object(out)))
}

async fn list_characters(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<CardDto>>, ApiError> {
    list_cards_impl(state, query, CardKind::Character).await
}

async fn create_character(
    State(state): State<AppState>,
    Json(input): Json<CardInput>,
) -> Result<(StatusCode, Json<CardDto>), ApiError> {
    create_card_impl(state, input, CardKind::Character).await
}

async fn get_character(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<Json<CardDto>, ApiError> {
    get_card_impl(state, id, CardKind::Character).await
}

async fn update_character(
    State(state): State<AppState>,
    Id(id): Id,
    Json(input): Json<CardInput>,
) -> Result<Json<CardDto>, ApiError> {
    update_card_impl(state, id, input, CardKind::Character).await
}

async fn delete_character(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<Json<Value>, ApiError> {
    delete_card_impl(state, id, CardKind::Character).await
}

async fn list_character_examples(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<Json<Vec<CardExampleDto>>, ApiError> {
    list_examples_impl(state, id, CardKind::Character).await
}

async fn replace_character_examples(
    State(state): State<AppState>,
    Id(id): Id,
    Json(body): Json<ExamplesInput>,
) -> Result<Json<CardDto>, ApiError> {
    replace_examples_impl(state, id, body, CardKind::Character).await
}

async fn add_character_examples(
    State(state): State<AppState>,
    Id(id): Id,
    Json(body): Json<ExamplesInput>,
) -> Result<Json<CardDto>, ApiError> {
    add_examples_impl(state, id, body, CardKind::Character).await
}

async fn delete_character_example(
    State(state): State<AppState>,
    Path((id, media_id)): Path<(i64, i64)>,
) -> Result<Json<Value>, ApiError> {
    delete_example_impl(state, id, media_id, CardKind::Character).await
}

async fn list_styles(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<CardDto>>, ApiError> {
    list_cards_impl(state, query, CardKind::Style).await
}

async fn create_style(
    State(state): State<AppState>,
    Json(input): Json<CardInput>,
) -> Result<(StatusCode, Json<CardDto>), ApiError> {
    create_card_impl(state, input, CardKind::Style).await
}

async fn get_style(State(state): State<AppState>, Id(id): Id) -> Result<Json<CardDto>, ApiError> {
    get_card_impl(state, id, CardKind::Style).await
}

async fn update_style(
    State(state): State<AppState>,
    Id(id): Id,
    Json(input): Json<CardInput>,
) -> Result<Json<CardDto>, ApiError> {
    update_card_impl(state, id, input, CardKind::Style).await
}

async fn delete_style(State(state): State<AppState>, Id(id): Id) -> Result<Json<Value>, ApiError> {
    delete_card_impl(state, id, CardKind::Style).await
}

async fn list_style_examples(
    State(state): State<AppState>,
    Id(id): Id,
) -> Result<Json<Vec<CardExampleDto>>, ApiError> {
    list_examples_impl(state, id, CardKind::Style).await
}

async fn replace_style_examples(
    State(state): State<AppState>,
    Id(id): Id,
    Json(body): Json<ExamplesInput>,
) -> Result<Json<CardDto>, ApiError> {
    replace_examples_impl(state, id, body, CardKind::Style).await
}

async fn add_style_examples(
    State(state): State<AppState>,
    Id(id): Id,
    Json(body): Json<ExamplesInput>,
) -> Result<Json<CardDto>, ApiError> {
    add_examples_impl(state, id, body, CardKind::Style).await
}

async fn delete_style_example(
    State(state): State<AppState>,
    Path((id, media_id)): Path<(i64, i64)>,
) -> Result<Json<Value>, ApiError> {
    delete_example_impl(state, id, media_id, CardKind::Style).await
}

// ---------------------------------------------------------------------------
//  测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
