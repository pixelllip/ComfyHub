//! 角色卡（P4 第 46 步，Kotlin 侧是 O8 / 2026-09-25 那一轮）。
//!
//! 一张卡 = 一个角色的**名称 + 正面提示词 + 负面提示词**（2026-10 按用户要求收敛到这三样，
//! 表注释与迁移见 `db/schema.sqlite.sql` 与 `infra::db::backfill_character_card_prompt`）。
//! 它要解决的现场问题："AI 每次写角色都靠印象编 tag" —— 库里明明有 `shinku_(nte)` 的完整
//! tag 栈（prompts 261/310），模型却把 shinku 当成 iroi 写了两版废图（总纲 §10.3 的 O7/O8）。
//!
//! 三条纪律：
//!
//!  1. **同名 = 同一张卡**：`normalized` 唯一索引兜住（`save` 走"先查再合并"；
//!     **不用** `INSERT OR REPLACE` —— R3：它内部是 DELETE+INSERT，会点亮 DELETE 触发器
//!     往 `_sync_log` 写**假的 delete**）。本表当前**不在同步范围**（总纲 §3.3），
//!     但这条习惯要留着：哪天进同步范围时不必回头改。
//!  2. **AI 那条路是"合并保存"**：模型一次只说一件事，只有**这次真给了值**的字段才覆盖，
//!     其余保持原样（全量替换会把上一轮的提示词悄悄抹掉）。
//!  3. **写不进去就报错，不许静默截断**：字段超长 / 名字为空一律 400 + 人话
//!     （与 `MemoryStore` 的取向一致 —— 悄悄丢用户的话更糟）。
//!
//! # 端点（与 Kotlin 逐字同形，少了一条）
//!
//! ```text
//!  GET    /api/ai/character-cards           列表（?q= 关键字 / ?limit=）
//!  POST   /api/ai/character-cards           新建 → 201 + 卡
//!  GET    /api/ai/character-cards/{id}      详情
//!  PUT    /api/ai/character-cards/{id}      编辑（全量替换，界面弹窗用）
//!  DELETE /api/ai/character-cards/{id}      删除 → {"deleted": id}
//! ```
//!
//! ⚠ 2026-10 **删掉了 `/api/ai/character-cards/summary`**（以及 `MAX_ENABLED` / token 估算 /
//! 5×5 位置网格）：那是"生图角色卡框架"那一层的东西，随"只留三个字段"一起收敛。
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
/// 正面 / 负面提示词的上限（够写一整段；超了**报错**，不截断）。
pub const MAX_PROMPT_CHARS: usize = 4000;
/// 搜索一次最多回几张卡（给模型看的，不是分页）。
pub const MAX_SEARCH: usize = 50;

/// 资料库类查询一次最多回几条（与 Kotlin `MAX_LIBRARY_RESULTS` 一致）。
pub const MAX_LIBRARY_RESULTS: i64 = 20;

const BASE_SELECT: &str =
    "SELECT id, name, normalized, prompt, negative, created_at, updated_at FROM character_cards";

/// 一张角色卡（`CharacterCardDto`）。
///
/// ⚠ 可空字段一律 `skip_serializing_if`（Kotlin 的 `explicitNulls = false`）——
/// 少了它界面上会多出一堆 `null` 键，`tests/shape_lint.rs` 也会红。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterCardDto {
    pub id: i64,
    pub name: String,
    /// 正面提示词（整段文本）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// 负面提示词（整段文本）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// 写入用的一份卡（`CharacterCardInput`）—— 界面弹窗与 AI 工具共用的形状。
///
/// `name` 之外都是 `Option`：**界面的 PUT 是全量替换**（`None` = 清空那个字段），
/// 而 AI 那条路走 [`merge`]（`None` = 这次没提，保留原值）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CharacterCardInput {
    pub name: String,
    pub prompt: Option<String>,
    pub negative: Option<String>,
}

impl CharacterCardInput {
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
            "{what} 超出上限（{len} > {limit} 字符）；请精简一下，不要指望被悄悄截断"
        )));
    }
    Ok(Some(trimmed.to_string()))
}

fn row_to_card(row: &Row<'_>) -> rusqlite::Result<CharacterCardDto> {
    let created: Option<String> = row.get("created_at")?;
    let updated: Option<String> = row.get("updated_at")?;
    Ok(CharacterCardDto {
        id: row.get("id")?,
        name: row.get::<_, Option<String>>("name")?.unwrap_or_default(),
        prompt: row.get("prompt")?,
        negative: row.get("negative")?,
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
                "{BASE_SELECT} WHERE (name LIKE ?1 OR normalized LIKE ?1 OR prompt LIKE ?1 \
                 OR negative LIKE ?1) ORDER BY updated_at DESC, id DESC LIMIT ?2"
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

/// 两个文本字段的清洗（超长报错，空串落 NULL）。
fn clean_text(input: &CharacterCardInput) -> Result<(Option<String>, Option<String>), WriteError> {
    Ok((
        require_fits(input.prompt.as_ref(), MAX_PROMPT_CHARS, "正面提示词")?,
        require_fits(input.negative.as_ref(), MAX_PROMPT_CHARS, "负面提示词")?,
    ))
}

pub fn create(conn: &Connection, input: &CharacterCardInput) -> Result<i64, WriteError> {
    let name = validate(input)?;
    let (prompt, negative) = clean_text(input)?;
    conn.execute(
        "INSERT INTO character_cards (name, normalized, prompt, negative) VALUES (?1,?2,?3,?4)",
        params![name, normalize(&name), prompt, negative],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 全量替换（界面上的"编辑保存"用它：弹窗里所有字段都在；`None` = 清空）。
pub fn update(conn: &Connection, id: i64, input: &CharacterCardInput) -> Result<bool, WriteError> {
    let name = validate(input)?;
    let (prompt, negative) = clean_text(input)?;
    let changed = conn.execute(
        "UPDATE character_cards SET name = ?1, normalized = ?2, prompt = ?3, negative = ?4 WHERE id = ?5",
        params![name, normalize(&name), prompt, negative, id],
    )?;
    Ok(changed > 0)
}

pub fn delete(conn: &Connection, id: i64) -> Result<bool, WriteError> {
    Ok(conn.execute("DELETE FROM character_cards WHERE id = ?1", params![id])? > 0)
}

/// 合并（**AI 保存角色卡时用这个**，不是全量替换）。
///
/// 为什么：模型一次只会说一件事（"这个角色的负面词是 bad hands"），
/// 全量替换会把它上一轮存下来的正面提示词一起抹掉 —— 那是**悄悄丢用户的数据**。
/// 所以：`incoming` 里为空 / 没给的字段一律保留 `existing` 的值。
///
/// ⚠ `prompt` 是**整段替换**（给了就整段覆盖），**不是追加** —— 追加会把两段风格不同的
/// 提示词粘在一起，读起来像一条、其实是两条。要"在原有的基础上补两句"就先
/// `search_character_cards` 把卡读出来，把完整的那一份重新给一遍（工具描述里写着这条）。
pub fn merge(existing: Option<&CharacterCardDto>, incoming: CharacterCardInput) -> CharacterCardInput {
    let Some(existing) = existing else { return incoming };
    let pick = |new: Option<String>, old: Option<String>| match new {
        Some(v) if !v.trim().is_empty() => Some(v),
        _ => old,
    };
    let name = if incoming.name.trim().is_empty() {
        existing.name.clone()
    } else {
        incoming.name.trim().to_string()
    };
    CharacterCardInput {
        name,
        prompt: pick(incoming.prompt, existing.prompt.clone()),
        negative: pick(incoming.negative, existing.negative.clone()),
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

fn clip(text: &str, max: usize) -> String {
    let len = text.chars().count();
    if len <= max {
        text.to_string()
    } else {
        format!("{}…（已截断，共 {len} 字符）", take_chars(text, max))
    }
}

/// 角色卡在工具结果里的正文形状：模型要能一眼看出"卡里已经有什么"。
pub fn render_card(card: &CharacterCardDto) -> String {
    let mut out = String::new();
    out.push_str(&format!("  #{}「{}」\n", card.id, card.name));
    match card.prompt.as_deref() {
        Some(prompt) => out.push_str(&format!("  正面提示词: {}\n", clip(prompt, 2000))),
        None => out.push_str("  正面提示词:（空）\n"),
    }
    if let Some(negative) = card.negative.as_deref() {
        out.push_str(&format!("  负面提示词: {}\n", clip(negative, 600)));
    }
    out.push_str(&format!("  更新于 {}\n", card.updated_at.as_deref().unwrap_or("?")));
    out
}

/// 角色卡的结构化形状（可空字段整键省略）。
pub fn card_json(card: &CharacterCardDto) -> Value {
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
                "本机没有匹配的角色卡（关键字：{}）。下一步：`danbooru_lookup(query=\"角色名\")` 核对写法与同现 tag，\
                 再把给这个角色用的正面 / 负面提示词用 `save_character_card` 存成卡；**不要**凭印象编 tag。",
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

/// `save_character_card`：把一个角色的**名称 / 正面提示词 / 负面提示词**存成角色卡。
///
/// 同名 = 同一张卡（走 [`save`] 的合并保存：这次没给的字段保持原样）。
/// **只有名称也能存**（先占个位、下次再补提示词），但正文里会提醒"这张卡还是空的"。
pub fn tool_save(
    state: &AppState,
    arguments: &Value,
) -> Result<crate::tools::tools::ToolOutcome, crate::tools::tools::ToolFailure> {
    let name = arg_str(arguments, "name")
        .ok_or_else(|| crate::tools::tools::ToolFailure::new("INVALID_ARGUMENT", "缺少参数 name"))?;
    let input = CharacterCardInput {
        name: name.clone(),
        prompt: arg_str(arguments, "prompt"),
        negative: arg_str(arguments, "negative"),
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
    if card.prompt.is_none() {
        text.push_str(
            "\n⚠ 这张卡还**没有正面提示词**（用户打开这一页会看到空的）—— \
             想补齐就先 `search_prompts` / `search_character_cards` / `danbooru_lookup` 查清楚，\
             再带 `prompt` 保存一次；**不要**凭印象编。",
        );
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
mod tests;
