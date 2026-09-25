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
    let asks = state.with(crate::questions::list_pending)?;
    let approvals = state.with(list_pending_approvals)?;
    let mut body = Map::new();
    body.insert("asks".into(), Value::Array(asks));
    body.insert("approvals".into(), Value::Array(approvals));
    Ok(Value::Object(body))
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

#[cfg(test)]
mod tests {
    //! ⚠ 提问那半边（形状 / ISO→毫秒 / 脏 JSON）的用例跟着实现搬到了
    //! [`crate::questions::tests`] —— 形状只有一份，用例也就不该有两份。

    use super::*;

    #[test]
    fn 没有待办时也是两个空数组() {
        let mut body = Map::new();
        body.insert("asks".into(), Value::Array(Vec::new()));
        body.insert("approvals".into(), Value::Array(Vec::new()));
        assert_eq!(body.len(), 2, "两个键都要在（不是 null、也不是少一个）");
    }
}
