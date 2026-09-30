//! 提问闸门（P4 第 32 步）—— `ask_user_question` 那一条链的**持久层 + 形状**。
//!
//! 逐条对照 Kotlin `AskGate.kt` / `AskRegistry` / `AiQuestionRepo`。它同时服务三件事：
//!
//! 1. **界面重开 / 断线重连**：`GET /api/ai/runs/{id}/question` 与
//!    `GET /api/ai/questions/{id}` 要能回答"还在等你回答什么"（内存闸门随进程消失，
//!    表里那行才是"卡还在"的唯一事实 —— M1 的纪律）；
//! 2. **等答案**：`ask_user_question` 挂在这里轮询 `status`（跨端可答：
//!    手机 / 第二个窗口点的提交也作数）；
//! 3. **挡重复**：同一个 Run 里已经问过、还在等的那条不许再开一张卡。
//!
//! # 形状（与 Kotlin `PendingQuestion.toJson()` 逐字对齐）
//!
//! ```jsonc
//! { questionId, runId, conversationId, question, allowFreeText, answered, options[],
//!   askedAt, index, total, groupId, multiSelect, header?, answer?, optionIndex?, optionIndexes? }
//! ```
//!
//! ⚠ 两条容易做错的：**`askedAt` 是数字（epoch 毫秒）**，不是 ISO 文本
//! （`/api/ai/pending` 里审批那份才是 ISO —— 两个 `askedAt` 类型不同，别"统一"）；
//! 可空键**整个键不出现**（`explicitNulls = false`）。
//! 这两条与 [`crate::pending`] 共用**同一份** [`row_to_json`]，所以不存在第二份形状。

use rusqlite::{Connection, Row};
use serde_json::{Map, Value};

use crate::routes::{ApiError, AppState};

/// 一张提问卡最多几个问题（Kotlin `ToolContext.MAX_QUESTIONS_PER_CARD`，用户拍板 5）。
pub const MAX_QUESTIONS_PER_CARD: usize = 5;
/// 一窗（一次回复）最多开几张提问卡（阈值制：**不问是常态**）。
pub const MAX_ASK_CARDS_PER_WINDOW: i64 = 1;
/// 一次 Run 里最多因"用户回答了提问"重置几次工具预算。
pub const MAX_BUDGET_RESETS: i64 = 4;
/// 一条提问的等待上限（秒）—— 与 Kotlin `AskGate(timeoutMs = 30 分钟)` 同。
pub const QUESTION_TIMEOUT_SECONDS: u64 = 30 * 60;
/// 拿到任意一条答复之后，再给其余几条多久（毫秒）。
///
/// 20 秒足够"整张卡一次提交"的界面把剩下的送达，又不至于让只交一题的旧界面把 Run 拖住
/// —— 这个数字是"降级体验"的边界，不是功能参数（Kotlin `DEFAULT_GRACE_MS`）。
pub const DEFAULT_GRACE_MS: u64 = 20_000;
/// 等答案时的轮询间隔（毫秒）。闸门读**库**（跨端可见），所以是轮询。
pub const QUESTION_POLL_MS: u64 = 150;

/// `ai_questions` 的列清单 —— **只此一份**（`SELECT` 与行 → JSON 都从它派生，
/// 少一列就是界面上少一块内容，而那种错不报错）。
pub const COLS: &str = "question_id, run_id, conversation_id, group_id, card_index, total,
                        question, options_json, allow_free_text, multi_select, header,
                        status, answer, option_index, option_indexes, asked_at";

/// 一行提问的完整内容（`status='pending'` 之外的行也读 —— 历史卡片回放要用）。
#[derive(Debug, Clone)]
pub struct StoredQuestion {
    pub question_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub group_id: String,
    pub index: i64,
    pub total: i64,
    pub question: String,
    pub options: Vec<String>,
    pub allow_free_text: bool,
    pub multi_select: bool,
    pub header: Option<String>,
    pub status: String,
    pub answer: Option<String>,
    pub option_index: Option<i64>,
    pub option_indexes: Vec<i64>,
    /// epoch 毫秒（库里是 ISO 文本，这一层换算成与 Kotlin 同形的数字）
    pub asked_at_ms: i64,
}

impl StoredQuestion {
    /// 发给界面的形状（SSE 事件与 HTTP 查询**共用一份** —— Kotlin 也是同一个 `toJson()`）。
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("questionId".into(), Value::String(self.question_id.clone()));
        out.insert("runId".into(), Value::String(self.run_id.clone()));
        out.insert("conversationId".into(), Value::String(self.conversation_id.clone()));
        out.insert("question".into(), Value::String(self.question.clone()));
        out.insert("allowFreeText".into(), Value::Bool(self.allow_free_text));
        out.insert("answered".into(), Value::Bool(self.answer.is_some()));
        out.insert(
            "options".into(),
            Value::Array(self.options.iter().cloned().map(Value::String).collect()),
        );
        out.insert("askedAt".into(), Value::from(self.asked_at_ms));
        out.insert("index".into(), Value::from(self.index));
        out.insert("total".into(), Value::from(self.total));
        out.insert(
            "groupId".into(),
            Value::String(if self.group_id.is_empty() {
                self.question_id.clone()
            } else {
                self.group_id.clone()
            }),
        );
        out.insert("multiSelect".into(), Value::Bool(self.multi_select));
        // 可空键：**没有就整个键不出现**（explicitNulls = false）
        if let Some(header) = self.header.as_ref().filter(|h| !h.is_empty()) {
            out.insert("header".into(), Value::String(header.clone()));
        }
        if let Some(answer) = self.answer.as_ref() {
            out.insert("answer".into(), Value::String(answer.clone()));
        }
        if let Some(index) = self.option_index {
            out.insert("optionIndex".into(), Value::from(index));
        }
        if !self.option_indexes.is_empty() {
            out.insert(
                "optionIndexes".into(),
                Value::Array(self.option_indexes.iter().copied().map(Value::from).collect()),
            );
        }
        Value::Object(out)
    }

    /// 带答案的副本（工具结果里的 `asked` / `askedAll` 要连答案一起落库）。
    pub fn with_answer(&self, answer: &Answer) -> Self {
        let mut next = self.clone();
        next.answer = Some(answer.answer.clone());
        next.option_index = answer.option_index;
        next.option_indexes = answer.option_indexes.clone();
        next
    }
}

/// 用户对一条提问的回答（Kotlin `AskedAnswer`）。
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub answer: String,
    /// 用户点的是第几个选项（自己敲的为 `None`）
    pub option_index: Option<i64>,
    /// 多选题勾了哪几个
    pub option_indexes: Vec<i64>,
}

/// 新开一条提问（`ask_user_question` 用）。
pub struct NewQuestion {
    pub question_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub group_id: String,
    pub index: i64,
    pub total: i64,
    pub question: String,
    pub options: Vec<String>,
    pub multi_select: bool,
    pub header: Option<String>,
}

/// 一行的列 → JSON（**`pending.rs` 与这里共用**，所以形状只有一份）。
pub fn row_to_json(row: &Row<'_>) -> rusqlite::Result<Value> {
    let question_id: String = row.get("question_id")?;
    let answer: Option<String> = row.get("answer")?;
    let option_indexes = parse_int_array(row.get::<_, Option<String>>("option_indexes")?.as_deref());
    let mut out = Map::new();
    out.insert("questionId".into(), Value::String(question_id.clone()));
    out.insert("runId".into(), Value::String(row.get::<_, Option<String>>("run_id")?.unwrap_or_default()));
    out.insert(
        "conversationId".into(),
        Value::String(row.get::<_, Option<String>>("conversation_id")?.unwrap_or_default()),
    );
    out.insert(
        "question".into(),
        Value::String(row.get::<_, Option<String>>("question")?.unwrap_or_default()),
    );
    out.insert(
        "allowFreeText".into(),
        Value::Bool(row.get::<_, Option<i64>>("allow_free_text")?.unwrap_or(1) != 0),
    );
    out.insert("answered".into(), Value::Bool(answer.is_some()));
    out.insert(
        "options".into(),
        Value::Array(parse_string_array(row.get::<_, Option<String>>("options_json")?.as_deref())),
    );
    out.insert(
        "askedAt".into(),
        Value::from(asked_at_ms(row.get::<_, Option<String>>("asked_at")?.as_deref())),
    );
    out.insert("index".into(), Value::from(row.get::<_, Option<i64>>("card_index")?.unwrap_or(0)));
    out.insert("total".into(), Value::from(row.get::<_, Option<i64>>("total")?.unwrap_or(1)));
    out.insert(
        "groupId".into(),
        Value::String(
            row.get::<_, Option<String>>("group_id")?
                .filter(|v| !v.is_empty())
                .unwrap_or(question_id),
        ),
    );
    out.insert(
        "multiSelect".into(),
        Value::Bool(row.get::<_, Option<i64>>("multi_select")?.unwrap_or(0) != 0),
    );
    if let Some(header) = row.get::<_, Option<String>>("header")? {
        out.insert("header".into(), Value::String(header));
    }
    if let Some(answer) = answer {
        out.insert("answer".into(), Value::String(answer));
    }
    if let Some(index) = row.get::<_, Option<i64>>("option_index")? {
        out.insert("optionIndex".into(), Value::from(index));
    }
    if !option_indexes.is_empty() {
        out.insert("optionIndexes".into(), Value::Array(option_indexes));
    }
    Ok(Value::Object(out))
}

/// 一行的列 → [`StoredQuestion`]（等答案 / 工具结果要结构化地读它）。
fn row_to_stored(row: &Row<'_>) -> rusqlite::Result<StoredQuestion> {
    Ok(StoredQuestion {
        question_id: row.get("question_id")?,
        run_id: row.get::<_, Option<String>>("run_id")?.unwrap_or_default(),
        conversation_id: row.get::<_, Option<String>>("conversation_id")?.unwrap_or_default(),
        group_id: row.get::<_, Option<String>>("group_id")?.unwrap_or_default(),
        index: row.get::<_, Option<i64>>("card_index")?.unwrap_or(0),
        total: row.get::<_, Option<i64>>("total")?.unwrap_or(1),
        question: row.get::<_, Option<String>>("question")?.unwrap_or_default(),
        options: parse_string_array_str(row.get::<_, Option<String>>("options_json")?.as_deref()),
        allow_free_text: row.get::<_, Option<i64>>("allow_free_text")?.unwrap_or(1) != 0,
        multi_select: row.get::<_, Option<i64>>("multi_select")?.unwrap_or(0) != 0,
        header: row.get::<_, Option<String>>("header")?,
        status: row.get::<_, Option<String>>("status")?.unwrap_or_default(),
        answer: row.get("answer")?,
        option_index: row.get("option_index")?,
        option_indexes: parse_int_array(row.get::<_, Option<String>>("option_indexes")?.as_deref())
            .into_iter()
            .filter_map(|v| v.as_i64())
            .collect(),
        asked_at_ms: asked_at_ms(row.get::<_, Option<String>>("asked_at")?.as_deref()),
    })
}

/// 落一行 pending（`track()` 的同一刻就落 —— M1 的纪律：**内存里活着但库里没有不许存在**）。
pub fn insert(state: &AppState, q: &NewQuestion) -> Result<(), ApiError> {
    let options = serde_json::to_string(&q.options).unwrap_or_else(|_| "[]".into());
    let q = NewQuestionRow {
        question_id: q.question_id.clone(),
        run_id: q.run_id.clone(),
        conversation_id: q.conversation_id.clone(),
        group_id: q.group_id.clone(),
        index: q.index,
        total: q.total,
        question: q.question.clone(),
        options,
        multi_select: q.multi_select,
        header: q.header.clone(),
    };
    state.write(move |tx| {
        tx.execute(
            "INSERT INTO ai_questions
               (question_id, run_id, conversation_id, group_id, card_index, total, question,
                options_json, allow_free_text, multi_select, header, status, asked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, 'pending',
                     strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            rusqlite::params![
                q.question_id,
                q.run_id,
                q.conversation_id,
                q.group_id,
                q.index,
                q.total,
                q.question,
                q.options,
                if q.multi_select { 1 } else { 0 },
                q.header
            ],
        )?;
        Ok(())
    })
}

/// `insert` 的搬运用形态（避开借用问题：`state.write` 的闭包要 `'static`）。
struct NewQuestionRow {
    question_id: String,
    run_id: String,
    conversation_id: String,
    group_id: String,
    index: i64,
    total: i64,
    question: String,
    options: String,
    multi_select: bool,
    header: Option<String>,
}

/// 按 questionId 查（**任何状态**都查得到 —— 历史卡片回放要用）。
pub fn find(state: &AppState, question_id: &str) -> Result<Option<Value>, ApiError> {
    let question_id = question_id.to_string();
    state.with(move |conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM ai_questions WHERE question_id = ?1"
        ))?;
        let mut rows = stmt.query_map(rusqlite::params![question_id], row_to_json)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    })
}

/// 结构化地读一行（等答案与工具结果用）。
pub fn find_stored(state: &AppState, question_id: &str) -> Result<Option<StoredQuestion>, ApiError> {
    let question_id = question_id.to_string();
    state.with(move |conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM ai_questions WHERE question_id = ?1"
        ))?;
        let mut rows = stmt.query_map(rusqlite::params![question_id], row_to_stored)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    })
}

/// 还在等回答的提问（**全后端**，`/api/ai/pending` 用）。
pub fn list_pending(conn: &Connection) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM ai_questions
          WHERE status = 'pending'
          ORDER BY asked_at, question_id"
    ))?;
    let rows = stmt
        .query_map([], row_to_json)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 某个 Run 里**还在等**的整张卡（同一 `group_id`，按题号排）。
///
/// 界面重开 / 断线重连要把 N 页都补回来 —— 只回第一条的话，用户看到的是一张缺题的卡。
pub fn pending_group_for(state: &AppState, run_id: &str) -> Result<Vec<Value>, ApiError> {
    let run_id = run_id.to_string();
    state.with(move |conn| {
        // 先找"最新那条还在等的"的 group_id，再把它整组取出来
        let group: Option<String> = conn
            .query_row(
                "SELECT group_id FROM ai_questions
                  WHERE run_id = ?1 AND status = 'pending'
                  ORDER BY asked_at DESC, question_id DESC LIMIT 1",
                rusqlite::params![run_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten();
        let Some(group) = group else { return Ok(Vec::new()) };
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM ai_questions
              WHERE run_id = ?1 AND group_id = ?2 AND status = 'pending'
              ORDER BY card_index"
        ))?;
        let rows = stmt
            .query_map(rusqlite::params![run_id, group], row_to_json)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// 这个 Run 里是不是已经问过一模一样的问题（**还在等**的才算）。
///
/// 不挡的话模型会连开好几张一模一样的卡片。
pub fn find_same_question(
    state: &AppState,
    run_id: &str,
    question: &str,
) -> Result<Option<(String, String)>, ApiError> {
    let run_id = run_id.to_string();
    let want = question.trim().to_string();
    state.with(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT question_id, question FROM ai_questions
              WHERE run_id = ?1 AND status = 'pending'
              ORDER BY asked_at",
        )?;
        let rows = stmt.query_map(rusqlite::params![run_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?.unwrap_or_default()))
        })?;
        for row in rows {
            let (id, text) = row?;
            if text.trim() == want {
                return Ok(Some((id, text)));
            }
        }
        Ok(None)
    })
}

/// 用户回答了：**只改还 pending 的那一行**，返回是否被采纳。
///
/// `false` = 这条提问已经不在等待（早就答过 / 超时收尾了），界面要如实说
/// "这次回答没被采纳"，不许假装成功。
pub fn record_answer(
    state: &AppState,
    question_id: &str,
    answer: &Answer,
) -> Result<bool, ApiError> {
    let question_id = question_id.to_string();
    let answer = answer.clone();
    state.write(move |tx| {
        let indexes = if answer.option_indexes.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&answer.option_indexes).unwrap_or_else(|_| "[]".into()))
        };
        let n = tx.execute(
            "UPDATE ai_questions
                SET status = 'answered', answer = ?1, option_index = ?2, option_indexes = ?3,
                    answered_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
              WHERE question_id = ?4 AND status = 'pending'",
            rusqlite::params![answer.answer, answer.option_index, indexes, question_id],
        )?;
        Ok(n > 0)
    })
}

/// 把还挂着 pending 的那一行按 `status` 收尾（`skipped` / `expired`）。
///
/// **状态守卫**：只动 `pending` —— 已经 `answered` 的不许被覆盖（用户答完又被收尾抹掉，
/// 那是"我明明答了它却说没答"）。
pub fn finalize_if_pending(state: &AppState, question_id: &str, status: &str) -> Result<usize, ApiError> {
    let question_id = question_id.to_string();
    let status = status.to_string();
    state.write(move |tx| {
        let n = tx.execute(
            "UPDATE ai_questions
                SET status = ?1, answered_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
              WHERE question_id = ?2 AND status = 'pending'",
            rusqlite::params![status, question_id],
        )?;
        Ok(n)
    })
}

/// Run 收尾：这个 Run 上还没答的提问一律按 `status`（一般是 `expired`）收尾。
///
/// 少了这一步，那些行会永远停在 pending：`/api/ai/pending` 一直把它们列出来，
/// 而点进去谁也答不了（Run 已经结束了）。
pub fn finalize_run(state: &AppState, run_id: &str, status: &str) -> Result<usize, ApiError> {
    let run_id = run_id.to_string();
    let status = status.to_string();
    state.write(move |tx| {
        let n = tx.execute(
            "UPDATE ai_questions
                SET status = ?1, answered_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
              WHERE run_id = ?2 AND status = 'pending'",
            rusqlite::params![status, run_id],
        )?;
        Ok(n)
    })
}

/// 后端重启恢复：库里还标着 pending 的行都是"再也不会有人来回答"的了（Kotlin
/// `AiQuestionRepo.expireStalePending`，调用点在 `fail_stale_running` 之后）。
pub fn expire_stale(state: &AppState) -> Result<usize, ApiError> {
    state.write(|tx| {
        let n = tx.execute(
            "UPDATE ai_questions SET status = 'expired',
                    answered_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
              WHERE status = 'pending'",
            [],
        )?;
        Ok(n)
    })
}

/// 等答案期间的读数：这一条**现在**是什么状态。
#[derive(Debug, Clone, PartialEq)]
pub enum QuestionState {
    /// 还没人答
    Pending,
    /// 答了（`answer` 就是内容；用户点了"跳过"不算答 —— 那是"他没回答"）
    Answered(Answer),
    /// 跳过了 / 已经失效（Run 收尾、后端重启）
    Gone,
}

/// 读一条提问的当前状态（等答案的轮询用它）。
pub fn state_of(state: &AppState, question_id: &str) -> Result<QuestionState, ApiError> {
    let Some(row) = find_stored(state, question_id)? else {
        // 表里根本没有这一行：当作"没了"，由调用方按"没回答"处理
        return Ok(QuestionState::Gone);
    };
    Ok(match (row.status.as_str(), row.answer) {
        ("pending", _) => QuestionState::Pending,
        ("answered", Some(answer)) => QuestionState::Answered(Answer {
            answer,
            option_index: row.option_index,
            option_indexes: row.option_indexes,
        }),
        // answered 但没有正文（脏数据）/ skipped / expired 一律按"没回答"
        _ => QuestionState::Gone,
    })
}

/// ISO 文本 → epoch **毫秒**（解析不出来回 0，与 Kotlin `askedAtMs ?: 0L` 同）。
pub fn asked_at_ms(raw: Option<&str>) -> i64 {
    match crate::iso_time::parse(raw.unwrap_or("")) {
        Some((secs, nanos)) => secs.saturating_mul(1000) + (nanos / 1_000_000) as i64,
        None => 0,
    }
}

fn parse_string_array(raw: Option<&str>) -> Vec<Value> {
    parse_string_array_str(raw).into_iter().map(Value::String).collect()
}

fn parse_string_array_str(raw: Option<&str>) -> Vec<String> {
    let Some(text) = raw.map(str::trim).filter(|t| !t.is_empty()) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(items)) => items
            .into_iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        // 脏 JSON 当没有（与 Kotlin `parseStringArray` 的 `runCatching` 同）
        _ => Vec::new(),
    }
}

fn parse_int_array(raw: Option<&str>) -> Vec<Value> {
    parse_string_array(raw)
        .into_iter()
        .filter_map(|v| match v {
            Value::String(s) => s.trim().parse::<i64>().ok().map(Value::from),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;
