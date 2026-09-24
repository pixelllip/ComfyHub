//! **跨端待办总览**：`GET /api/ai/pending`（M1 定的契约，P4 第 25 步搬到 Rust）。
//!
//! # 它是给谁用的
//!
//! "另一台设备刚连上来"——手机、第二个窗口、或者刚重启完的桌面 App。它**没有经历**
//! 那条 SSE 事件流，所以那些"正在等人"的卡片（AI 的提问、需要批准的工具调用）
//! 靠这一个接口一次补齐，而不是逐个 Run 去问。
//!
//! # ⚠ 与 Kotlin 侧的一处**必须说清的差异**
//!
//! Kotlin 那边 `asks` 来自**内存里的提问闸门**（`AskRegistry.allPending()`），
//! `approvals` 来自 `ai_tool_calls` 的 pending 行。Rust 侧**没有 Run 执行器**，
//! 也就没有那个内存闸门 —— 所以这里 `asks` 直接读 **`ai_questions` 表里 `status='pending'` 的行**。
//!
//! 为什么这不是"退一步的实现"，而是同一件事：
//! M1 的纪律是 **`AskRegistry.track()` 当场落一行 pending**（正是为了"界面还没挂上、
//! 后端重启了，那张卡也不能丢"），而收尾只把 `pending` 改成终态。
//! 于是"表里还 pending 的行" ≡ "闸门里还活着的提问" —— 前提被写进了 M1 的用例
//! （`AskPersistenceTest`）。反过来，**内存里活着但库里没有**这种状态 M1 起就不存在。
//!
//! 真机上验这一条很容易：真库副本上 `sqlite3 ... "SELECT status, COUNT(*) FROM ai_questions GROUP BY status"`，
//! 有 pending 行时本接口就该回出来那几条。
//!
//! # 形状（与 Kotlin 逐字对齐，键名错一个界面就少一块内容）
//!
//! ```jsonc
//! { "asks":       [ {questionId, runId, conversationId, question, options[], allowFreeText,
//!                    answered, askedAt, index, total, groupId, multiSelect,
//!                    header?, answer?, optionIndex?, optionIndexes?} ],
//!   "approvals":  [ {callId, runId, conversationId, name, argumentsJson, askedAt} ] }
//! ```
//!
//! 两条容易做错的地方：
//! ① **`askedAt` 是数字（epoch 毫秒）**，不是 ISO 文本 —— 提问那份是 `askedAtMs`，
//!    而审批那份是 ISO 字符串（`rs.isoTime`）。两个 `askedAt` 类型不同，别想着"统一一下"；
//! ② 可空键**不出现**（`explicitNulls = false`）：`header` / `answer` / `optionIndex`
//!    没有就整个键不出现，`optionIndexes` 空数组也不出现。

use rusqlite::Connection;
use serde_json::{Map, Value};

use crate::iso_time::iso_time;
use crate::routes::{ApiError, AppState};

/// `GET /api/ai/pending`。
pub fn pending(state: &AppState) -> Result<Value, ApiError> {
    let asks = state.with(list_pending_asks)?;
    let approvals = state.with(list_pending_approvals)?;
    let mut body = Map::new();
    body.insert("asks".into(), Value::Array(asks));
    body.insert("approvals".into(), Value::Array(approvals));
    Ok(Value::Object(body))
}

/// 还在等回答的提问（`ai_questions.status = 'pending'`）。
///
/// 排序照抄 Kotlin 那边的可预期顺序：先按问的时间、再按 question_id
/// （同一毫秒落两行时靠 id 才稳定 —— 卡片顺序乱了用户看到的是"题号跳来跳去"）。
fn list_pending_asks(conn: &Connection) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT question_id, run_id, conversation_id, group_id, card_index, total,
                question, options_json, allow_free_text, multi_select, header,
                status, answer, option_index, option_indexes, asked_at
           FROM ai_questions
          WHERE status = 'pending'
          ORDER BY asked_at, question_id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let question_id: String = row.get("question_id")?;
            let asked_at: Option<String> = row.get("asked_at")?;
            let answer: Option<String> = row.get("answer")?;
            let option_index: Option<i64> = row.get("option_index")?;
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
            out.insert("allowFreeText".into(), Value::Bool(row.get::<_, Option<i64>>("allow_free_text")?.unwrap_or(1) != 0));
            // 表里只有 pending 的行；`answered` 就按"有没有答案"算（与 Kotlin 同义）
            out.insert("answered".into(), Value::Bool(answer.is_some()));
            out.insert("options".into(), Value::Array(parse_string_array(row.get::<_, Option<String>>("options_json")?.as_deref())));
            out.insert("askedAt".into(), Value::from(asked_at_ms(asked_at.as_deref())));
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
            out.insert("multiSelect".into(), Value::Bool(row.get::<_, Option<i64>>("multi_select")?.unwrap_or(0) != 0));
            // 以下是可空键：**没有就整个键不出现**（explicitNulls = false）
            if let Some(header) = row.get::<_, Option<String>>("header")? {
                out.insert("header".into(), Value::String(header));
            }
            if let Some(answer) = answer {
                out.insert("answer".into(), Value::String(answer));
            }
            if let Some(index) = option_index {
                out.insert("optionIndex".into(), Value::from(index));
            }
            if !option_indexes.is_empty() {
                out.insert("optionIndexes".into(), Value::Array(option_indexes));
            }
            Ok(Value::Object(out))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 还在等批准的调用：`ai_tool_calls.status='pending'` **且 Run 还在 running**。
///
/// ⚠ 两个条件缺一不可：Run 已经收尾的 pending 行是"永远等不到人来点"的残留
/// （后端重启恢复会把它们按拒绝收尾），把它列出来会让另一台设备上冒出一张
/// **点了也没用**的审批卡。
fn list_pending_approvals(conn: &Connection) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT tc.provider_call_id, tc.run_id, r.conversation_id, tc.name,
                tc.arguments_json, tc.started_at
           FROM ai_tool_calls tc
           JOIN ai_runs r ON r.id = tc.run_id
          WHERE tc.status = 'pending' AND r.status = 'running'
          ORDER BY tc.started_at, tc.id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let mut out = Map::new();
            out.insert(
                "callId".into(),
                Value::String(row.get::<_, Option<String>>("provider_call_id")?.unwrap_or_default()),
            );
            out.insert("runId".into(), Value::String(row.get::<_, Option<String>>("run_id")?.unwrap_or_default()));
            out.insert(
                "conversationId".into(),
                Value::String(row.get::<_, Option<String>>("conversation_id")?.unwrap_or_default()),
            );
            out.insert("name".into(), Value::String(row.get::<_, Option<String>>("name")?.unwrap_or_default()));
            out.insert(
                "argumentsJson".into(),
                Value::String(row.get::<_, Option<String>>("arguments_json")?.unwrap_or_default()),
            );
            // ⚠ 这一份是 **ISO 文本**（提问那份是毫秒数），照 Kotlin `rs.isoTime` 的规则
            out.insert(
                "askedAt".into(),
                match iso_time(row.get::<_, Option<String>>("started_at")?.as_deref()) {
                    Some(text) => Value::String(text),
                    None => Value::Null,
                },
            );
            Ok(Value::Object(out))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// ISO 文本 → epoch **毫秒**（解析不出来回 0，与 Kotlin `askedAtMs ?: 0L` 同）。
fn asked_at_ms(raw: Option<&str>) -> i64 {
    match crate::iso_time::parse(raw.unwrap_or("")) {
        Some((secs, nanos)) => secs.saturating_mul(1000) + (nanos / 1_000_000) as i64,
        None => 0,
    }
}

fn parse_string_array(raw: Option<&str>) -> Vec<Value> {
    let Some(text) = raw.map(str::trim).filter(|t| !t.is_empty()) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(items)) => items
            .into_iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(Value::String(s)),
                Value::Number(n) => Some(Value::String(n.to_string())),
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
mod tests {
    use super::*;

    #[test]
    fn asked_at_是毫秒而脏数据回零() {
        // `2026-09-24T14:41:29.647Z`（本机真机验收那一刻）
        assert_eq!(asked_at_ms(Some("2026-09-24T14:41:29.647Z")), 1_790_260_889_647);
        // 整秒（没有小数点）也要算对 —— 这正是 `iso_time` 那一课
        assert_eq!(asked_at_ms(Some("2026-09-24T14:41:29Z")), 1_790_260_889_000);
        assert_eq!(asked_at_ms(Some("")), 0);
        assert_eq!(asked_at_ms(Some("不是时间")), 0);
        assert_eq!(asked_at_ms(None), 0);
    }

    #[test]
    fn 脏的_options_json_当没有() {
        assert!(parse_string_array(Some("not json")).is_empty());
        assert!(parse_string_array(Some("  ")).is_empty());
        assert!(parse_string_array(None).is_empty());
        assert_eq!(parse_string_array(Some(r#"["a","b"]"#)).len(), 2);
        // optionIndexes 那一列是**数字数组的 JSON**
        let idx = parse_int_array(Some("[0,2]"));
        assert_eq!(idx, vec![Value::from(0), Value::from(2)]);
        assert!(parse_int_array(Some(r#"["x"]"#)).is_empty(), "认不出来的分量要丢掉，不是编一个 0");
    }
}
