//! 会话与消息的**读端点**（P4 第 21 步 = `/api/ai/*` 的第三批）。
//!
//! 为什么先搬这三条：界面打开 AI 工作台要做的第一件事就是"把历史会话列出来、
//! 点进去把消息与有序块画出来"。这一批搬完之后，**指向 Rust 后端也能看到全部历史** ——
//! 而不是整页 `接口不存在`。写侧（建会话 / 改标题 / 删会话 / 追加消息）留在下一批，
//! Run 与 SSE 更后面（R5/R6 还没重建）。
//!
//! # 形状逐条对照 `AiConversationRepo`（真库副本上比过，不是照 DTO 猜）
//!
//! * `AiConversationDto`：`id` / `title` / `providerId?` / `modelId?` /
//!   `systemPromptVersion` / `archived` / `messageCount` / `createdAt?` / `updatedAt?`
//!   —— 后两个可空（`isoTime` 解析不出来就 null → 键不出现），
//!   而 `systemPromptVersion`（默认 `"v1"`）与 `archived` / `messageCount`
//!   **一定在**（Kotlin `encodeDefaults = true`）。
//! * `title` 在库里是 NULL 时回 `"新对话"`（`toConversation` 的 `?:`），不是空串。
//! * `AiMessageDto`：`parts` **一定是数组**（没有块就是 `[]`），
//!   `text` NULL → `""`，`status` NULL → `"complete"`；`usage` 是**原样 JSON 对象**。
//! * `parts[]`：`type` 一定在，其余四个（`text` / `attachmentId` / `toolCallId` /
//!   `jsonPayload`）为空就**整个键不出现**。
//!
//! # 三条容易静默做错的地方
//!
//! 1. **有序块的顺序**：Kotlin 先按 `p.message_id, p.ordinal` 取出来、再按 `message_id`
//!    分组，所以**每一条消息内部的块是 ordinal 升序**。少了这一步，工具卡与正文的顺序
//!    就会乱（而正文还在，看起来只是"顺序有点怪"）。
//! 2. **消息顺序**是 `ORDER BY seq, created_at`（**不是** `created_at` 单列）——
//!    同一毫秒发的两条消息只靠 `seq` 才排得稳。
//! 3. **会话列表**是 `ORDER BY c.updated_at DESC LIMIT ?`，且 `WHERE (? = 1 OR c.archived = 0)`
//!    用的是**位置参数**与 `includeArchived=1/true` 两个写法都认；`limit` 夹在 1..500。

use serde::Serialize;
use serde_json::Value;

use crate::iso_time::iso_time;
use crate::routes::{ApiError, AppState};

/// 会话列表里的一行（与 Kotlin `AiConversationDto` 逐字同形）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConversationDto {
    pub id: String,
    /// 库里 NULL → `"新对话"`（不是空串）
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// Kotlin 有默认值 `"v1"` 且 `encodeDefaults = true` → **一定在**
    pub system_prompt_version: String,
    pub archived: bool,
    pub message_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// 一条消息的一个有序块。
///
/// `Deserialize` 是为了 `POST .../messages` 的 `parts[]`（写侧与读侧**同一份结构**：
/// 分开写两份就会出现"写进去的键名与读出来的不一样"，而那种错在界面上只是少一块内容）。
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiMessagePartDto {
    /// text / reasoning / attachment / tool_call / tool_result
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// 工具调用 / 工具结果的结构化负载，**原样 JSON**（不是字符串）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_payload: Option<Value>,
}

/// 一条消息（带它的有序块）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiMessageDto {
    pub id: String,
    pub conversation_id: String,
    pub seq: i64,
    /// user / assistant / tool / system_note
    pub role: String,
    /// pending / streaming / complete / failed / cancelled
    pub status: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_response_id: Option<String>,
    /// token 用量（`usage_json` 列），原样 JSON 对象
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    /// **一定是数组**：没有块就是 `[]`（前端按它渲染工具卡与思考段）
    pub parts: Vec<AiMessagePartDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// `GET /api/ai/conversations?includeArchived=1|true`。
pub fn list(state: &AppState, include_archived: bool, limit: Option<i64>) -> Result<Vec<AiConversationDto>, ApiError> {
    let limit = limit.unwrap_or(200).clamp(1, 500);
    state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT c.*, (SELECT COUNT(*) FROM ai_messages m WHERE m.conversation_id = c.id) AS message_count
               FROM ai_conversations c
              WHERE (?1 = 1 OR c.archived = 0)
              ORDER BY c.updated_at DESC
              LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![if include_archived { 1 } else { 0 }, limit], conversation_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// `GET /api/ai/conversations/{id}`。
///
/// 不存在 → **400 `CONFIG_ERROR` + `detail:"会话不存在: xx"`**（`AiException`，
/// 与 Kotlin 的 `requireProvider` / `respondNotFound` 那套不一样：AI 领域一律走 `error` 稳定码）。
pub fn get(state: &AppState, id: &str) -> Result<AiConversationDto, ApiError> {
    let found = state.with(|conn| {
        let mut stmt = conn.prepare(
            "SELECT c.*, (SELECT COUNT(*) FROM ai_messages m WHERE m.conversation_id = c.id) AS message_count
               FROM ai_conversations c WHERE c.id = ?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![id], conversation_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    })?;
    found.ok_or_else(|| conversation_missing(id))
}

fn conversation_missing(id: &str) -> ApiError {
    ApiError::ai("CONFIG_ERROR", format!("会话不存在: {id}"))
}

/// `GET /api/ai/conversations/{id}/messages`。
///
/// ⚠ 先确认会话存在（Kotlin 的 `AiConversationRepo.get(id) ?: throw`）：
/// 对着一个不存在的会话回 `[]` 会让界面显示"这条会话是空的"，而不是"它没了"。
pub fn list_messages(state: &AppState, conversation_id: &str) -> Result<Vec<AiMessageDto>, ApiError> {
    // 存在性检查与取消息用**同一条连接**（AppState 只有一条）：分两次 `with` 也可以，
    // 但这里刻意合成一次 —— 少一次加锁，语义也更清楚（这一段是一次读快照）。
    state.with(|conn| {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_conversations WHERE id = ?1",
            rusqlite::params![conversation_id],
            |row| row.get(0),
        )?;
        if exists == 0 {
            return Ok(None);
        }

        let mut stmt = conn.prepare(
            "SELECT * FROM ai_messages WHERE conversation_id = ?1 ORDER BY seq, created_at",
        )?;
        let mut messages = stmt
            .query_map(rusqlite::params![conversation_id], message_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // 有序块：按 (message_id, ordinal) 取出来，再按 message_id 归拢 ——
        // Kotlin 就是这么做的，所以**每条消息内部的块是 ordinal 升序**
        let mut stmt = conn.prepare(
            "SELECT p.* FROM ai_message_parts p
               JOIN ai_messages m ON m.id = p.message_id
              WHERE m.conversation_id = ?1
              ORDER BY p.message_id, p.ordinal",
        )?;
        let mut rows = stmt.query(rusqlite::params![conversation_id])?;
        let mut parts: std::collections::HashMap<String, Vec<AiMessagePartDto>> =
            std::collections::HashMap::new();
        while let Some(row) = rows.next()? {
            let message_id: String = row.get("message_id")?;
            parts.entry(message_id).or_default().push(part_from_row(row)?);
        }
        for message in messages.iter_mut() {
            message.parts = parts.remove(&message.id).unwrap_or_default();
        }
        Ok(Some(messages))
    })?
    .ok_or_else(|| conversation_missing(conversation_id))
}

/// 一行 `ai_conversations`（可带 `message_count` 子查询列）→ DTO。
pub fn conversation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AiConversationDto> {
    // `message_count` 只在列表 / 详情那两条 SQL 里出现；用 `get` 失败就当 0
    // （Kotlin 那边是 `runCatching { getInt("message_count") }.getOrDefault(0)`）
    let message_count: i64 = row.get("message_count").unwrap_or(0);
    Ok(AiConversationDto {
        id: row.get("id")?,
        title: row
            .get::<_, Option<String>>("title")?
            .unwrap_or_else(|| "新对话".to_string()),
        provider_id: row.get("provider_id")?,
        model_id: row.get("model_id")?,
        system_prompt_version: row
            .get::<_, Option<String>>("system_prompt_version")?
            .unwrap_or_else(|| "v1".to_string()),
        archived: row.get::<_, Option<i64>>("archived")?.unwrap_or(0) != 0,
        message_count,
        created_at: iso_time(row.get::<_, Option<String>>("created_at")?.as_deref()),
        updated_at: iso_time(row.get::<_, Option<String>>("updated_at")?.as_deref()),
    })
}

/// 一行 `ai_messages` → DTO（`parts` 由调用方补上）。
pub fn message_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AiMessageDto> {
    Ok(AiMessageDto {
        id: row.get("id")?,
        conversation_id: row.get("conversation_id")?,
        seq: row.get::<_, Option<i64>>("seq")?.unwrap_or(0),
        role: row.get::<_, Option<String>>("role")?.unwrap_or_default(),
        status: row
            .get::<_, Option<String>>("status")?
            .unwrap_or_else(|| "complete".to_string()),
        text: row.get::<_, Option<String>>("text")?.unwrap_or_default(),
        provider_id: row.get("provider_id")?,
        model_id: row.get("model_id")?,
        provider_response_id: row.get("provider_response_id")?,
        usage: parse_json_object(row.get::<_, Option<String>>("usage_json")?.as_deref()),
        parts: Vec::new(),
        created_at: iso_time(row.get::<_, Option<String>>("created_at")?.as_deref()),
        updated_at: iso_time(row.get::<_, Option<String>>("updated_at")?.as_deref()),
    })
}

/// 一行 `ai_message_parts` → DTO。
pub fn part_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AiMessagePartDto> {
    Ok(AiMessagePartDto {
        r#type: row.get::<_, Option<String>>("type")?.unwrap_or_default(),
        text: row.get("text")?,
        attachment_id: row.get("attachment_id")?,
        tool_call_id: row.get("tool_call_id")?,
        json_payload: parse_json_any(row.get::<_, Option<String>>("json_payload")?.as_deref()),
    })
}

// ---------------------------------------------------------------------------
//  写侧（P4 第 24 步）
// ---------------------------------------------------------------------------

/// 允许的角色 / 状态 —— **与 Kotlin `AiConversationRepo.ROLES / STATUSES` 同一个集合**。
///
/// 少一个值 = 一次合法的追加被判 400（比如手机上 `system_note` 用不了）；
/// 多一个值 = 库里出现界面渲染不出来的行。两边的集合必须逐字一致。
pub const ROLES: [&str; 4] = ["user", "assistant", "tool", "system_note"];
pub const STATUSES: [&str; 5] = ["pending", "streaming", "complete", "failed", "cancelled"];

/// `POST /api/ai/conversations` 的入参（`AiConversationCreate`）。
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationCreate {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
}

/// `PATCH /api/ai/conversations/{id}` 的入参（`AiConversationPatch`）。
///
/// ⚠ **`null` 与"没给这个键"同义 = 不改**（Kotlin 是 `req.x ?: current.x`）。
/// 想清空 providerId 现在做不到 —— 那不是这个端点的能力，别在这里"顺手支持一下"。
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub archived: Option<bool>,
}

/// `POST /api/ai/conversations/{id}/messages` 的入参（`AiMessageAppend`）。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageAppend {
    pub role: String,
    #[serde(default)]
    pub text: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    /// 调用方指定的消息 id（界面按它认自己插的那条气泡）；不给就自己生成一个。
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub parts: Vec<AiMessagePartDto>,
}

fn default_status() -> String {
    "complete".to_string()
}

/// 标准 UUID v4（小写带横线，与 Kotlin `UUID.randomUUID()` 同形）。
///
/// 用 `auth` 里那份系统 CSPRNG（随机数只留一处实现）：主键撞车在这里是**静默**
/// 的数据覆盖（`INSERT` 会撞主键、`ON CONFLICT` 会就地改写别人的消息），
/// 所以宁可复用一条已经被单测盯住的取随机数路径。
fn uuid_v4() -> Result<String, ApiError> {
    let mut bytes = crate::auth::random_bytes(16)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 10xx
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// `POST /api/ai/conversations` → **201** + 新会话。
///
/// 标题：trim 后为空就用 `"新对话"`（Kotlin 的 `?:`），并截到 255 字符
/// （那一列在 MySQL 时代是 `VARCHAR(255)`，SQLite 不拦，界面也不拦）。
pub fn create(state: &AppState, req: &ConversationCreate) -> Result<AiConversationDto, ApiError> {
    let id = uuid_v4()?;
    let title = req
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("新对话")
        .chars()
        .take(255)
        .collect::<String>();

    state.write(|tx| {
        tx.execute(
            "INSERT INTO ai_conversations (id, title, provider_id, model_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, title, req.provider_id, req.model_id],
        )?;
        Ok(())
    })?;
    get(state, &id)
}

/// `PATCH /api/ai/conversations/{id}`。
pub fn patch(state: &AppState, id: &str, req: &ConversationPatch) -> Result<AiConversationDto, ApiError> {
    let current = get(state, id)?;
    // 空标题 = 不改（不是"改成空"）：Kotlin 是 `?.takeIf { it.isNotEmpty() } ?: current.title`
    let title = req
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().take(255).collect::<String>())
        .unwrap_or(current.title);
    let provider_id = req.provider_id.clone().or(current.provider_id);
    let model_id = req.model_id.clone().or(current.model_id);
    let archived = req.archived.unwrap_or(current.archived);

    state.write(|tx| {
        tx.execute(
            "UPDATE ai_conversations
                SET title = ?1, provider_id = ?2, model_id = ?3, archived = ?4
              WHERE id = ?5",
            rusqlite::params![title, provider_id, model_id, if archived { 1 } else { 0 }, id],
        )?;
        Ok(())
    })?;
    get(state, id)
}

/// `DELETE /api/ai/conversations/{id}` —— 消息与有序块由**外键级联**清理（AIH-018）。
///
/// ⚠ 级联真的生效靠的是 R1（每条连接都 `PRAGMA foreign_keys=ON`）：
/// 漏了那条 PRAGMA，这里会**静默留下孤儿消息**，而且删除本身"成功"。
pub fn delete(state: &AppState, id: &str) -> Result<bool, ApiError> {
    let removed = state.write(|tx| {
        let n = tx.execute("DELETE FROM ai_conversations WHERE id = ?1", rusqlite::params![id])?;
        Ok(n > 0)
    })?;
    if !removed {
        return Err(conversation_missing(id));
    }
    Ok(true)
}

/// `POST /api/ai/conversations/{id}/messages` → **201** + 落库后的那条消息（带块）。
///
/// # 三条纪律
///
/// ① **校验在事务之前**：角色 / 状态非法与"会话不存在"都是 `AiException` 那一档
///    （`error` = 稳定码 + `detail` = 人话），而 `AppState::write` 的错误通道只认
///    `WriteError`（普通错误 = 人话）。Kotlin 也是这个顺序（`get()` → `Db.tx {}`）。
/// ② **`seq` 由服务端分配**（`COALESCE(MAX(seq),0)+1`）：调用方自己算的话，
///    两台设备同时追加就会拿到同一个序号，而消息顺序是"恢复顺序稳定"的唯一依据。
/// ③ **消息与其有序块同一个事务**：分开写就会出现"有消息没片段"的半截状态，
///    界面上表现为一条空回复（正是 R6 要防的那种"已经说出去的话丢了"）。
pub fn append_message(
    state: &AppState,
    conversation_id: &str,
    req: &MessageAppend,
) -> Result<AiMessageDto, ApiError> {
    if !ROLES.contains(&req.role.as_str()) {
        return Err(ApiError::ai("CONFIG_ERROR", format!("非法角色: {}", req.role)));
    }
    if !STATUSES.contains(&req.status.as_str()) {
        return Err(ApiError::ai("CONFIG_ERROR", format!("非法状态: {}", req.status)));
    }
    // 存在性检查（不存在 → `CONFIG_ERROR`，与 Kotlin 同）
    get(state, conversation_id)?;

    let id = match req.id.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        Some(given) => given.to_string(),
        None => uuid_v4()?,
    };
    let id_for_tx = id.clone();
    let conversation = conversation_id.to_string();
    let role = req.role.clone();
    let status = req.status.clone();
    let text = req.text.clone();
    let provider_id = req.provider_id.clone();
    let model_id = req.model_id.clone();
    let parts: Vec<(String, Option<String>, Option<String>, Option<String>, Option<String>)> = req
        .parts
        .iter()
        .map(|p| {
            (
                p.r#type.clone(),
                p.text.clone(),
                p.attachment_id.clone(),
                p.tool_call_id.clone(),
                p.json_payload.as_ref().map(|v| v.to_string()),
            )
        })
        .collect();

    state.write(move |tx| {
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM ai_messages WHERE conversation_id = ?1",
            rusqlite::params![conversation],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO ai_messages
               (id, conversation_id, seq, role, status, text, provider_id, model_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![id_for_tx, conversation, seq, role, status, text, provider_id, model_id],
        )?;
        for (ordinal, part) in parts.iter().enumerate() {
            tx.execute(
                "INSERT INTO ai_message_parts
                   (message_id, ordinal, type, text, attachment_id, tool_call_id, json_payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![id_for_tx, ordinal as i64, part.0, part.1, part.2, part.3, part.4],
            )?;
        }
        // 会话列表按 updated_at 排序：发一条消息必须把它顶上去，否则这次对话会沉底
        tx.execute(
            "UPDATE ai_conversations SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
            rusqlite::params![conversation],
        )?;
        Ok(())
    })?;

    // 读回**落库后的那一行**（而不是把入参拼回去）：seq / 时间戳 / 块顺序都以库为准
    list_messages(state, conversation_id)?
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| ApiError::internal(format!("消息写入后读不到: {id}")))
}

/// 坏 JSON **不能让整个接口 500**（Kotlin 那边是 `runCatching { … }.getOrNull()`）：
/// 一条脏的 `json_payload` 只会让那张工具卡少一块内容，不该连累整段历史。
fn parse_json_any(raw: Option<&str>) -> Option<Value> {
    let text = raw?.trim();
    if text.is_empty() {
        return None;
    }
    serde_json::from_str(text).ok()
}

/// `usage_json` 那一列：形状上是个对象，但坏数据同样按"没有"处理。
fn parse_json_object(raw: Option<&str>) -> Option<Value> {
    match parse_json_any(raw) {
        Some(Value::Object(map)) => Some(Value::Object(map)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 库里的 NULL 在 DTO 上的**回落值**（Kotlin `toConversation` / `toMessage` 的那几个 `?:`）。
    ///
    /// ⚠ 用"算出来的行"测，而不是夹具：真 schema 上 `title` / `system_prompt_version` /
    /// `status` 都是 NOT NULL，夹具里**构造不出** NULL —— 只靠集成测试的话，
    /// 这几条回落路径等于没有防线（老 MySQL 搬过来的脏行才走得到）。
    #[test]
    fn 可空列在_dto_上都有回落值() {
        let conn = crate::db::open_memory().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT 'c1' AS id, NULL AS title, NULL AS provider_id, NULL AS model_id,
                        NULL AS system_prompt_version, NULL AS archived, NULL AS message_count,
                        NULL AS created_at, NULL AS updated_at",
            )
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        let conv = conversation_from_row(rows.next().unwrap().unwrap()).unwrap();
        assert_eq!(conv.title, "新对话", "标题空着就回落成「新对话」，不是空串");
        assert_eq!(conv.system_prompt_version, "v1");
        assert!(!conv.archived);
        assert_eq!(conv.message_count, 0, "取不到 message_count 就当 0（那两条 SQL 之外的行）");
        assert!(conv.created_at.is_none() && conv.updated_at.is_none());

        let mut stmt = conn
            .prepare(
                "SELECT 'm1' AS id, 'c1' AS conversation_id, NULL AS seq, NULL AS role,
                        NULL AS status, NULL AS text, NULL AS provider_id, NULL AS model_id,
                        NULL AS provider_response_id, NULL AS usage_json,
                        NULL AS created_at, NULL AS updated_at",
            )
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        let msg = message_from_row(rows.next().unwrap().unwrap()).unwrap();
        assert_eq!(msg.status, "complete");
        assert_eq!(msg.text, "");
        assert_eq!(msg.seq, 0);
        assert!(msg.parts.is_empty());
    }

    /// 脏 `json_payload` / `usage_json` → **当没有**（键不出现），不抛。
    #[test]
    fn 坏的_json_列按没有处理() {
        assert_eq!(parse_json_any(Some("not json")), None);
        assert_eq!(parse_json_any(Some("  ")), None);
        assert_eq!(parse_json_any(None), None);
        // usage 只认对象：数组 / 标量一概当没有（Kotlin 那边 `runCatching` 能拿到数组，
        // 但那个列本来只会写对象；这里收紧成"不是对象就没有"，免得界面上出现一个读不懂的字段）
        assert_eq!(parse_json_object(Some("[1,2]")), None);
        assert_eq!(parse_json_object(Some("12")), None);
        assert!(parse_json_object(Some(r#"{"a":1}"#)).is_some());
    }
}
